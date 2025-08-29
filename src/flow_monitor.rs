use anyhow::Result;
use colored::*;
use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::time::{interval, sleep};
use tracing::{info, error, debug};
use uuid::Uuid;

use crate::{config::Config, MonitorTarget};

#[derive(Debug)]
pub struct FlowMonitor {
    client: Client,
    config: Config,
}

impl FlowMonitor {
    pub fn new(config: Config) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.api_timeout_seconds))
            .build()
            .expect("Failed to create HTTP client");

        Self { client, config }
    }

    pub async fn test_connection(&self) -> Result<()> {
        info!("{} {}", "🔗 Testing Flow API connection:".bright_blue(), self.config.flow_rpc_url.yellow());

        // Test basic connectivity with a simple query
        let response = self.client
            .post(&self.config.flow_rpc_url)
            .json(&json!({
                "jsonrpc": "2.0",
                "method": "flow_getLatestBlock",
                "params": [true],
                "id": Uuid::new_v4().to_string()
            }))
            .send()
            .await?;

        if response.status().is_success() {
            info!("{}", "✅ Flow API connection established".bright_green());
            Ok(())
        } else {
            error!("{} {}", "❌ Flow API connection failed:".bright_red(), response.status());
            Err(anyhow::anyhow!("Flow API connection failed"))
        }
    }

    pub async fn get_latest_block(&self) -> Result<Value> {
        let response = self.client
            .post(&self.config.flow_rpc_url)
            .json(&json!({
                "jsonrpc": "2.0",
                "method": "flow_getLatestBlock",
                "params": [true],
                "id": Uuid::new_v4().to_string()
            }))
            .send()
            .await?;

        let result: Value = response.json().await?;
        Ok(result)
    }

    pub async fn get_events(&self, event_type: Option<&str>) -> Result<Value> {
        let method = match event_type {
            Some(_event_type) => "flow_getEventsForHeightRange".to_string(),
            None => "flow_getLatestBlock".to_string(),
        };

        let response = self.client
            .post(&self.config.flow_rpc_url)
            .json(&json!({
                "jsonrpc": "2.0",
                "method": method,
                "params": [],
                "id": Uuid::new_v4().to_string()
            }))
            .send()
            .await?;

        let result: Value = response.json().await?;
        Ok(result)
    }

    pub async fn monitor_blocks(&self, interval_ms: u64, detailed: bool) -> Result<()> {
        let mut interval = interval(Duration::from_millis(interval_ms));
        let mut last_height = None;

        info!("{}", "🧱 Starting Flow block monitoring...".bright_cyan());

        loop {
            interval.tick().await;

            match self.get_latest_block().await {
                Ok(block_data) => {
                    if let Some(result) = block_data.get("result") {
                        if let Some(height) = result.get("height").and_then(|h| h.as_u64()) {
                            if last_height != Some(height) {
                                self.print_block_update(height, result, detailed);
                                last_height = Some(height);
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("{} {}", "❌ Error fetching block:".bright_red(), e);
                    sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }

    pub async fn monitor_events(&self, event_type: Option<&str>, interval_ms: u64) -> Result<()> {
        let mut interval = interval(Duration::from_millis(interval_ms));

        info!("{}", "📡 Starting Flow events monitoring...".bright_cyan());
        if let Some(event_type) = event_type {
            info!("{} {}", "🎯 Filtering for event type:".bright_white(), event_type.bright_yellow());
        }

        loop {
            interval.tick().await;

            match self.get_events(event_type).await {
                Ok(events_data) => {
                    self.print_events_update(&events_data);
                }
                Err(e) => {
                    error!("{} {}", "❌ Error fetching events:".bright_red(), e);
                    sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }

    pub async fn monitor_all(&self, interval_ms: u64) -> Result<()> {
        let mut interval = interval(Duration::from_millis(interval_ms));

        info!("{}", "📊 Starting comprehensive Flow monitoring...".bright_cyan());

        loop {
            interval.tick().await;

            // Monitor blocks
            if let Ok(block_data) = self.get_latest_block().await {
                if let Some(result) = block_data.get("result") {
                    if let Some(height) = result.get("height").and_then(|h| h.as_u64()) {
                        self.print_block_update(height, result, false);
                    }
                }
            }

            // Add small delay between requests
            sleep(Duration::from_millis(100)).await;
        }
    }

    fn print_block_update(&self, height: u64, block_data: &Value, detailed: bool) {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        if detailed {
            if let Some(id) = block_data.get("id").and_then(|id| id.as_str()) {
                println!(
                    "{} {} | {} {} | {} {} | {} {}",
                    "🧱 Block:".bright_cyan(),
                    height.to_string().bright_yellow(),
                    "📋 ID:".bright_white(),
                    format!("{}...", &id[..8]).bright_green(),
                    "⏰ Time:".bright_white(),
                    timestamp.to_string().bright_cyan(),
                    "🔗 Chain:".bright_white(),
                    "Flow".bright_magenta()
                );
            }
        } else {
            println!(
                "{} {} | {} {} | {} {}",
                "🧱 Block:".bright_cyan(),
                height.to_string().bright_yellow(),
                "⏰ Time:".bright_white(),
                timestamp.to_string().bright_cyan(),
                "🔗 Chain:".bright_white(),
                "Flow".bright_magenta()
            );
        }
    }

    fn print_events_update(&self, events_data: &Value) {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        println!(
            "{} {} | {} {}",
            "📡 Events Update:".bright_purple(),
            "Flow".bright_magenta(),
            "⏰ Time:".bright_white(),
            timestamp.to_string().bright_cyan()
        );

        // Log the events data for debugging
        debug!("Events data: {}", events_data);
    }
}

pub async fn start_monitoring(target: MonitorTarget, config: &Config) -> Result<()> {
    let monitor = FlowMonitor::new(config.clone());

    // Test connection first
    monitor.test_connection().await?;

    match target {
        MonitorTarget::Blocks { interval, detailed } => {
            monitor.monitor_blocks(interval, detailed).await?;
        }

        MonitorTarget::Events { event_type, interval } => {
            monitor.monitor_events(event_type.as_deref(), interval).await?;
        }

        MonitorTarget::Transactions { interval, success_only } => {
            info!("{}", "💰 Transaction monitoring not yet implemented".bright_yellow());
            info!("{} {}", "⚠️  Success only:".bright_white(), success_only.to_string().bright_cyan());
            info!("{} {}ms", "🔄 Would update every:".bright_white(), interval.to_string().bright_cyan());
        }

        MonitorTarget::TxEvents { tx_id } => {
            info!("{}", "🎯 Transaction-specific event monitoring not yet implemented".bright_yellow());
            info!("{} {}", "🆔 Transaction ID:".bright_white(), tx_id.bright_cyan());
        }

        MonitorTarget::DelegatedRewards { interval } => {
            info!("{}", "🏆 Delegated staking rewards monitoring not yet implemented".bright_yellow());
            info!("{} {}ms", "🔄 Would update every:".bright_white(), interval.to_string().bright_cyan());
        }

        MonitorTarget::NodeRewards { node_id, interval } => {
            info!("{}", "🖥️  Node staking rewards monitoring not yet implemented".bright_yellow());
            if let Some(node_id) = node_id {
                info!("{} {}", "🆔 Node ID:".bright_white(), node_id.bright_cyan());
            }
            info!("{} {}ms", "🔄 Would update every:".bright_white(), interval.to_string().bright_cyan());
        }

        MonitorTarget::All { interval } => {
            monitor.monitor_all(interval).await?;
        }
    }

    Ok(())
}

pub async fn show_flow_info(config: &Config) -> Result<()> {
    let monitor = FlowMonitor::new(config.clone());

    match monitor.get_latest_block().await {
        Ok(block_data) => {
            println!("{}", "🌊 Current Flow Blockchain Information".bright_cyan().bold());

            if let Some(result) = block_data.get("result") {
                if let Some(height) = result.get("height").and_then(|h| h.as_u64()) {
                    println!("   {} {}", "Block Height:".bright_white(), height.to_string().bright_yellow());
                }

                if let Some(id) = result.get("id").and_then(|id| id.as_str()) {
                    println!("   {} {}", "Block ID:".bright_white(), format!("{}...", &id[..16]).bright_green());
                }

                if let Some(timestamp) = result.get("timestamp").and_then(|ts| ts.as_str()) {
                    println!("   {} {}", "Timestamp:".bright_white(), timestamp.bright_blue());
                }

                println!("   {} {}", "Network:".bright_white(), "Flow Mainnet".bright_magenta());
                println!("   {} {}", "API Endpoint:".bright_white(), config.flow_rpc_url.bright_cyan());
            }
        }
        Err(e) => {
            error!("{} {}", "❌ Failed to fetch Flow blockchain info:".bright_red(), e);
        }
    }

    Ok(())
}
