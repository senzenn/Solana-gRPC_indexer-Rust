use {
    bs58,
    futures::{sink::SinkExt, stream::StreamExt},
    log::{info, error, warn},
    std::{collections::HashMap, env},
    tokio,
    tonic::{
        transport::ClientTlsConfig,
        service::Interceptor,
        Status,
    },
    yellowstone_grpc_client::GeyserGrpcClient,
    yellowstone_grpc_proto::{
        geyser::SubscribeUpdate,
        prelude::{
            subscribe_update::UpdateOneof,
            CommitmentLevel,
            SubscribeRequest,
            SubscribeRequestFilterTransactions,
        },
    },
    anyhow::Result,
    colored::*,
    crate::logger::NerdLogger,
};

// Constants
const RUST_LOG_LEVEL: &str = "info";
const PUMP_FUN_FEE_ACCOUNT: &str = "CebN5WGQ4jvEPvsVU4EoHEpgzq1VV7AbicfhtW4xC9iM";
const PUMP_FUN_PROGRAM: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";

pub struct YellowstoneMonitor {
    endpoint: String,
    auth_token: String,
    logger: NerdLogger,
    accounts_to_monitor: Vec<String>,
}

impl YellowstoneMonitor {
    pub fn new(endpoint: String, auth_token: String, logger: NerdLogger) -> Self {
        Self {
            endpoint,
            auth_token,
            logger,
            accounts_to_monitor: vec![
                PUMP_FUN_FEE_ACCOUNT.to_string(),
                PUMP_FUN_PROGRAM.to_string(),
            ],
        }
    }

    pub fn add_account(&mut self, account: String) {
        if !self.accounts_to_monitor.contains(&account) {
            self.accounts_to_monitor.push(account);
        }
    }

    pub fn remove_account(&mut self, account: &str) {
        self.accounts_to_monitor.retain(|acc| acc != account);
    }

    pub fn list_monitored_accounts(&self) -> &Vec<String> {
        &self.accounts_to_monitor
    }

    /// Start monitoring with beautiful terminal output
    pub async fn start_monitoring(&self) -> Result<()> {
        self.setup_logging();

        let content_width = 120;
        let border_line = format!("┌─{}─┐", "─".repeat(content_width - 2));
        println!("{}", border_line.truecolor(80, 250, 123));

        let title = "YELLOWSTONE gRPC MONITOR";
        let title_padding = content_width - title.len() - 2;
        println!("{} {} {} {}",
            "│".truecolor(80, 250, 123),
            title.truecolor(80, 250, 123).bold(),
            " ".repeat(title_padding),
            "│".truecolor(80, 250, 123)
        );

        let separator = format!("├─{}─┤", "─".repeat(content_width - 2));
        println!("{}", separator.truecolor(80, 250, 123));

        let endpoint_info = format!("Endpoint: {}", self.endpoint);
        let endpoint_padding = content_width - endpoint_info.len() - 2;
        println!("{} {} {} {}",
            "│".truecolor(80, 250, 123),
            endpoint_info.truecolor(139, 233, 253),
            " ".repeat(endpoint_padding),
            "│".truecolor(80, 250, 123)
        );

        let accounts_info = format!("Monitoring {} accounts", self.accounts_to_monitor.len());
        let accounts_padding = content_width - accounts_info.len() - 2;
        println!("{} {} {} {}",
            "│".truecolor(80, 250, 123),
            accounts_info.truecolor(255, 184, 108),
            " ".repeat(accounts_padding),
            "│".truecolor(80, 250, 123)
        );

        for account in &self.accounts_to_monitor {
            let account_info = format!("  • {}", account);
            let account_padding = content_width - account_info.len() - 2;
            println!("{} {} {} {}",
                "│".truecolor(80, 250, 123),
                account_info.truecolor(139, 233, 253),
                " ".repeat(account_padding),
                "│".truecolor(80, 250, 123)
            );
        }

        let bottom_border = format!("└─{}─┘", "─".repeat(content_width - 2));
        println!("{}", bottom_border.truecolor(80, 250, 123));

        info!("Starting to monitor {} accounts", self.accounts_to_monitor.len());
        self.logger.info(&format!("Starting Yellowstone gRPC monitoring for {} accounts", self.accounts_to_monitor.len()), "YELLOWSTONE");

        let mut client = self.setup_client().await?;
        info!("Connected to gRPC endpoint");
        self.logger.info("Connected to Yellowstone gRPC endpoint", "YELLOWSTONE");

        let (subscribe_tx, subscribe_rx) = client.subscribe().await?;

        self.send_subscription_request(subscribe_tx).await?;
        info!("Subscription request sent. Listening for updates...");
        self.logger.info("Subscription request sent. Listening for updates...", "YELLOWSTONE");

        self.process_updates(subscribe_rx).await?;

        info!("Stream closed");
        self.logger.info("Yellowstone gRPC stream closed", "YELLOWSTONE");
        Ok(())
    }

    /// Initialize the logging system
    fn setup_logging(&self) {
        unsafe {
            env::set_var("RUST_LOG", RUST_LOG_LEVEL);
        }
        env_logger::init();
    }

    /// Create and connect to the gRPC client
    async fn setup_client(&self) -> Result<GeyserGrpcClient<impl Interceptor>> {
        info!("Connecting to gRPC endpoint: {}", self.endpoint);
        self.logger.info(&format!("Connecting to gRPC endpoint: {}", self.endpoint), "YELLOWSTONE");

        // Build the gRPC client with TLS config
        let client = GeyserGrpcClient::build_from_shared(self.endpoint.clone())?
            .x_token(Some(self.auth_token.clone()))?
            .tls_config(ClientTlsConfig::new().with_native_roots())?
            .connect()
            .await?;

        Ok(client)
    }

        /// Send the subscription request with transaction filters
    async fn send_subscription_request<T>(
        &self,
        mut tx: T,
    ) -> Result<()>
    where
        T: SinkExt<SubscribeRequest> + Unpin,
        <T as futures::Sink<SubscribeRequest>>::Error: std::error::Error + 'static + Send + Sync,
    {
        // Create account filter with the target accounts
        let mut accounts_filter = HashMap::new();
        accounts_filter.insert(
            "account_monitor".to_string(),
            SubscribeRequestFilterTransactions {
                account_include: vec![],
                account_exclude: vec![],
                account_required: self.accounts_to_monitor.clone(),
                vote: Some(false),
                failed: Some(false),
                signature: None,
            },
        );

        // Send subscription request
        tx.send(SubscribeRequest {
            transactions: accounts_filter,
            commitment: Some(CommitmentLevel::Processed as i32),
            ..Default::default()
        }).await?;

        Ok(())
    }

    /// Process updates from the stream with beautiful formatting
    async fn process_updates<S>(
        &self,
        mut stream: S,
    ) -> Result<()>
    where
        S: StreamExt<Item = Result<SubscribeUpdate, Status>> + Unpin,
    {
        let mut transaction_count = 0;

        while let Some(message) = stream.next().await {
            match message {
                Ok(msg) => {
                    transaction_count += 1;
                    self.handle_message(msg, transaction_count);
                },
                Err(e) => {
                    error!("Error receiving message: {:?}", e);
                    self.logger.error(&format!("Error receiving message: {:?}", e), "YELLOWSTONE");
                    break;
                }
            }
        }

        Ok(())
    }

    /// Handle an individual message from the stream with beautiful formatting
    fn handle_message(&self, msg: SubscribeUpdate, count: u64) {
        match msg.update_oneof {
            Some(UpdateOneof::Transaction(transaction_update)) => {
                if let Some(tx_info) = &transaction_update.transaction {
                    let signature = &tx_info.signature;
                    let tx_id = bs58::encode(signature).into_string();

                    // Display transaction with terminal borders
                    let content_width = 120;
                    let border_line = format!("┌─{}─┐", "─".repeat(content_width - 2));
                    println!("{}", border_line.truecolor(189, 147, 249));

                    let title = format!("TRANSACTION #{}", count);
                    let title_padding = content_width - title.len() - 2;
                    println!("{} {} {} {}",
                        "│".truecolor(189, 147, 249),
                        title.truecolor(189, 147, 249).bold(),
                        " ".repeat(title_padding),
                        "│".truecolor(189, 147, 249)
                    );

                    let separator = format!("├─{}─┤", "─".repeat(content_width - 2));
                    println!("{}", separator.truecolor(189, 147, 249));

                    let signature_info = format!("Signature: {}", tx_id);
                    let signature_padding = content_width - signature_info.len() - 2;
                    println!("{} {} {} {}",
                        "│".truecolor(189, 147, 249),
                        signature_info.truecolor(80, 250, 123).bold(),
                        " ".repeat(signature_padding),
                        "│".truecolor(189, 147, 249)
                    );

                    let slot_info = format!("Slot: {}", transaction_update.slot);
                    let slot_padding = content_width - slot_info.len() - 2;
                    println!("{} {} {} {}",
                        "│".truecolor(189, 147, 249),
                        slot_info.truecolor(248, 248, 242).bold(),
                        " ".repeat(slot_padding),
                        "│".truecolor(189, 147, 249)
                    );

                    let bottom_border = format!("└─{}─┘", "─".repeat(content_width - 2));
                    println!("{}", bottom_border.truecolor(189, 147, 249));

                    info!("Transaction update received! ID: {}", tx_id);
                    self.logger.info(&format!("Transaction update received! ID: {}", tx_id), "YELLOWSTONE");
                } else {
                    warn!("Transaction update received but no transaction info available");
                    self.logger.warn("Transaction update received but no transaction info available", "YELLOWSTONE");
                }
            },
            Some(other) => {
                info!("Other update received: {:?}", other);
                self.logger.info(&format!("Other update received: {:?}", other), "YELLOWSTONE");
            },
            None => {
                warn!("Empty update received");
                self.logger.warn("Empty update received", "YELLOWSTONE");
            }
        }
    }
}

/// Quick start function for easy CLI integration
pub async fn start_yellowstone_monitoring(
    endpoint: String,
    auth_token: String,
    logger: NerdLogger,
) -> Result<()> {
    let monitor = YellowstoneMonitor::new(endpoint, auth_token, logger);
    monitor.start_monitoring().await
}
