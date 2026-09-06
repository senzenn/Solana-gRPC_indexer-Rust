use anyhow::Result;
use clap::{
    builder::styling::{AnsiColor, Effects, Styles},
    CommandFactory, Parser, Subcommand,
};
use clap_complete::{generate, Generator, Shell};
use dotenvy::dotenv;
use std::io;
use tracing::info;
use tracing_subscriber::{fmt::format::FmtSpan, EnvFilter};

mod pipeline;

fn get_styles() -> Styles {
    Styles::styled()
        .header(AnsiColor::Yellow.on_default() | Effects::BOLD)
        .usage(AnsiColor::Cyan.on_default() | Effects::BOLD)
        .literal(AnsiColor::Blue.on_default() | Effects::BOLD)
        .placeholder(AnsiColor::Green.on_default())
}

#[derive(Parser)]
#[command(
    name = "index",
    about = "Typed-event Solana indexer: geyser ingest, IDL decode, SQLite store, live HTTP/WS",
    version = "0.2.0",
    long_about = "Solana typed-event indexer\n\nPrimary commands:\n  run      Yellowstone geyser -> decode -> SQLite -> HTTP/WebSocket live view\n  parser   IDL registry (idl-drift gated)\n  config   Validate index.yaml\n  demo     Serve stored events (read-only)\n\nSee README.md for docker compose quick start.",
    styles = get_styles(),
    help_template = "\n{name} {version}\n{about}\n\n{usage-heading} {usage}\n\n{all-args}"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Typed-event indexer: geyser -> decode -> sqlite -> websocket
    Run {
        /// Load settings from YAML (CLI flags override config values)
        #[arg(long, value_name = "FILE")]
        config: Option<std::path::PathBuf>,
        /// Yellowstone / Dragon's Mouth gRPC endpoint
        #[arg(short, long, env = "YELLOWSTONE_ENDPOINT")]
        endpoint: Option<String>,
        /// x-token for the geyser endpoint
        #[arg(long, env = "YELLOWSTONE_AUTH_TOKEN", default_value = "")]
        auth_token: String,
        /// SQLite url or path
        #[arg(long, env = "DATABASE_URL", default_value = "sqlite:./index.db")]
        db: String,
        /// Directory of program IDL JSON files
        #[arg(long, default_value = "./idls")]
        idl_dir: std::path::PathBuf,
        /// HTTP/WebSocket bind address
        #[arg(long, default_value = "0.0.0.0:8080")]
        bind: String,
        /// Wallet addresses to include (optional filter)
        #[arg(long = "wallet")]
        wallets: Vec<String>,
        /// Terminal live view instead of just HTTP
        #[arg(long)]
        tui: bool,
        /// Optional gRPC export bind address (e.g. 0.0.0.0:50051)
        #[arg(long)]
        grpc_bind: Option<String>,
    },

    /// IDL parser registry (idl-drift gated)
    Parser {
        #[command(subcommand)]
        action: ParserCmd,
    },

    /// Configuration file utilities
    Config {
        #[command(subcommand)]
        action: ConfigCmd,
    },

    /// Serve stored events only (no geyser) — demo / read-only mode
    Demo {
        #[arg(long, env = "DATABASE_URL", default_value = "sqlite:./index.db")]
        db: String,
        #[arg(long, default_value = "0.0.0.0:8080")]
        bind: String,
    },

    /// Generate shell completion scripts
    #[command(alias = "comp")]
    Completion {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: Shell,
    },
}

#[derive(Subcommand)]
enum ParserCmd {
    /// Copy an IDL into ./idls after an idl-drift safety gate
    New {
        #[arg(long)]
        idl: std::path::PathBuf,
        #[arg(long, default_value = "./idls")]
        idl_dir: std::path::PathBuf,
        /// Allow Breaking diffs and unmapped Generic fields
        #[arg(long)]
        force: bool,
    },
    /// List loaded program addresses
    List {
        #[arg(long, default_value = "./idls")]
        idl_dir: std::path::PathBuf,
    },
    /// Diff two IDL files (exit 1 on Breaking)
    Diff {
        old: std::path::PathBuf,
        new: std::path::PathBuf,
    },
    /// Fetch Anchor IDL from chain and install locally
    Fetch {
        #[arg(long)]
        program_id: String,
        #[arg(long, env = "SOLANA_RPC_URL", default_value = "https://api.mainnet-beta.solana.com")]
        rpc_url: String,
        #[arg(long, default_value = "./idls")]
        idl_dir: std::path::PathBuf,
        #[arg(long)]
        force: bool,
    },
    /// Poll on-chain IDLs and update local copies on drift
    Watch {
        #[arg(long, default_value = "./idls")]
        idl_dir: std::path::PathBuf,
        #[arg(long, env = "SOLANA_RPC_URL", default_value = "https://api.mainnet-beta.solana.com")]
        rpc_url: String,
        #[arg(long, default_value = "300")]
        interval: u64,
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Validate an index.yaml configuration file
    Validate {
        /// Path to the YAML config file
        #[arg(long, default_value = "index.yaml")]
        file: std::path::PathBuf,
    },
}

struct ResolvedRunArgs {
    endpoint: String,
    auth_token: String,
    db: String,
    idl_dir: std::path::PathBuf,
    bind: String,
    wallets: Vec<String>,
    rpc_url: Option<String>,
    sinks: Vec<pipeline::config_file::SinkConfig>,
    api_keys: Vec<String>,
    grpc_bind: Option<std::net::SocketAddr>,
    track_failed: bool,
    track_tokens: bool,
    accounts_include: Vec<String>,
}

struct RunCliOverrides {
    config_path: Option<std::path::PathBuf>,
    endpoint: Option<String>,
    auth_token: String,
    db: String,
    idl_dir: std::path::PathBuf,
    bind: String,
    wallets: Vec<String>,
    grpc_bind: Option<String>,
}

fn resolve_run_args(overrides: RunCliOverrides) -> Result<ResolvedRunArgs> {
    let RunCliOverrides {
        config_path,
        endpoint,
        auth_token,
        db,
        idl_dir,
        bind,
        wallets,
        grpc_bind,
    } = overrides;
    let config_path = config_path.as_deref();
    let cfg = config_path
        .map(pipeline::config_file::load)
        .transpose()?
        .map(pipeline::config_file::merge_with_env);

    let endpoint = endpoint
        .or_else(|| cfg.as_ref().map(|c| c.geyser.endpoint.clone()))
        .filter(|s| !s.trim().is_empty() && !s.contains("${"))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "geyser endpoint required: pass --endpoint, set YELLOWSTONE_ENDPOINT, or use --config"
            )
        })?;

    let auth_token = if auth_token.is_empty() {
        cfg.as_ref()
            .map(|c| c.geyser.auth_token.clone())
            .unwrap_or_default()
    } else {
        auth_token
    };

    let db = if db == "sqlite:./index.db" {
        cfg.as_ref()
            .map(|c| c.store.url.clone())
            .unwrap_or(db)
    } else {
        db
    };

    let bind = if bind == "0.0.0.0:8080" {
        cfg.as_ref()
            .map(|c| c.serve.bind.clone())
            .unwrap_or(bind)
    } else {
        bind
    };

    let wallets = if wallets.is_empty() {
        cfg.as_ref()
            .map(|c| c.wallets.clone())
            .unwrap_or_default()
    } else {
        wallets
    };

    let idl_dir = if idl_dir == std::path::Path::new("./idls") {
        cfg.as_ref()
            .and_then(|c| c.programs.first())
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or(idl_dir)
    } else {
        idl_dir
    };

    bind.parse::<std::net::SocketAddr>()
        .map_err(|e| anyhow::anyhow!("serve.bind is not a valid socket address: {bind}: {e}"))?;

    let mut rpc_url = cfg.as_ref().and_then(|c| c.rpc_url.clone());
    if rpc_url.is_none()
        && let Ok(rpc) = std::env::var("SOLANA_RPC_URL")
            && !rpc.is_empty() {
                rpc_url = Some(rpc);
            }

    let sinks = cfg.as_ref().map(|c| c.sinks.clone()).unwrap_or_default();

    let mut api_keys = cfg
        .as_ref()
        .and_then(|c| c.auth.as_ref())
        .map(|a| a.api_keys.clone())
        .unwrap_or_default();
    if api_keys.is_empty()
        && let Ok(keys) = std::env::var("INDEX_API_KEYS") {
            api_keys = keys
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }

    let grpc_bind = grpc_bind
        .or_else(|| {
            cfg.as_ref()
                .and_then(|c| c.grpc.as_ref())
                .map(|g| g.bind.clone())
        })
        .and_then(|s| s.parse::<std::net::SocketAddr>().ok());

    let track_failed = cfg.as_ref().map(|c| c.geyser.track_failed).unwrap_or(false);
    let track_tokens = cfg.as_ref().map(|c| c.geyser.track_tokens).unwrap_or(false);
    let accounts_include = cfg
        .as_ref()
        .map(|c| c.geyser.accounts_include.clone())
        .unwrap_or_default();

    Ok(ResolvedRunArgs {
        endpoint,
        auth_token,
        db,
        idl_dir,
        bind,
        wallets,
        rpc_url,
        sinks,
        api_keys,
        grpc_bind,
        track_failed,
        track_tokens,
        accounts_include,
    })
}

fn print_completions<G: Generator>(generator: G, cmd: &mut clap::Command) {
    generate(generator, cmd, cmd.get_name().to_string(), &mut io::stdout());
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();

    let cli = Cli::parse();

    let filter = if cli.verbose {
        EnvFilter::new("debug")
    } else {
        EnvFilter::new("info")
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_span_events(FmtSpan::CLOSE)
        .init();

    match cli.command {
        Commands::Completion { shell } => {
            print_completions(shell, &mut Cli::command());
            Ok(())
        }
        Commands::Config { action } => match action {
            ConfigCmd::Validate { file } => {
                let cfg = pipeline::config_file::load(&file)?;
                let cfg = pipeline::config_file::merge_with_env(cfg);
                cfg.validate()?;
                info!(file = %file.display(), "config valid");
                Ok(())
            }
        },
        Commands::Run {
            config,
            endpoint,
            auth_token,
            db,
            idl_dir,
            bind,
            wallets,
            tui,
            grpc_bind,
        } => {
            let resolved = resolve_run_args(RunCliOverrides {
                config_path: config,
                endpoint,
                auth_token,
                db,
                idl_dir,
                bind,
                wallets,
                grpc_bind,
            })?;
            let ws_bind: std::net::SocketAddr = resolved.bind.parse()?;
            pipeline::run::run(pipeline::run::RunArgs {
                endpoint: resolved.endpoint,
                auth_token: resolved.auth_token,
                db: resolved.db,
                idl_dir: resolved.idl_dir,
                ws_bind,
                wallets: resolved.wallets,
                tui,
                rpc_url: resolved.rpc_url,
                sinks: resolved.sinks,
                api_keys: resolved.api_keys,
                grpc_bind: resolved.grpc_bind,
                track_failed: resolved.track_failed,
                track_tokens: resolved.track_tokens,
                accounts_include: resolved.accounts_include,
            })
            .await
        }
        Commands::Demo { db, bind } => {
            let bind: std::net::SocketAddr = bind
                .parse()
                .map_err(|e| anyhow::anyhow!("invalid --bind {bind}: {e}"))?;
            let api_keys = std::env::var("INDEX_API_KEYS")
                .ok()
                .map(|k| k.split(',').map(|s| s.trim().to_string()).collect())
                .unwrap_or_default();
            pipeline::demo::run_demo(db, bind, api_keys).await
        }
        Commands::Parser { action } => match action {
            ParserCmd::New { idl, idl_dir, force } => {
                pipeline::parser_cmd::add_idl(&idl_dir, &idl, force)
            }
            ParserCmd::List { idl_dir } => pipeline::parser_cmd::list_idls(&idl_dir),
            ParserCmd::Diff { old, new } => pipeline::parser_cmd::diff_idls(&old, &new),
            ParserCmd::Fetch {
                program_id,
                rpc_url,
                idl_dir,
                force,
            } => pipeline::parser_cmd::fetch_idl(&rpc_url, &program_id, &idl_dir, force).await,
            ParserCmd::Watch {
                idl_dir,
                rpc_url,
                interval,
                force,
            } => pipeline::parser_cmd::watch_idls(&rpc_url, &idl_dir, interval, force).await,
        },
    }
}
