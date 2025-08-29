use std::env;
use anyhow::Result;

#[derive(Debug, Clone)]
pub struct Config {
    pub solana_rpc_url: String,
    pub solana_ws_url: String,
    pub flow_rpc_url: String,
    pub flow_ws_url: String,
    pub helius_rpc_url: String,
    pub helius_ws_url: String,
    pub helius_api_key: String,
    pub quicknode_api_key: String,
    pub helius_parsed_tx_url: String,
    pub helius_tx_history_url: String,

    // Webhook configuration (from environment - sensitive)
    pub webhook_base_url: String,
    pub webhook_secrets: WebhookSecrets,

    // Security (from environment - sensitive)
    pub api_key: String,
    pub jwt_secret: String,

    // Performance settings (hardcoded defaults, can be overridden)
    pub api_timeout_seconds: u64,
    pub max_retries: u32,
    pub retry_delay_ms: u64,
    pub log_level: String,
    pub enable_colored_output: bool,
    pub update_interval_ms: u64,
    pub batch_size: usize,
    pub cache_size: usize,

    // Server configuration (defaults)
    pub api_server_port: u16,
    pub grpc_server_port: u16,
    pub webhook_listener_port: u16,
    pub metrics_server_port: u16,
    pub ipfs_api_port: u16,

    // High-performance cache settings
    pub cache_config: CacheConfig,

    // Monitoring configuration
    pub monitoring_config: MonitoringConfig,

    // Helius enhanced features
    pub helius_config: HeliusConfig,

    // Performance tuning
    pub performance_config: PerformanceConfig,

    // SQLx database configuration
    pub database_config: DatabaseConfig,
}

#[derive(Debug, Clone)]
pub struct WebhookSecrets {
    pub quicknode_secret: String,
    pub flow_secret: String,
    pub solana_secret: String,
    pub helius_secret: String,
}

#[derive(Debug, Clone)]
pub struct CacheConfig {
    pub enabled: bool,
    pub total_size_mb: u64,
    pub l1_max_entries: u64,
    pub l1_ttl_seconds: u64,
    pub l1_idle_seconds: u64,
    pub l2_max_entries: u64,
    pub l2_ttl_seconds: u64,
    pub l2_idle_seconds: u64,
    pub l2_max_size_mb: u64,
    pub l3_max_entries: u64,
    pub l3_ttl_seconds: u64,
    pub l3_idle_seconds: u64,
    pub l3_max_size_mb: u64,
    pub l4_max_entries: u64,
    pub l4_ttl_seconds: u64,
    pub l4_max_size_mb: u64,
    pub warming_enabled: bool,
    pub warm_recent_slots: u64,
    pub maintenance_interval_seconds: u64,
}

#[derive(Debug, Clone)]
pub struct MonitoringConfig {
    pub monitor_blocks: bool,
    pub monitor_events: bool,
    pub monitor_transactions: bool,
    pub monitor_staking_rewards: bool,
    pub monitor_flow_blocks: bool,
    pub monitor_flow_events: bool,
    pub monitor_flow_transactions: bool,
    pub monitor_flow_delegated_staking: bool,
    pub monitor_flow_node_staking: bool,
    pub slot_tracking_enabled: bool,
    pub track_slot_leaders: bool,
    pub track_finalized_only: bool,
    pub account_tracking_enabled: bool,
    pub account_update_interval_ms: u64,
    pub max_tracked_accounts: u64,
    pub transaction_monitoring_enabled: bool,
    pub transaction_processing_threads: u32,
}

#[derive(Debug, Clone)]
pub struct HeliusConfig {
    pub base_url: String,
    pub webhooks_url: String,
    pub nft_events_url: String,
    pub balances_url: String,
    pub names_url: String,
    pub commitment: String,
    pub max_supported_transaction_version: u8,
    pub encoding: String,
    pub parse_instructions: bool,
    pub parse_events: bool,
    pub include_failed_txs: bool,
    pub enable_parsed_transactions: bool,
    pub enable_nft_tracking: bool,
    pub enable_defi_tracking: bool,
    pub enable_token_tracking: bool,
    pub enrich_metadata: bool,
    pub resolve_names: bool,
    pub include_balances: bool,
    pub tx_history_limit: u64,
    pub requests_per_second: u32,
    pub burst_limit: u32,
    pub webhook_events: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PerformanceConfig {
    pub worker_threads: u32,
    pub async_runtime_threads: u32,
    pub rpc_connection_pool_size: u32,
    pub http_client_pool_size: u32,
    pub enable_rate_limiting: bool,
    pub max_requests_per_second: u32,
    pub rate_limit_window_seconds: u64,
    pub optimize_for_latency: bool,
    pub flow_block_interval_ms: u64,
    pub flow_event_interval_ms: u64,
    pub flow_transaction_interval_ms: u64,
}

#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    pub database_url: String,
    pub enable_database: bool,
    pub connection_timeout_seconds: u64,
    pub max_connections: u32,
    pub min_connections: u32,
    pub retry_attempts: u32,
    pub auto_migrate: bool,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            total_size_mb: 1000,
            l1_max_entries: 1000,
            l1_ttl_seconds: 30,
            l1_idle_seconds: 10,
            l2_max_entries: 10000,
            l2_ttl_seconds: 300,
            l2_idle_seconds: 60,
            l2_max_size_mb: 50,
            l3_max_entries: 5000,
            l3_ttl_seconds: 600,
            l3_idle_seconds: 120,
            l3_max_size_mb: 100,
            l4_max_entries: 500,
            l4_ttl_seconds: 3600,
            l4_max_size_mb: 500,
            warming_enabled: true,
            warm_recent_slots: 100,
            maintenance_interval_seconds: 30,
        }
    }
}

impl Default for MonitoringConfig {
    fn default() -> Self {
        Self {
            monitor_blocks: true,
            monitor_events: true,
            monitor_transactions: true,
            monitor_staking_rewards: true,
            monitor_flow_blocks: true,
            monitor_flow_events: true,
            monitor_flow_transactions: true,
            monitor_flow_delegated_staking: true,
            monitor_flow_node_staking: true,
            slot_tracking_enabled: true,
            track_slot_leaders: true,
            track_finalized_only: false,
            account_tracking_enabled: true,
            account_update_interval_ms: 1000,
            max_tracked_accounts: 10000,
            transaction_monitoring_enabled: true,
            transaction_processing_threads: 4,
        }
    }
}

impl Default for HeliusConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.helius.xyz/v0".to_string(),
            webhooks_url: "https://api.helius.xyz/v0/webhooks".to_string(),
            nft_events_url: "https://api.helius.xyz/v0/nft-events".to_string(),
            balances_url: "https://api.helius.xyz/v0/addresses/{address}/balances".to_string(),
            names_url: "https://api.helius.xyz/v0/names".to_string(),
            commitment: "confirmed".to_string(),
            max_supported_transaction_version: 0,
            encoding: "json".to_string(),
            parse_instructions: true,
            parse_events: true,
            include_failed_txs: false,
            enable_parsed_transactions: true,
            enable_nft_tracking: true,
            enable_defi_tracking: true,
            enable_token_tracking: true,
            enrich_metadata: true,
            resolve_names: true,
            include_balances: true,
            tx_history_limit: 1000,
            requests_per_second: 100,
            burst_limit: 1000,
            webhook_events: vec![
                "TRANSACTION".to_string(),
                "NFT_SALE".to_string(),
                "STAKING".to_string(),
                "TOKEN_TRANSFER".to_string(),
            ],
        }
    }
}

impl Default for PerformanceConfig {
    fn default() -> Self {
        Self {
            worker_threads: 8,
            async_runtime_threads: 4,
            rpc_connection_pool_size: 20,
            http_client_pool_size: 100,
            enable_rate_limiting: true,
            max_requests_per_second: 1000,
            rate_limit_window_seconds: 60,
            optimize_for_latency: true,
            flow_block_interval_ms: 2000,
            flow_event_interval_ms: 1000,
            flow_transaction_interval_ms: 1500,
        }
    }
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            database_url: "sqlite:./solana_indexer.db".to_string(),
            enable_database: true,
            connection_timeout_seconds: 30,
            max_connections: 10,
            min_connections: 1,
            retry_attempts: 3,
            auto_migrate: true,
        }
    }
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Ok(Config {
            // API endpoints (from environment)
            solana_rpc_url: env::var("QUICK_NODE_URL")
                .unwrap_or_else(|_| env::var("SOLANA_RPC_URL")
                    .unwrap_or_else(|_| "https://api.mainnet-beta.solana.com".to_string())),
            solana_ws_url: env::var("QUICK_NODE_WSS")
                .unwrap_or_else(|_| env::var("SOLANA_WS_URL")
                    .unwrap_or_else(|_| "wss://api.mainnet-beta.solana.com".to_string())),
            flow_rpc_url: env::var("FLOW_RPC_URL")
                .unwrap_or_else(|_| "https://rest-mainnet.onflow.org".to_string()),
            flow_ws_url: env::var("FLOW_WS_URL")
                .unwrap_or_else(|_| "wss://rest-mainnet.onflow.org".to_string()),
            helius_rpc_url: env::var("RPC_URL")
                .unwrap_or_else(|_| "https://mainnet.helius-rpc.com".to_string()),
            helius_ws_url: env::var("WS_URL")
                .unwrap_or_else(|_| "wss://mainnet.helius-rpc.com".to_string()),
            helius_api_key: env::var("HELIUS_API_KEY")
                .unwrap_or_else(|_| "your-helius-api-key".to_string()),
            quicknode_api_key: env::var("QUICK_NODE_API_KEY")
                .unwrap_or_else(|_| "your-quicknode-api-key".to_string()),
            helius_parsed_tx_url: env::var("HELIUS_PARSED_TX_URL")
                .unwrap_or_else(|_| "https://api.helius.xyz/v0/transactions/".to_string()),
            helius_tx_history_url: env::var("HELIUS_TX_HISTORY_URL")
                .unwrap_or_else(|_| "https://api.helius.xyz/v0/addresses/{address}/transactions/".to_string()),

            // Webhook configuration
            webhook_base_url: env::var("WEBHOOK_BASE_URL")
                .unwrap_or_else(|_| "https://your-domain.com/webhooks".to_string()),
            webhook_secrets: WebhookSecrets {
                quicknode_secret: env::var("QUICKNODE_WEBHOOK_SECRET")
                    .unwrap_or_else(|_| "your-quicknode-secret".to_string()),
                flow_secret: env::var("FLOW_WEBHOOK_SECRET")
                    .unwrap_or_else(|_| "your-flow-secret".to_string()),
                solana_secret: env::var("SOLANA_WEBHOOK_SECRET")
                    .unwrap_or_else(|_| "your-solana-secret".to_string()),
                helius_secret: env::var("HELIUS_WEBHOOK_SECRET")
                    .unwrap_or_else(|_| "your-helius-secret".to_string()),
            },

            // Security
            api_key: env::var("API_KEY")
                .unwrap_or_else(|_| "your-secure-api-key".to_string()),
            jwt_secret: env::var("JWT_SECRET")
                .unwrap_or_else(|_| "your-jwt-secret".to_string()),

            // Performance settings (with defaults)
            api_timeout_seconds: env::var("API_TIMEOUT_SECONDS")
                .unwrap_or_else(|_| "30".to_string())
                .parse()
                .unwrap_or(30),
            max_retries: env::var("MAX_RETRIES")
                .unwrap_or_else(|_| "3".to_string())
                .parse()
                .unwrap_or(3),
            retry_delay_ms: env::var("RETRY_DELAY_MS")
                .unwrap_or_else(|_| "1000".to_string())
                .parse()
                .unwrap_or(1000),
            log_level: env::var("LOG_LEVEL")
                .unwrap_or_else(|_| "info".to_string()),
            enable_colored_output: env::var("ENABLE_COLORED_OUTPUT")
                .unwrap_or_else(|_| "true".to_string())
                .parse()
                .unwrap_or(true),
            update_interval_ms: env::var("UPDATE_INTERVAL_MS")
                .unwrap_or_else(|_| "400".to_string())
                .parse()
                .unwrap_or(400),
            batch_size: env::var("BATCH_SIZE")
                .unwrap_or_else(|_| "100".to_string())
                .parse()
                .unwrap_or(100),
            cache_size: env::var("CACHE_SIZE")
                .unwrap_or_else(|_| "1000".to_string())
                .parse()
                .unwrap_or(1000),

            // Server ports (defaults)
            api_server_port: 3000,
            grpc_server_port: 50051,
            webhook_listener_port: 8080,
            metrics_server_port: 9090,
            ipfs_api_port: 5001,

            // Configuration structs with defaults
            cache_config: CacheConfig::default(),
            monitoring_config: MonitoringConfig::default(),
            helius_config: HeliusConfig::default(),
            performance_config: PerformanceConfig::default(),
            database_config: DatabaseConfig {
                database_url: env::var("DATABASE_URL")
                    .unwrap_or_else(|_| "sqlite:./solana_indexer.db".to_string()),
                enable_database: env::var("ENABLE_DATABASE")
                    .unwrap_or_else(|_| "true".to_string())
                    .parse()
                    .unwrap_or(true),
                max_connections: env::var("DB_MAX_CONNECTIONS")
                    .unwrap_or_else(|_| "10".to_string())
                    .parse()
                    .unwrap_or(10),
                auto_migrate: env::var("DB_AUTO_MIGRATE")
                    .unwrap_or_else(|_| "true".to_string())
                    .parse()
                    .unwrap_or(true),
                ..DatabaseConfig::default()
            },
        })
    }

    /// Get the QuickNode API key for authentication
    pub fn get_quicknode_api_key(&self) -> Option<&str> {
        if !self.quicknode_api_key.is_empty() && self.quicknode_api_key != "your-quicknode-api-key" {
            Some(&self.quicknode_api_key)
        } else {
            None
        }
    }
}