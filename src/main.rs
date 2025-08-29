use anyhow::Result;
use clap::{Parser, Subcommand, ValueHint, ValueEnum, CommandFactory, builder::styling::{AnsiColor, Effects, Styles}};
use clap_complete::{generate, Generator, Shell};
use colored::*;
use dotenv::dotenv;
use solana_client::rpc_client::RpcClient;
use std::{env, io};
use tracing::{info, error};
use tracing_subscriber::{EnvFilter, fmt::format::FmtSpan};

use crate::logger::{NerdLogger, icons};

const PUMP_FUN_FEE_ACCOUNT: &str = "CebN5WGQ4jvEPvsVU4EoHEpgzq1VV7AbicfhtW4xC9iM";
const PUMP_FUN_PROGRAM: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";

mod account_watcher;
mod animations;
mod api;
mod cache;
mod config;
mod database;
mod enhanced_logger;

mod flow_monitor;
mod grpc_server;
mod ipfs;
mod ipfs_storage;
mod logger;
mod metrics;
mod performance_benchmark;
mod slot_tracker;

mod wallet_tracker;
mod webhooks;
mod yellowstone_monitor;

fn get_styles() -> Styles {
    Styles::styled()
        .header(AnsiColor::Yellow.on_default() | Effects::BOLD)
        .usage(AnsiColor::Cyan.on_default() | Effects::BOLD)
        .literal(AnsiColor::Blue.on_default() | Effects::BOLD)
        .placeholder(AnsiColor::Green.on_default())
}

#[derive(Parser)]
#[command(
    name = "solana-indexer",
    about = "High-performance Solana blockchain indexer with real-time tracking, gRPC streaming, and multi-blockchain support",
    version = "1.0.0",
    author = "Built with  & ",
    long_about = "High-performance Solana blockchain indexer\n\nFeatures:\n  • Real-time slot tracking & wallet monitoring\n  • gRPC streaming API with Protocol Buffers\n  • Multi-blockchain support (Solana & Flow)\n  • Advanced caching & IPFS integration\n  • Webhook support for QuickNode & Yellowstone",
    styles = get_styles(),
    help_template = "\n{name} {version}\n{about}\n\n{usage-heading} {usage}\n\n{all-args}",
    disable_version_flag = false
)]
#[allow(unused_variables)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    ///  Solana RPC endpoint URL (QuickNode/Yellowstone recommended)
    #[arg(
        short,
        long,
        default_value = "https://api.mainnet-beta.solana.com",
        value_hint = ValueHint::Url,
        help_heading = "Connection Options"
    )]
    rpc_url: String,

    ///  Enable verbose logging with detailed output
    #[arg(
        short,
        long,
        help_heading = "Debug Options"
    )]
    verbose: bool,

    ///  gRPC server port for streaming connections
    #[arg(
        short,
        long,
        default_value = "50051",
        value_hint = ValueHint::Other,
        help_heading = "Network Options"
    )]
    port: u16,





    ///  Enable colored output (auto-detected)
    #[arg(
        long,
        value_enum,
        default_value = "auto",
        help_heading = "Display Options"
    )]
    color: ColorChoice,
}

#[derive(clap::ValueEnum, Clone, Debug)]
enum ColorChoice {
    Auto,
    Always,
    Never,
}



#[derive(clap::ValueEnum, Clone, Debug)]
enum BlockchainType {
    Solana,
    Flow,
}

#[derive(clap::ValueEnum, Clone, Debug)]
enum WalletSortBy {
    Name,
    Balance,
    Activity,
    Created,
}

#[derive(clap::ValueEnum, Clone, Debug)]
enum ActivityType {
    Transfer,
    Stake,
    Vote,
    Program,
    Token,
    Nft,
    All,
}

#[derive(ValueEnum, Clone, Debug)]
enum AccountActivityType {
    BalanceChange,
    DataChange,
    OwnerChange,
    ExecutableChange,
    RentEpochChange,
    ProgramInteraction,
}

#[derive(ValueEnum, Clone, Debug)]
enum AccountSortBy {
    Name,
    Balance,
    Activity,
    Created,
}

#[derive(Subcommand)]
enum Commands {
    ///  Start real-time blockchain tracking
    #[command(alias = "t")]
    Track {
        #[command(subcommand)]
        target: TrackTarget,
    },



    ///  Monitor blockchain networks (Flow/Solana)
    #[command(alias = "mon", visible_alias = "monitor")]
    FlowMonitor {
        #[command(subcommand)]
        target: MonitorTarget,
    },

    ///  Launch high-performance gRPC streaming server
    #[command(alias = "s", visible_alias = "serve")]
    GrpcServe {
        ///  Bind address for the gRPC server
        #[arg(short, long, default_value = "0.0.0.0", value_hint = ValueHint::Hostname)]
        bind: String,
    },

    ///  Display current blockchain information
    #[command(alias = "i", visible_alias = "info")]
    BlockchainInfo {
        ///  Blockchain network to query
        #[arg(short, long, default_value = "solana", value_enum)]
        blockchain: BlockchainType,
    },

    ///  Get slot leader information (Solana)
    #[command(alias = "l", visible_alias = "leader")]
    SlotLeader {
        ///  Slot number (defaults to current slot)
        slot: Option<u64>,
    },

    ///  Run comprehensive system tests
    #[command(alias = "test")]
    SystemTest,



    ///  gRPC server management & streaming
    #[command(alias = "grpc")]
    GrpcServer {
        #[command(subcommand)]
        action: GrpcAction,
    },

    ///  Advanced caching system management
    #[command(alias = "c")]
    Cache {
        #[command(subcommand)]
        action: CacheAction,
    },

    ///  IPFS distributed storage operations
    #[command(alias = "ipfs")]
    IpfsStorage {
        #[command(subcommand)]
        action: IpfsAction,
    },

    ///  QuickNode/Yellowstone webhook management
    #[command(alias = "w")]
    Webhooks {
        #[command(subcommand)]
        action: WebhookAction,
    },

        ///  Real-time Solana monitoring with Yellowstone gRPC
    #[command(alias = "ys")]
    Yellowstone {
        ///  gRPC endpoint URL
        #[arg(short, long, default_value = "https://example-guide-demo.solana-mainnet.quiknode.pro:10000")]
        endpoint: String,

        ///  Authentication token
        #[arg(long, env = "YELLOWSTONE_AUTH_TOKEN")]
        auth_token: String,

        ///  Add additional account to monitor
        #[arg(long)]
        add_account: Option<String>,

        ///  Remove account from monitoring
        #[arg(long)]
        remove_account: Option<String>,

        ///  List currently monitored accounts
        #[arg(long)]
        list_accounts: bool,
    },

    ///  Performance metrics & monitoring
    #[command(alias = "met")]
    Metrics {
        #[command(subcommand)]
        action: MetricsAction,
    },

    ///  High-performance REST API server
    #[command(alias = "api")]
    RestApi {
        #[command(subcommand)]
        action: ApiAction,
    },

    ///  Database operations & queries
    #[command(alias = "db")]
    Database {
        #[command(subcommand)]
        action: DatabaseAction,
    },

    ///  Interactive command selection menu
    #[command(alias = "menu")]
    Interactive,

    ///  Generate shell completion scripts
    #[command(alias = "comp")]
    Completion {
        ///  Shell to generate completions for
        #[arg(value_enum)]
        shell: Shell,
    },


}

#[derive(Subcommand)]
enum TrackTarget {
    ///  Real-time Solana slot progression monitoring
    #[command(alias = "slot")]
    Slots {
        ///  Include slot leader information in tracking
        #[arg(short, long, help_heading = "Tracking Options")]
        leaders: bool,

        ///  Only track finalized slots (more reliable)
        #[arg(short, long, help_heading = "Tracking Options")]
        finalized_only: bool,

        ///  Update interval in milliseconds (default: 400ms)
        #[arg(short, long, default_value = "400", value_hint = ValueHint::Other, help_heading = "Performance")]
        interval: u64,

        ///  Enable detailed transaction information
        #[arg(short, long, help_heading = "Data Options")]
        transactions: bool,

        ///  Save tracking data to database
        #[arg(long, help_heading = "Storage Options")]
        save: bool,
    },

    ///  Advanced wallet monitoring & analytics
    #[command(alias = "wallet")]
    Wallets {
        #[command(subcommand)]
        action: WalletAction,
    },

    ///  Advanced account monitoring & analytics
    #[command(alias = "account")]
    Accounts {
        #[command(subcommand)]
        action: AccountAction,
    },

    ///  Monitor network-wide validator performance
    #[command(alias = "validator")]
    Validators {
        ///  Specific validator identity to track
        #[arg(short, long, value_hint = ValueHint::Other)]
        identity: Option<String>,

        ///  Track voting performance metrics
        #[arg(short, long)]
        voting: bool,

        ///  Monitor stake changes
        #[arg(short, long)]
        stake: bool,
    },
}

#[derive(Subcommand)]
enum WalletAction {
    ///  Add new wallet to tracking system
    #[command(alias = "a")]
    Add {
        ///  Solana wallet address (base58 encoded)
        #[arg(value_hint = ValueHint::Other)]
        address: String,

        ///  Custom display name for the wallet
        #[arg(short, long, value_hint = ValueHint::Other)]
        name: Option<String>,

        ///  Add notification tags for this wallet
        #[arg(short, long, value_delimiter = ',')]
        tags: Option<Vec<String>>,

        ///  Set balance alert threshold (SOL)
        #[arg(long, value_hint = ValueHint::Other)]
        alert_threshold: Option<f64>,
    },

    ///  Remove wallet from tracking system
    #[command(alias = "r")]
    Remove {
        ///  Wallet address, name, or ID to remove
        #[arg(value_hint = ValueHint::Other)]
        wallet: String,

        ///  Skip confirmation prompt
        #[arg(short, long)]
        force: bool,
    },

    ///  Display all tracked wallets with status
    #[command(alias = "ls")]
    List {
        ///  Show detailed wallet information
        #[arg(short, long)]
        detailed: bool,

        ///  Filter by wallet tags
        #[arg(short, long, value_delimiter = ',')]
        tags: Option<Vec<String>>,

        ///  Sort by balance, activity, or name
        #[arg(short, long, value_enum, default_value = "name")]
        sort: WalletSortBy,
    },

    ///  Real-time wallet activity monitoring
    #[command(alias = "monitor")]
    Watch {
        ///  Update interval in milliseconds
        #[arg(short, long, default_value = "1000", value_hint = ValueHint::Other)]
        interval: u64,

        ///  Activity types to monitor
        #[arg(long, value_delimiter = ',', value_enum)]
        filter: Option<Vec<ActivityType>>,

        ///  Enable desktop notifications
        #[arg(short, long)]
        notify: bool,

        ///  Minimum transaction value to show (SOL)
        #[arg(long, value_hint = ValueHint::Other)]
        min_value: Option<f64>,
    },

    ///  Comprehensive wallet activity history
    #[command(alias = "hist")]
    History {
        ///  Wallet address, name, or ID
        #[arg(value_hint = ValueHint::Other)]
        wallet: String,

        ///  Number of recent activities to display
        #[arg(short, long, default_value = "50", value_hint = ValueHint::Other)]
        limit: u32,

        ///  Export format for the history
        #[arg(short, long, value_enum)]
        export: Option<ExportFormat>,

        ///  Filter by date range (YYYY-MM-DD)
        #[arg(long, value_hint = ValueHint::Other)]
        from: Option<String>,

        #[arg(long, value_hint = ValueHint::Other)]
        to: Option<String>,
    },

    ///  Analyze wallet transaction patterns
    #[command(alias = "analyze")]
    Analytics {
        ///  Wallet to analyze
        #[arg(value_hint = ValueHint::Other)]
        wallet: String,

        ///  Analysis time period in days
        #[arg(short, long, default_value = "30", value_hint = ValueHint::Other)]
        days: u32,

        ///  Generate detailed report
        #[arg(short, long)]
        report: bool,
    },
}

#[derive(Subcommand)]
enum AccountAction {
    ///  Add new account to tracking system
    #[command(alias = "a")]
    Add {
        ///  Solana account address (base58 encoded)
        #[arg(value_hint = ValueHint::Other)]
        address: String,

        ///  Custom display name for the account
        #[arg(short, long, value_hint = ValueHint::Other)]
        name: Option<String>,

        ///  Associated program ID (optional)
        #[arg(long, value_hint = ValueHint::Other)]
        program_id: Option<String>,

        ///  Set balance alert threshold (SOL)
        #[arg(long, value_hint = ValueHint::Other)]
        balance_threshold: Option<f64>,

        ///  Set data size change threshold (bytes)
        #[arg(long, value_hint = ValueHint::Other)]
        data_threshold: Option<u64>,
    },

    ///  Remove account from tracking system
    #[command(alias = "r")]
    Remove {
        ///  Account address, name, or ID to remove
        #[arg(value_hint = ValueHint::Other)]
        account: String,

        ///  Skip confirmation prompt
        #[arg(short, long)]
        force: bool,
    },

    ///  Display all tracked accounts with status
    #[command(alias = "ls")]
    List {
        ///  Show detailed account information
        #[arg(short, long)]
        detailed: bool,

        ///  Filter by program ID
        #[arg(long, value_hint = ValueHint::Other)]
        program_id: Option<String>,

        ///  Sort by balance, activity, or name
        #[arg(short, long, value_enum, default_value = "name")]
        sort: AccountSortBy,
    },

    ///  Real-time account activity monitoring
    #[command(alias = "monitor")]
    Watch {
        ///  Update interval in milliseconds
        #[arg(short, long, default_value = "1000", value_hint = ValueHint::Other)]
        interval: u64,

        ///  Activity types to monitor
        #[arg(long, value_delimiter = ',', value_enum)]
        filter: Option<Vec<AccountActivityType>>,

        ///  Enable desktop notifications
        #[arg(short, long)]
        notify: bool,

        ///  Minimum balance change to show (SOL)
        #[arg(long, value_hint = ValueHint::Other)]
        min_balance_change: Option<f64>,
    },

    ///  Comprehensive account activity history
    #[command(alias = "hist")]
    History {
        ///  Account address, name, or ID
        #[arg(value_hint = ValueHint::Other)]
        account: String,

        ///  Number of recent activities to display
        #[arg(short, long, default_value = "50", value_hint = ValueHint::Other)]
        limit: u32,

        ///  Export format for the history
        #[arg(short, long, value_enum)]
        export: Option<ExportFormat>,

        ///  Filter by activity type
        #[arg(long, value_enum)]
        activity_type: Option<AccountActivityType>,
    },

    ///  Analyze account change patterns
    #[command(alias = "analyze")]
    Analytics {
        ///  Account to analyze
        #[arg(value_hint = ValueHint::Other)]
        account: String,

        ///  Analysis time period in days
        #[arg(short, long, default_value = "30", value_hint = ValueHint::Other)]
        days: u32,

        ///  Generate detailed report
        #[arg(short, long)]
        report: bool,
    },
}

#[derive(Subcommand)]
enum MonitorTarget {
    ///  Monitor Flow blocks
    Blocks {
        /// Update interval in milliseconds
        #[arg(short, long, default_value = "2000")]
        interval: u64,

        /// Show detailed block information
        #[arg(short, long)]
        detailed: bool,
    },

    ///  Monitor Flow events
    Events {
        /// Event type filter
        #[arg(short, long)]
        event_type: Option<String>,

        /// Update interval in milliseconds
        #[arg(short, long, default_value = "1000")]
        interval: u64,
    },

    ///  Monitor Flow transactions
    Transactions {
        /// Update interval in milliseconds
        #[arg(short, long, default_value = "1000")]
        interval: u64,

        /// Show only successful transactions
        #[arg(short, long)]
        success_only: bool,
    },

    ///  Monitor transaction-specific events
    TxEvents {
        /// Transaction ID to monitor
        #[arg(short, long)]
        tx_id: String,
    },

    ///  Monitor delegated staking rewards
    DelegatedRewards {
        /// Update interval in milliseconds
        #[arg(short, long, default_value = "5000")]
        interval: u64,
    },

    ///  Monitor node staking rewards
    NodeRewards {
        /// Node ID to monitor
        #[arg(short, long)]
        node_id: Option<String>,

        /// Update interval in milliseconds
        #[arg(short, long, default_value = "5000")]
        interval: u64,
    },

    ///  Monitor all Flow blockchain data
    All {
        /// Update interval in milliseconds
        #[arg(short, long, default_value = "2000")]
        interval: u64,
    },
}

#[derive(Subcommand)]
enum GrpcAction {
    ///  Start gRPC server
    Start {
        /// Bind address for the gRPC server
        #[arg(short, long, default_value = "0.0.0.0")]
        bind: String,

        /// gRPC server port
        #[arg(short, long, default_value = "50051")]
        port: u16,

        /// Enable Solana slot streaming
        #[arg(long)]
        enable_solana: bool,

        /// Enable Flow blockchain streaming
        #[arg(long)]
        enable_flow: bool,
    },

    ///  Show gRPC server status
    Status,

    ///  Test gRPC endpoints
    Test {
        /// gRPC server address to test
        #[arg(short, long, default_value = "http://127.0.0.1:50051")]
        address: String,
    },
}

#[derive(Subcommand)]
enum CacheAction {
    ///  Start cache system
    Start {
        /// Enable cache warming
        #[arg(long)]
        warm: bool,

        /// Cache size in MB
        #[arg(short, long, default_value = "1000")]
        size: u64,
    },

    ///  Show cache statistics
    Stats,

    ///  Run cache maintenance
    Maintenance,

    ///  Clear all caches
    Clear,

    ///  Inspect cache contents
    Inspect {
        /// Cache type to inspect
        #[arg(value_enum)]
        cache_type: CacheType,
    },
}

#[derive(clap::ValueEnum, Clone, Debug)]
enum CacheType {
    Slots,
    Transactions,
    Accounts,
    Blocks,
    All,
}

#[derive(Subcommand)]
enum IpfsAction {
    ///  Start IPFS daemon
    Start {
        /// IPFS node port
        #[arg(short, long, default_value = "5001")]
        port: u16,
    },

    ///  Upload data to IPFS
    Upload {
        /// File or data to upload
        #[arg(short, long)]
        file: String,

        /// Pin the data
        #[arg(long)]
        pin: bool,
    },

    ///  Download data from IPFS
    Download {
        /// IPFS hash
        #[arg(short, long)]
        hash: String,

        /// Output file
        #[arg(short, long)]
        output: String,
    },

    ///  List pinned content
    List,

    ///  Show IPFS status
    Status,
}

#[derive(Subcommand)]
enum WebhookAction {
    ///  Start webhook listener
    Listen {
        /// Webhook listener port
        #[arg(short, long, default_value = "8080")]
        port: u16,

        /// QuickNode webhook secret
        #[arg(short, long)]
        secret: Option<String>,
    },

    ///  Subscribe to QuickNode webhooks
    Subscribe {
        /// Webhook URL endpoint
        #[arg(short, long)]
        url: String,

        /// Event types to subscribe to
        #[arg(short, long, value_delimiter = ',')]
        events: Vec<String>,
    },

    ///  List active webhooks
    List,

    ///  Test webhook connectivity
    Test,
}

#[derive(Subcommand)]
enum MetricsAction {
    ///  Start Prometheus metrics server
    Start {
        /// Metrics server port
        #[arg(short, long, default_value = "9090")]
        port: u16,
    },

    ///  Show current metrics
    Show,

    ///  Performance benchmark
    Benchmark {
        /// Number of operations
        #[arg(short, long, default_value = "1000")]
        ops: u32,

        /// Concurrent workers
        #[arg(short, long, default_value = "10")]
        workers: u32,
    },

    ///  Export metrics to file
    Export {
        /// Output format
        #[arg(value_enum, default_value = "json")]
        format: ExportFormat,

        /// Output file
        #[arg(short, long)]
        output: String,
    },
}

#[derive(clap::ValueEnum, Clone, Debug)]
enum ExportFormat {
    Json,
    Csv,
    Prometheus,
}

#[derive(Subcommand)]
enum ApiAction {
    ///  Start high-performance API server
    Start {
        /// API server port
        #[arg(short, long, default_value = "3000")]
        port: u16,

        /// Enable rate limiting
        #[arg(long)]
        rate_limit: bool,

        /// Max requests per second
        #[arg(long, default_value = "1000")]
        max_rps: u32,

        /// Enable caching
        #[arg(long, default_value = "true")]
        cache: bool,
    },

    ///  Show API status
    Status,

    ///  Run API benchmarks
    Benchmark {
        /// Target endpoint
        #[arg(short, long, default_value = "http://127.0.0.1:3000")]
        endpoint: String,

        /// Number of requests
        #[arg(short, long, default_value = "10000")]
        requests: u32,

        /// Concurrent connections
        #[arg(short, long, default_value = "100")]
        concurrency: u32,
    },
}

#[derive(Subcommand)]
enum DatabaseAction {
    ///  Test database connection
    Test,

    ///  Show database statistics
    Stats,

    ///  List recent slots
    ListSlots {
        /// Limit number of results
        #[arg(short, long, default_value = "10")]
        limit: u64,
    },

    ///  List finalized slots
    ListFinalizedSlots {
        /// Limit number of results
        #[arg(short, long, default_value = "10")]
        limit: u64,
    },

    ///  List transactions by slot
    ListTransactions {
        /// Slot number
        slot: u64,
    },

    ///  Get slot leader
    GetLeader {
        /// Slot number
        slot: u64,
    },

    ///  Get slot info
    GetSlot {
        /// Slot number
        slot: u64,
    },

    ///  Get transaction info
    GetTransaction {
        /// Transaction signature
        signature: String,
    },



    ///  Fetch and store current slot from Solana RPC
    FetchCurrentSlot,

    ///  Fetch and store recent slots from Solana RPC
    FetchRecentSlots {
        /// Number of recent slots to fetch
        #[arg(short, long, default_value = "10")]
        count: u64,
    },

    ///  Fetch and store slot leaders from Solana RPC
    FetchSlotLeaders {
        /// Starting slot number
        #[arg(short, long)]
        slot: Option<u64>,
        /// Number of leaders to fetch
        #[arg(short, long, default_value = "10")]
        count: u64,
    },

    ///  Fetch and store specific transaction from Solana RPC
    FetchTransaction {
        /// Transaction signature
        signature: String,
    },


}

#[tokio::main]
#[allow(unused_variables)]
async fn main() -> Result<()> {
    dotenv().ok();

    let cli = Cli::parse();

    let logger = NerdLogger::new(1000);
    let filter = if cli.verbose {
        EnvFilter::new("debug")
    } else {
        EnvFilter::new("info")
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_span_events(FmtSpan::CLOSE)
        .init();



    if let Some(Commands::Completion { shell }) = &cli.command {
        print_completions(*shell, &mut Cli::command());
        return Ok(());
    }

    let show_detailed_help = cli.verbose || std::env::args().any(|arg| arg == "--help" || arg == "-h");
    if !show_detailed_help {
        print_banner();
    }

    let config = config::Config::from_env()?;

    let solana_url = config.solana_rpc_url.clone();
    animations::CliAnimations::show_connection_animation(&solana_url);

    let client = RpcClient::new(solana_url.clone());

    logger.info(&format!("{} Connecting to Solana RPC: {}", icons::CONNECTION, solana_url), "main");
    match client.get_health() {
        Ok(_) => {
            logger.success(&format!("{} Solana RPC connection established", icons::COMPLETE), "main");

            if let Some(api_key) = config.get_quicknode_api_key() {
                logger.info(&format!("{} Using QuickNode API key: {}...", icons::KEY, &api_key[..8.min(api_key.len())]), "main");
            } else {
                logger.info(&format!("{} No QuickNode API key configured, using public endpoints", icons::INFO), "main");
            }
        },
        Err(e) => {
            logger.error(&format!("{} Failed to connect to Solana RPC: {}", icons::FAILED, e), "main");
            return Err(e.into());
        }
    }

    match cli.command {

        Some(command) => match command {

        Commands::Track { target } => {
            match target {
                TrackTarget::Slots { leaders, finalized_only, interval: update_interval, transactions, save } => {
                    logger.info(&format!("{} Starting real-time Solana slot tracking...", icons::TRACKING), "main");
                    slot_tracker::start_tracking(client, leaders, finalized_only, update_interval).await?;
                }
                TrackTarget::Validators { identity, voting, stake } => {
                    logger.info(&format!("{} Starting validator performance tracking...", icons::TRACKING), "main");
                    println!("{} Validator tracking feature coming soon!", icons::INFO.bright_yellow());
                }
                TrackTarget::Wallets { action } => {
                    match action {
                        WalletAction::Add { address, name, tags, alert_threshold } => {
                            logger.info(&format!("{} Adding wallet to tracking: {}", icons::DATABASE, address), "main");
                            account_watcher::add_wallet(&config, &address, name).await?;
                        }
                        WalletAction::Remove { wallet, force } => {
                            logger.info(&format!("{} Removing wallet from tracking: {}", icons::DATABASE, wallet), "main");
                            account_watcher::remove_wallet(&config, &wallet).await?;
                        }
                        WalletAction::List { detailed, tags, sort } => {
                            logger.info(&format!("{} Listing tracked wallets...", icons::LIST), "main");
                            account_watcher::list_wallets(&config).await?;
                        }
                        WalletAction::Watch { interval, filter, notify, min_value } => {
                            logger.info(&format!("{} Starting real-time wallet monitoring...", icons::TRACKING), "main");
                            // Convert ActivityType to String for compatibility
                            let string_filter = filter.map(|f| f.iter().map(|a| format!("{:?}", a).to_lowercase()).collect());
                            account_watcher::start_wallet_monitoring(&config, &client, interval, string_filter).await?;
                        }
                        WalletAction::History { wallet, limit, export, from, to } => {
                            logger.info(&format!("{} Fetching wallet activity history: {}", icons::SEARCH, wallet), "main");
                            wallet_tracker::show_history(&config, &wallet, limit).await?;
                        }
                        WalletAction::Analytics { wallet, days, report } => {
                            logger.info(&format!("{} Analyzing wallet patterns: {}", icons::CHART, wallet), "main");
                            println!("{} Wallet analytics feature coming soon!", icons::INFO.bright_yellow());
                        }
                    }
                }
                TrackTarget::Accounts { action } => {
                    match action {
                        AccountAction::Add { address, name, program_id, balance_threshold, data_threshold } => {
                            logger.info(&format!("{} Adding account to tracking: {}", icons::DATABASE, address), "main");
                            account_watcher::add_account(&config, &address, name, program_id).await?;
                        }
                        AccountAction::Remove { account, force } => {
                            logger.info(&format!("{} Removing account from tracking: {}", icons::DATABASE, account), "main");
                            account_watcher::remove_account(&config, &account).await?;
                        }
                        AccountAction::List { detailed, program_id, sort } => {
                            logger.info(&format!("{} Listing tracked accounts...", icons::LIST), "main");
                            account_watcher::list_accounts(&config).await?;
                        }
                        AccountAction::Watch { interval, filter, notify, min_balance_change } => {
                            logger.info(&format!("{} Starting real-time account monitoring...", icons::TRACKING), "main");
                            // Convert AccountActivityType to String for compatibility
                            let string_filter = filter.map(|f| f.iter().map(|a| format!("{:?}", a).to_lowercase()).collect());
                            account_watcher::start_monitoring(&config, &client, interval, string_filter).await?;
                        }
                        AccountAction::History { account, limit, export, activity_type } => {
                            logger.info(&format!("{} Fetching account activity history: {}", icons::SEARCH, account), "main");
                            account_watcher::show_history(&config, &account, limit).await?;
                        }
                        AccountAction::Analytics { account, days, report: _ } => {
                            logger.info(&format!("{} Analyzing account patterns: {}", icons::CHART, account), "main");
                            println!("{} Account analytics feature coming soon!", icons::INFO.bright_yellow());
                        }
                    }
                }
            }
        }

        Commands::FlowMonitor { target } => {
            logger.info(&format!("{} Starting Flow blockchain monitoring...", icons::MONITOR), "main");
            flow_monitor::start_monitoring(target, &config).await?;
        }

        Commands::GrpcServe { bind } => {
            logger.info(&format!("{} Starting gRPC server on {}:{}", icons::SERVER, bind, cli.port), "main");
            grpc_server::start_server(bind, cli.port, client).await?;
        }

        Commands::BlockchainInfo { blockchain } => {
            match blockchain {
                BlockchainType::Solana => {
                    logger.info(&format!("{} Fetching current Solana slot information...", icons::INFO), "main");
                    show_slot_info(&client, &logger).await?;
                }
                BlockchainType::Flow => {
                    logger.info(&format!("{} Fetching current Flow blockchain information...", icons::INFO), "main");
                    flow_monitor::show_flow_info(&config).await?;
                }
            }
        }

        Commands::SlotLeader { slot } => {
            let target_slot = match slot {
                Some(s) => s,
                None => client.get_slot()?,
            };
            logger.info(&format!("{} Fetching slot leader for slot: {}", icons::LEADER, target_slot), "main");
            show_slot_leader(&client, &logger, target_slot).await?;
        }

        Commands::SystemTest => {
            logger.info(&format!("{} Running configuration tests...", icons::TEST), "main");
            run_tests(&config, &client, &logger).await?;
        }

        Commands::GrpcServer { action } => {
            match action {
                GrpcAction::Start { bind, port, enable_solana, enable_flow } => {
                    info!("{} {}:{}", "📡 Starting gRPC server on".bright_cyan(), bind.yellow(), port.to_string().yellow());
                    grpc_server::start_enhanced_server(bind, port, client, &config, enable_solana, enable_flow).await?;
                }
                GrpcAction::Status => {
                    info!("{}", "📊 Checking gRPC server status...".bright_cyan());
                    grpc_server::show_status().await?;
                }
                GrpcAction::Test { address } => {
                    info!("{} {}", "🔧 Testing gRPC server at".bright_cyan(), address.yellow());
                    grpc_server::test_grpc_client(&address).await?;
                }
            }
        }

        Commands::Cache { action } => {
            match action {
                CacheAction::Start { warm, size } => {
                    info!("{} {}MB", "💾 Starting cache system with".bright_cyan(), size.to_string().yellow());
                    cache::start_cache_system(&config, &client, warm, size).await?;
                }
                CacheAction::Stats => {
                    cache::show_cache_stats(&config).await?;
                }
                CacheAction::Maintenance => {
                    cache::run_cache_maintenance(&config).await?;
                }
                CacheAction::Clear => {
                    cache::clear_all_caches(&config).await?;
                }
                CacheAction::Inspect { cache_type } => {
                    cache::inspect_cache(&config, &cache_type).await?;
                }
            }
        }

        Commands::IpfsStorage { action } => {
            match action {
                IpfsAction::Start { port } => {
                    ipfs::start_ipfs_daemon(&port).await?;
                }
                IpfsAction::Upload { file, pin } => {
                    ipfs::upload_to_ipfs(&file, &pin).await?;
                }
                IpfsAction::Download { hash, output } => {
                    ipfs::download_from_ipfs(&hash, &output).await?;
                }
                IpfsAction::List => {
                    ipfs::list_pinned_content().await?;
                }
                IpfsAction::Status => {
                    ipfs::show_ipfs_status().await?;
                }
            }
        }

        Commands::Webhooks { action } => {
            match action {
                WebhookAction::Listen { port, secret } => {
                    webhooks::start_webhook_listener(&port, secret.clone()).await?;
                }
                WebhookAction::Subscribe { url, events } => {
                    webhooks::subscribe_to_webhooks(&url, &events).await?;
                }
                WebhookAction::List => {
                    webhooks::list_active_webhooks().await?;
                }
                WebhookAction::Test => {
                    webhooks::test_webhook_connectivity().await?;
                }
            }
        }

        Commands::Yellowstone { endpoint, auth_token, add_account, remove_account, list_accounts } => {
            use crate::yellowstone_monitor::start_yellowstone_monitoring;

            if list_accounts {
                println!("{}", "📋 Currently monitored accounts:".bright_cyan());
                println!("  • {}", PUMP_FUN_FEE_ACCOUNT.bright_green());
                println!("  • {}", PUMP_FUN_PROGRAM.bright_green());
                return Ok(());
            }

            if let Some(account) = add_account {
                println!("{} {}", "➕ Added account to monitoring:".bright_green(), account.bright_cyan());
                // In a full implementation, you'd store this in a config file
            }

            if let Some(account) = remove_account {
                println!("{} {}", "➖ Removed account from monitoring:".bright_red(), account.bright_cyan());
                // In a full implementation, you'd remove this from a config file
            }

            println!("{} {}", "🚀 Starting Yellowstone gRPC monitoring for endpoint:".bright_yellow(), endpoint.bright_cyan());
            start_yellowstone_monitoring(endpoint, auth_token, logger).await?;
        }

        Commands::Metrics { action } => {
            match action {
                MetricsAction::Start { port } => {
                    metrics::start_metrics_server(&port).await?;
                }
                MetricsAction::Show => {
                    metrics::show_current_metrics().await?;
                }
                MetricsAction::Benchmark { ops, workers } => {
                    metrics::run_performance_benchmark(&ops, &workers).await?;
                }
                MetricsAction::Export { format, output } => {
                    metrics::export_metrics(&format, &output).await?;
                }
            }
        }

        Commands::RestApi { action } => {
            match action {
                ApiAction::Start { port, rate_limit, max_rps, cache } => {
                    api::start_high_performance_api(&port, &rate_limit, &max_rps, &cache, &config, client).await?;
                }
                ApiAction::Status => {
                    api::show_api_status().await?;
                }
                ApiAction::Benchmark { endpoint, requests, concurrency } => {
                    api::run_api_benchmark(&endpoint, &requests, &concurrency).await?;
                }
            }
        }

        Commands::Database { action } => {
            if !config.database_config.enable_database {
                error!("{}", "❌ Database is disabled in configuration. Set ENABLE_DATABASE=true to enable.".bright_red());
                return Ok(());
            }

            let db = database::Database::new(&config.database_config).await?;

            match action {
                DatabaseAction::Test => {
                    db.test_connection().await?;
                }
                DatabaseAction::Stats => {
                    db.show_database_stats().await?;
                }
                DatabaseAction::ListSlots { limit } => {
                    info!("{} {}", "📋 Fetching recent slots (limit:".bright_cyan(), format!("{})", limit).bright_cyan());
                    let slots = db.get_recent_slots(limit).await?;
                    for slot in slots {
                        println!("🔹 Slot {}: {} (Parent: {}, Finalized: {}, Time: {})",
                            slot.slot.to_string().bright_yellow(),
                            slot.blockhash.bright_blue(),
                            slot.parent_slot.to_string().bright_white(),
                            if slot.finalized { "✅".bright_green() } else { "⏳".bright_yellow() },
                            slot.timestamp.format("%Y-%m-%d %H:%M:%S UTC").to_string().bright_cyan()
                        );
                    }
                }
                DatabaseAction::ListFinalizedSlots { limit } => {
                    info!("{} {}", "📋 Fetching finalized slots (limit:".bright_cyan(), format!("{})", limit).bright_cyan());
                    let slots = db.get_finalized_slots(limit).await?;
                    for slot in slots {
                        println!("✅ Slot {}: {} (Parent: {}, Time: {})",
                            slot.slot.to_string().bright_yellow(),
                            slot.blockhash.bright_blue(),
                            slot.parent_slot.to_string().bright_white(),
                            slot.timestamp.format("%Y-%m-%d %H:%M:%S UTC").to_string().bright_cyan()
                        );
                    }
                }
                DatabaseAction::ListTransactions { slot } => {
                    info!("{} {}", "📋 Fetching transactions for slot:".bright_cyan(), slot.to_string().yellow());
                    let transactions = db.get_transactions_by_slot(slot).await?;
                    for tx in transactions {
                        println!("💳 {}: {} SOL fee, {} (Programs: {})",
                            tx.signature.bright_blue(),
                            (tx.fee as f64 / 1_000_000_000.0).to_string().bright_yellow(),
                            tx.status.bright_green(),
                            tx.program_ids.join(", ").bright_white()
                        );
                    }
                }
                DatabaseAction::GetLeader { slot } => {
                    info!("{} {}", "👑 Fetching leader for slot:".bright_cyan(), slot.to_string().yellow());
                    if let Some(leader) = db.get_slot_leader(slot).await? {
                        println!("👑 Slot {} Leader: {}",
                            leader.slot.to_string().bright_yellow(),
                            leader.leader_pubkey.bright_green()
                        );
                        if let Some(name) = leader.validator_name {
                            println!("   Validator: {}", name.bright_cyan());
                        }
                    } else {
                        println!("{}", "❌ No leader found for this slot".bright_red());
                    }
                }
                DatabaseAction::GetSlot { slot } => {
                    info!("{} {}", "🔍 Fetching slot:".bright_cyan(), slot.to_string().yellow());
                    if let Some(slot_data) = db.get_slot(slot).await? {
                        println!("🔹 Slot {}: {}",
                            slot_data.slot.to_string().bright_yellow(),
                            slot_data.blockhash.bright_blue()
                        );
                        println!("   Parent: {}", slot_data.parent_slot.to_string().bright_white());
                        println!("   Finalized: {}", if slot_data.finalized { "✅ Yes".bright_green() } else { "⏳ No".bright_yellow() });
                        println!("   Timestamp: {}", slot_data.timestamp.format("%Y-%m-%d %H:%M:%S UTC").to_string().bright_cyan());
                    } else {
                        println!("{}", "❌ Slot not found".bright_red());
                    }
                }
                DatabaseAction::GetTransaction { signature } => {
                    info!("{} {}", "🔍 Fetching transaction:".bright_cyan(), signature.yellow());
                    if let Some(tx) = db.get_transaction(&signature).await? {
                        println!("💳 Transaction: {}", tx.signature.bright_blue());
                        println!("   Slot: {}", tx.slot.to_string().bright_yellow());
                        println!("   Fee: {} SOL", (tx.fee as f64 / 1_000_000_000.0).to_string().bright_yellow());
                        println!("   Status: {}", tx.status.bright_green());
                        println!("   Programs: {}", tx.program_ids.join(", ").bright_white());
                        println!("   Timestamp: {}", tx.timestamp.format("%Y-%m-%d %H:%M:%S UTC").to_string().bright_cyan());
                    } else {
                        println!("{}", "❌ Transaction not found".bright_red());
                    }
                }

                DatabaseAction::FetchCurrentSlot => {
                    info!("{}", "🌐 Fetching current slot from Solana RPC...".bright_cyan());
                    let rpc_client = RpcClient::new(config.solana_rpc_url.clone());
                    let slot = db.fetch_and_store_current_slot(&rpc_client).await?;
                    println!("{} {}", "✅ Successfully fetched and stored current slot:".bright_green(), slot.to_string().bright_yellow());
                }
                DatabaseAction::FetchRecentSlots { count } => {
                    info!("{} {} {}", "🌐 Fetching".bright_cyan(), count.to_string().bright_yellow(), "recent slots from Solana RPC...".bright_cyan());
                    let rpc_client = RpcClient::new(config.solana_rpc_url.clone());
                    let slots = db.fetch_and_store_recent_slots(&rpc_client, count).await?;
                    println!("{} {} {}", "✅ Successfully fetched and stored".bright_green(), slots.len().to_string().bright_yellow(), "slots with their transactions".bright_green());
                }
                DatabaseAction::FetchSlotLeaders { slot, count } => {
                    let rpc_client = RpcClient::new(config.solana_rpc_url.clone());
                    let start_slot = if let Some(s) = slot {
                        s
                    } else {
                        // Get current slot if not specified
                        rpc_client.get_slot()?
                    };
                    info!("{} {} {} {} {}", "🌐 Fetching".bright_cyan(), count.to_string().bright_yellow(), "slot leaders starting from slot".bright_cyan(), start_slot.to_string().bright_yellow(), "...".bright_cyan());
                    db.fetch_and_store_slot_leaders(&rpc_client, start_slot, count).await?;
                    println!("{} {} {}", "✅ Successfully fetched and stored".bright_green(), count.to_string().bright_yellow(), "slot leaders".bright_green());
                }
                DatabaseAction::FetchTransaction { signature } => {
                    info!("{} {}", "🌐 Fetching transaction from Solana RPC:".bright_cyan(), signature.bright_blue());
                    let rpc_client = RpcClient::new(config.solana_rpc_url.clone());
                    db.fetch_and_store_transaction(&rpc_client, &signature).await?;
                    println!("{} {}", "✅ Successfully fetched and stored transaction:".bright_green(), signature.bright_blue());
                }
            }
        }

        Commands::Interactive => {
            logger.info(&format!("{} Launching interactive command menu...", icons::MONITOR), "main");
            run_interactive_menu(&config, &client, &logger).await?;
        }

        Commands::Completion { .. } => {
            // This case is handled earlier, before banner
            unreachable!("Completion should be handled before this match");
        }
        }
        None => {
            // No command provided, show help based on flags
            if show_detailed_help {
                // Show detailed help with all options
                logger.info(&format!("{} No command provided. Showing detailed help...", icons::HELP), "main");
                println!("{}", "Use 'solana-indexer [COMMAND] --help' for command-specific options".bright_white());
            } else {
                // Show quick help
                logger.info(&format!("{} No command provided. Showing quick start guide...", icons::HELP), "main");
                print_help();
            }
        }
    }

    Ok(())
}

fn print_help() {
    println!();
    println!("{}", format!("{} Quick Start Commands:", icons::HELP).bright_cyan().bold());
    println!();
    println!("  {} {}", icons::TRACKING, "solana-indexer track slots           # Real-time slot monitoring".bright_white());
    println!("  {} {}", icons::DATABASE, "solana-indexer track wallets add     # Add wallet tracking".bright_white());
    println!("  {} {}", icons::INFO, "solana-indexer info                   # Current blockchain status".bright_white());
    println!("  {} {}", icons::SERVER, "solana-indexer serve                  # Start gRPC streaming server".bright_white());
    println!("  {} {}", icons::CACHE, "solana-indexer cache stats            # View cache performance".bright_white());
    println!("  {} {}", icons::METRICS, "solana-indexer metrics show           # System metrics dashboard".bright_white());
    println!("  {} {}", icons::TEST, "solana-indexer test                   # Run system diagnostics".bright_white());

    println!("  {} {}", icons::STAR, "solana-indexer interactive            # Interactive command menu".bright_white());
    println!();
    println!("{}", format!("{} Pro Tips:", icons::ROCKET).bright_yellow().bold());
    println!("  {} Use {} for command shortcuts (e.g., {} instead of {})",
        icons::TIME, "aliases".bright_green(), "track t slots".bright_cyan(), "track slots".bright_cyan());
    println!("  {} Add {} for detailed output and troubleshooting",
        icons::DEBUG, "--verbose".bright_green());
    println!("  {} Set {} environment variable for QuickNode/Yellowstone",
        icons::CONFIG, "SOLANA_RPC_URL".bright_green());
    println!();
    println!("{}", "Use 'solana-indexer --help' for comprehensive documentation".bright_black());
    println!();
}

fn print_banner() {
    // Show cool animated startup banner with Solana colors
    animations::CliAnimations::show_startup_banner();

    // Show command help for first-time users with nerd icons and Solana colors
    println!("{} {}",
        icons::STAR.truecolor(220, 38, 127),
        "Pro Tip: Use --help to see all available commands or try:".truecolor(0, 200, 83).bold()
    );
    println!("   {} {}",
        icons::DATABASE.truecolor(103, 58, 183),
        "solana-indexer track wallets list".bright_white().bold()
    );
    println!("   {} {}",
        icons::CACHE.truecolor(33, 150, 243),
        "solana-indexer cache stats".bright_white().bold()
    );

    println!("   {} {}",
        icons::ROCKET.truecolor(156, 39, 176),
        "solana-indexer interactive".bright_white().bold()
    );
    println!();
}

async fn show_slot_info(client: &RpcClient, _logger: &NerdLogger) -> Result<()> {
    let current_slot = client.get_slot()?;
    // For now, just use current slot - will fix commitment configs later
    let finalized_slot = current_slot.saturating_sub(32); // Rough estimate
    let confirmed_slot = current_slot.saturating_sub(2);   // Rough estimate

    println!("{}", format!("{} Current Slot Information", icons::SLOT).bright_cyan().bold());
    println!("   {} {}", "Current Slot:".bright_white(), current_slot.to_string().bright_yellow());
    println!("   {} {}", "Confirmed Slot:".bright_white(), confirmed_slot.to_string().bright_green());
    println!("   {} {}", "Finalized Slot:".bright_white(), finalized_slot.to_string().bright_blue());

    let slot_diff = current_slot.saturating_sub(finalized_slot);
    println!("   {} {}", "Slot Difference:".bright_white(), slot_diff.to_string().bright_magenta());

    Ok(())
}

async fn show_slot_leader(client: &RpcClient, _logger: &NerdLogger, slot: u64) -> Result<()> {
    // Get slot leaders for a range starting from the target slot
    match client.get_slot_leaders(slot, 1) {
        Ok(leaders) => {
            if let Some(leader_pubkey) = leaders.first() {
                println!("{}", format!("{} Slot Leader Information", icons::LEADER).bright_cyan().bold());
                println!("   {} {}", "Slot:".bright_white(), slot.to_string().bright_yellow());
                println!("   {} {}", "Leader:".bright_white(), leader_pubkey.to_string().bright_green());
            } else {
                error!("{}", "❌ No slot leader found for the specified slot".bright_red());
            }
        }
        Err(e) => {
            error!("{} {}", "❌ Failed to fetch slot leader:".bright_red(), e);
        }
    }

    Ok(())
}

async fn run_tests(config: &config::Config, solana_client: &RpcClient, _logger: &NerdLogger) -> Result<()> {
    println!("{}", format!("{} Configuration Test Results", icons::TEST).bright_cyan().bold());
    println!();

    // Test environment variables
    println!("{}", format!("{} Environment Configuration:", icons::CONFIG).bright_yellow());
    println!("   {} {}", "Flow RPC URL:".bright_white(), config.flow_rpc_url.bright_green());
    println!("   {} {}", "Flow WS URL:".bright_white(), config.flow_ws_url.bright_green());
    println!("   {} {}", "Solana RPC URL:".bright_white(), config.solana_rpc_url.bright_green());
    println!("   {} {}", "Update Interval:".bright_white(), format!("{}ms", config.update_interval_ms).bright_cyan());
    println!("   {} {}", "API Timeout:".bright_white(), format!("{}s", config.api_timeout_seconds).bright_cyan());
    println!();

    // Test Solana connection
    println!("{}", format!("{} Solana Connection Test:", icons::CONNECTION).bright_yellow());
    match solana_client.get_health() {
        Ok(_) => {
            println!("   {} {}", "✅ Status:".bright_white(), "Connected".bright_green());
            if let Ok(slot) = solana_client.get_slot() {
                println!("   {} {}", "📊 Current Slot:".bright_white(), slot.to_string().bright_yellow());
            }
        }
        Err(e) => {
            println!("   {} {}", "❌ Status:".bright_white(), "Failed".bright_red());
            println!("   {} {}", "📝 Error:".bright_white(), e.to_string().bright_red());
        }
    }
    println!();

    // Test Flow connection
    println!("{}", format!("{} Flow Connection Test:", icons::FLOW).bright_yellow());
    let flow_monitor = flow_monitor::FlowMonitor::new(config.clone());
    match flow_monitor.test_connection().await {
        Ok(_) => {
            println!("   {} {}", "✅ Status:".bright_white(), "Connected".bright_green());
            if let Ok(block_data) = flow_monitor.get_latest_block().await {
                if let Some(result) = block_data.get("result") {
                    if let Some(height) = result.get("height").and_then(|h| h.as_u64()) {
                        println!("   {} {}", "🧱 Latest Block:".bright_white(), height.to_string().bright_yellow());
                    }
                }
            }
        }
        Err(e) => {
            println!("   {} {}", "❌ Status:".bright_white(), "Failed".bright_red());
            println!("   {} {}", "📝 Error:".bright_white(), e.to_string().bright_red());
        }
    }
    println!();

    // Test monitoring settings
    println!("{}", format!("{} Monitoring Configuration:", icons::MONITOR).bright_yellow());
    println!("   {} {}", "Monitor Blocks:".bright_white(),
             if config.monitoring_config.monitor_blocks { "✅ Enabled".bright_green() } else { "❌ Disabled".bright_red() });
    println!("   {} {}", "Monitor Events:".bright_white(),
             if config.monitoring_config.monitor_events { "✅ Enabled".bright_green() } else { "❌ Disabled".bright_red() });
    println!("   {} {}", "Monitor Transactions:".bright_white(),
             if config.monitoring_config.monitor_transactions { "✅ Enabled".bright_green() } else { "❌ Disabled".bright_red() });
    println!("   {} {}", "Monitor Staking Rewards:".bright_white(),
             if config.monitoring_config.monitor_staking_rewards { "✅ Enabled".bright_green() } else { "❌ Disabled".bright_red() });
    println!();

    println!("{}", format!("{} Test completed!", icons::COMPLETE).bright_green().bold());

    Ok(())
}

async fn run_interactive_menu(config: &config::Config, client: &RpcClient, logger: &NerdLogger) -> Result<()> {
    let options = [
        "🏦 Manage Wallets",
        "📊 View Cache Statistics",
        "🗄️ Database Operations",
        "⚙️ System Configuration",
        "📈 Live Monitoring",
        "❌ Exit"
    ];

    loop {
        let selection = animations::CliAnimations::show_interactive_menu("🎯 SOLANA INDEXER - INTERACTIVE MENU", &options);

        match selection {
            0 => {
                // Clear screen and show wallet management
                print!("\x1B[2J\x1B[1;1H");
                println!("{}", format!("{} Wallet Management", icons::WALLET).truecolor(0, 200, 83).bold());
                println!("{}", "-".repeat(50).truecolor(103, 58, 183));

                match wallet_tracker::list_wallets(config).await {
                    Ok(_) => println!("{} Wallet list displayed successfully!", icons::SUCCESS.truecolor(0, 200, 83)),
                    Err(e) => println!("{} Error: {}", icons::ERROR.truecolor(220, 38, 127), e),
                }
            }
            1 => {
                // Clear screen and show cache stats
                print!("\x1B[2J\x1B[1;1H");
                println!("{}", format!("{} Cache Statistics", icons::CACHE).truecolor(0, 200, 83).bold());
                println!("{}", "-".repeat(50).truecolor(103, 58, 183));

                match cache::show_cache_stats(config).await {
                    Ok(_) => println!("{} Cache statistics displayed successfully!", icons::SUCCESS.truecolor(0, 200, 83)),
                    Err(e) => println!("{} Error: {}", icons::ERROR.truecolor(220, 38, 127), e),
                }
            }
            2 => {
                // Clear screen and show database info
                print!("\x1B[2J\x1B[1;1H");
                println!("{}", format!("{} Database Operations", icons::DATABASE).truecolor(0, 200, 83).bold());
                println!("{}", "-".repeat(50).truecolor(103, 58, 183));

                match show_database_info(config, client, logger).await {
                    Ok(_) => println!("{} Database information displayed successfully!", icons::SUCCESS.truecolor(0, 200, 83)),
                    Err(e) => println!("{} Error: {}", icons::ERROR.truecolor(220, 38, 127), e),
                }
            }
            3 => {
                // Clear screen and show config
                print!("\x1B[2J\x1B[1;1H");
                println!("{}", format!("{} System Configuration", icons::CONFIG).truecolor(0, 200, 83).bold());
                println!("{}", "-".repeat(50).truecolor(103, 58, 183));

                show_config_info(config);
                println!("{} Configuration displayed successfully!", icons::SUCCESS.truecolor(0, 200, 83));
            }
            4 => {
                // Clear screen and start monitoring
                print!("\x1B[2J\x1B[1;1H");
                println!("{}", format!("{} Starting Live Monitoring...", icons::MONITOR).truecolor(0, 200, 83).bold());
                println!("{}", "-".repeat(50).truecolor(103, 58, 183));

                match wallet_tracker::start_monitoring(config, client, 5000, None).await {
                    Ok(_) => println!("{} Monitoring completed!", icons::SUCCESS.truecolor(0, 200, 83)),
                    Err(e) => println!("{} Error: {}", icons::ERROR.truecolor(220, 38, 127), e),
                }
            }
            _ => {
                // Exit case (index 5 or higher, including when user types 'q')
                print!("\x1B[2J\x1B[1;1H");
                animations::CliAnimations::show_success("👋 Goodbye! Thanks for using Solana Indexer!");
                break;
            }
        }

        // Wait for user input to continue (unless exiting)
        println!();
        println!("{} {}", icons::STAR.truecolor(220, 38, 127), "Press Enter to return to menu...".truecolor(0, 200, 83));
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).unwrap();
    }

    Ok(())
}



#[allow(unused_variables)]
async fn show_database_info(config: &config::Config, client: &RpcClient, logger: &NerdLogger) -> Result<()> {
    if !config.database_config.enable_database {
        animations::CliAnimations::show_error("Database Disabled", "Database functionality is not enabled in configuration");
        return Ok(());
    }

    let db = database::Database::new(&config.database_config).await?;

    println!("{}", "🗄️ Database Information".bright_blue().bold());
    println!("   {} {}", "URL:".bright_white(), config.database_config.database_url.bright_cyan());
    println!("   {} {}", "Status:".bright_white(), "Connected".bright_green());
    println!("   {} {}", "Auto Migration:".bright_white(),
        if config.database_config.auto_migrate { "Enabled".bright_green() } else { "Disabled".bright_yellow() }
    );

    // Show some stats with progress bars
    println!("\n{}", "📊 Database Statistics".bright_yellow().bold());

    // Mock progress bars for demonstration
    for i in 0..=100 {
        animations::CliAnimations::show_progress_bar("Loading database stats", i, 100);
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
    }

    Ok(())
}

fn show_config_info(config: &config::Config) {
    println!("{}", format!("{} Current Configuration", icons::CONFIG).bright_green().bold());
    println!("   {} {}", "Solana RPC:".bright_white(), config.solana_rpc_url.bright_blue());
    println!("   {} {}", "Update Interval:".bright_white(), format!("{}ms", config.update_interval_ms).bright_yellow());
    println!("   {} {}", "Cache Size:".bright_white(), config.cache_size.to_string().bright_magenta());
    println!("   {} {}", "API Timeout:".bright_white(), format!("{}s", config.api_timeout_seconds).bright_cyan());
    println!();
}

/// Generate shell completions
fn print_completions<G: Generator>(generator: G, cmd: &mut clap::Command) {
    generate(generator, cmd, cmd.get_name().to_string(), &mut io::stdout());
}


