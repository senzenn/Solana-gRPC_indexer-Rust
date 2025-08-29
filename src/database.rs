use anyhow::Result;
use sqlx::{Pool, Sqlite, Row};
use std::time::Duration;
use crate::config::DatabaseConfig;
use tracing::{info, error, debug, warn};
use colored::*;
use chrono::{DateTime, Utc};
use solana_client::rpc_client::RpcClient;
use solana_transaction_status::{UiTransactionEncoding, TransactionDetails};

pub struct Database {
    pool: Pool<Sqlite>,
}

impl Database {
    pub async fn new(config: &DatabaseConfig) -> Result<Self> {
        if !config.enable_database {
            return Err(anyhow::anyhow!("Database is disabled in configuration"));
        }

        info!("{} {}", "🔗 Connecting to SQLite database:".bright_blue(), config.database_url.yellow());

        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(config.max_connections)
            .min_connections(config.min_connections)
            .acquire_timeout(Duration::from_secs(config.connection_timeout_seconds))
            .connect(&config.database_url)
            .await?;

        if config.auto_migrate {
            debug!("Running database migrations...");
            sqlx::migrate!("./migrations/sqlite").run(&pool).await?;
            info!("{}", "📋 Database migrations completed".bright_green());
        }

        info!("{}", "✅ Database connection established".bright_green());

        Ok(Self { pool })
    }

    pub fn get_pool(&self) -> &Pool<Sqlite> {
        &self.pool
    }

    pub async fn insert_slot(&self, slot: u64, blockhash: &str, parent_slot: u64, finalized: bool, timestamp: DateTime<Utc>) -> Result<()> {
        debug!("Inserting slot {} into database", slot);

        sqlx::query(
            "INSERT OR REPLACE INTO slots (slot, blockhash, parent_slot, finalized, timestamp) VALUES (?, ?, ?, ?, ?)"
        )
        .bind(slot as i64)
        .bind(blockhash)
        .bind(parent_slot as i64)
        .bind(finalized)
        .bind(timestamp)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn get_slot(&self, slot: u64) -> Result<Option<SlotData>> {
        debug!("Fetching slot {} from database", slot);

        let row = sqlx::query(
            "SELECT slot, blockhash, parent_slot, finalized, timestamp FROM slots WHERE slot = ?"
        )
        .bind(slot as i64)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(row) = row {
            Ok(Some(SlotData {
                slot: row.get::<i64, _>("slot") as u64,
                blockhash: row.get("blockhash"),
                parent_slot: row.get::<i64, _>("parent_slot") as u64,
                finalized: row.get("finalized"),
                timestamp: row.get("timestamp"),
            }))
        } else {
            Ok(None)
        }
    }

    pub async fn get_recent_slots(&self, limit: u64) -> Result<Vec<SlotData>> {
        debug!("Fetching {} recent slots from database", limit);

        let rows = sqlx::query(
            "SELECT slot, blockhash, parent_slot, finalized, timestamp FROM slots ORDER BY slot DESC LIMIT ?"
        )
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        let slots = rows.into_iter().map(|row| SlotData {
            slot: row.get::<i64, _>("slot") as u64,
            blockhash: row.get("blockhash"),
            parent_slot: row.get::<i64, _>("parent_slot") as u64,
            finalized: row.get("finalized"),
            timestamp: row.get("timestamp"),
        }).collect();

        Ok(slots)
    }

    pub async fn get_finalized_slots(&self, limit: u64) -> Result<Vec<SlotData>> {
        debug!("Fetching {} finalized slots from database", limit);

        let rows = sqlx::query(
            "SELECT slot, blockhash, parent_slot, finalized, timestamp FROM slots WHERE finalized = 1 ORDER BY slot DESC LIMIT ?"
        )
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        let slots = rows.into_iter().map(|row| SlotData {
            slot: row.get::<i64, _>("slot") as u64,
            blockhash: row.get("blockhash"),
            parent_slot: row.get::<i64, _>("parent_slot") as u64,
            finalized: row.get("finalized"),
            timestamp: row.get("timestamp"),
        }).collect();

        Ok(slots)
    }

    // Transaction operations
    pub async fn insert_transaction(&self, signature: &str, slot: u64, fee: u64, status: &str, program_ids: &[String], timestamp: DateTime<Utc>) -> Result<()> {
        debug!("Inserting transaction {} into database", signature);

        let program_ids_json = serde_json::to_string(program_ids)?;

        sqlx::query(
            "INSERT OR REPLACE INTO transactions (signature, slot, fee, status, program_ids, timestamp) VALUES (?, ?, ?, ?, ?, ?)"
        )
        .bind(signature)
        .bind(slot as i64)
        .bind(fee as i64)
        .bind(status)
        .bind(program_ids_json)
        .bind(timestamp)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn get_transaction(&self, signature: &str) -> Result<Option<TransactionData>> {
        debug!("Fetching transaction {} from database", signature);

        let row = sqlx::query(
            "SELECT signature, slot, fee, status, program_ids, timestamp FROM transactions WHERE signature = ?"
        )
        .bind(signature)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(row) = row {
            let program_ids_str: String = row.get("program_ids");
            let program_ids: Vec<String> = serde_json::from_str(&program_ids_str)?;
            Ok(Some(TransactionData {
                signature: row.get("signature"),
                slot: row.get::<i64, _>("slot") as u64,
                fee: row.get::<i64, _>("fee") as u64,
                status: row.get("status"),
                program_ids,
                timestamp: row.get("timestamp"),
            }))
        } else {
            Ok(None)
        }
    }

    pub async fn get_transactions_by_slot(&self, slot: u64) -> Result<Vec<TransactionData>> {
        debug!("Fetching transactions for slot {} from database", slot);

        let rows = sqlx::query(
            "SELECT signature, slot, fee, status, program_ids, timestamp FROM transactions WHERE slot = ?"
        )
        .bind(slot as i64)
        .fetch_all(&self.pool)
        .await?;

        let transactions = rows.into_iter().map(|row| {
            let program_ids_str: String = row.get("program_ids");
            let program_ids: Vec<String> = serde_json::from_str(&program_ids_str).unwrap_or_default();
            TransactionData {
                signature: row.get("signature"),
                slot: row.get::<i64, _>("slot") as u64,
                fee: row.get::<i64, _>("fee") as u64,
                status: row.get("status"),
                program_ids,
                timestamp: row.get("timestamp"),
            }
        }).collect();

        Ok(transactions)
    }

    // Leader operations
    pub async fn insert_slot_leader(&self, slot: u64, leader_pubkey: &str, validator_name: Option<&str>) -> Result<()> {
        debug!("Inserting slot leader for slot {} into database", slot);

        sqlx::query(
            "INSERT OR REPLACE INTO slot_leaders (slot, leader_pubkey, validator_name) VALUES (?, ?, ?)"
        )
        .bind(slot as i64)
        .bind(leader_pubkey)
        .bind(validator_name)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn get_slot_leader(&self, slot: u64) -> Result<Option<SlotLeaderData>> {
        debug!("Fetching leader for slot {} from database", slot);

        let row = sqlx::query(
            "SELECT slot, leader_pubkey, validator_name FROM slot_leaders WHERE slot = ?"
        )
        .bind(slot as i64)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(row) = row {
            Ok(Some(SlotLeaderData {
                slot: row.get::<i64, _>("slot") as u64,
                leader_pubkey: row.get("leader_pubkey"),
                validator_name: row.get("validator_name"),
            }))
        } else {
            Ok(None)
        }
    }

    // Test connection
    pub async fn test_connection(&self) -> Result<()> {
        info!("{}", "🧪 Testing database connection...".bright_cyan());

        sqlx::query("SELECT 1").fetch_one(&self.pool).await?;

        info!("{}", "✅ Database connection test successful!".bright_green());
        Ok(())
    }

    // Display statistics
    pub async fn show_database_stats(&self) -> Result<()> {
        println!("{}", "📊 Database Statistics".bright_cyan().bold());
        println!();

        // Get counts for each table
        let slots_count = sqlx::query("SELECT COUNT(*) as count FROM slots")
            .fetch_one(&self.pool)
            .await?
            .get::<i64, _>("count");

        let transactions_count = sqlx::query("SELECT COUNT(*) as count FROM transactions")
            .fetch_one(&self.pool)
            .await?
            .get::<i64, _>("count");

        let leaders_count = sqlx::query("SELECT COUNT(*) as count FROM slot_leaders")
            .fetch_one(&self.pool)
            .await?
            .get::<i64, _>("count");

        println!("   {} {}", "Slots:".bright_white(), slots_count.to_string().bright_yellow());
        println!("   {} {}", "Transactions:".bright_white(), transactions_count.to_string().bright_yellow());
        println!("   {} {}", "Slot Leaders:".bright_white(), leaders_count.to_string().bright_yellow());
        println!();

        Ok(())
    }

    // Real data fetching methods
    pub async fn fetch_and_store_current_slot(&self, rpc_client: &RpcClient) -> Result<u64> {
        info!("{}", "🔄 Fetching current slot from Solana RPC...".bright_cyan());

        let current_slot = rpc_client.get_slot()?;
        info!("{} {}", "📍 Current slot:".bright_blue(), current_slot.to_string().bright_yellow());

        // Get slot info including blockhash with proper configuration
        let slot_info = rpc_client.get_block_with_config(
            current_slot,
            solana_client::rpc_config::RpcBlockConfig {
                encoding: Some(UiTransactionEncoding::Base58),
                transaction_details: Some(solana_transaction_status::TransactionDetails::None),
                rewards: Some(false),
                commitment: None,
                max_supported_transaction_version: Some(0),
            },
        );
        match slot_info {
            Ok(block) => {
                let timestamp = if let Some(block_time) = block.block_time {
                    DateTime::from_timestamp(block_time, 0).unwrap_or_else(|| Utc::now())
                } else {
                    Utc::now()
                };

                self.insert_slot(
                    current_slot,
                    &block.blockhash,
                    block.parent_slot,
                    true, // Current slot is considered finalized for this demo
                    timestamp,
                ).await?;

                info!("{} {}", "✅ Stored current slot:".bright_green(), current_slot.to_string().bright_yellow());
            }
            Err(e) => {
                warn!("{} {} - {}", "⚠️  Could not get block info for slot".bright_yellow(), current_slot, e);
                // Store with minimal info
                self.insert_slot(
                    current_slot,
                    "unknown_blockhash",
                    current_slot.saturating_sub(1),
                    false,
                    Utc::now(),
                ).await?;
            }
        }

        Ok(current_slot)
    }

    pub async fn fetch_and_store_recent_slots(&self, rpc_client: &RpcClient, count: u64) -> Result<Vec<u64>> {
        info!("{} {} {}", "🔄 Fetching".bright_cyan(), count.to_string().bright_yellow(), "recent slots...".bright_cyan());

        let current_slot = rpc_client.get_slot()?;
        let mut stored_slots = Vec::new();

        for i in 0..count {
            let slot_number = current_slot.saturating_sub(i);

            match rpc_client.get_block_with_config(
                slot_number,
                solana_client::rpc_config::RpcBlockConfig {
                    encoding: Some(UiTransactionEncoding::Base58),
                    transaction_details: Some(solana_transaction_status::TransactionDetails::None),
                    rewards: Some(false),
                    commitment: None,
                    max_supported_transaction_version: Some(0),
                },
            ) {
                Ok(block) => {
                    let timestamp = if let Some(block_time) = block.block_time {
                        DateTime::from_timestamp(block_time, 0).unwrap_or_else(|| Utc::now())
                    } else {
                        Utc::now()
                    };

                    self.insert_slot(
                        slot_number,
                        &block.blockhash,
                        block.parent_slot,
                        slot_number < current_slot.saturating_sub(31), // Consider slots older than 31 as finalized
                        timestamp,
                    ).await?;

                    // Log transaction count for this block but don't store summary records
                    if let Some(transactions) = &block.transactions {
                        let tx_count = transactions.len();
                        if tx_count > 0 {
                            debug!("Block {} contains {} transactions", slot_number, tx_count);
                        }
                    }

                    stored_slots.push(slot_number);
                    info!("{} {}", "✅ Stored slot:".bright_green(), slot_number.to_string().bright_yellow());
                }
                Err(e) => {
                    debug!("Could not get block info for slot {}: {}", slot_number, e);
                    // Store with minimal info
                    self.insert_slot(
                        slot_number,
                        "unknown_blockhash",
                        slot_number.saturating_sub(1),
                        false,
                        Utc::now(),
                    ).await?;
                    stored_slots.push(slot_number);
                }
            }

            // Add small delay to avoid overwhelming the RPC
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }

        info!("{} {} {}", "✅ Stored".bright_green(), stored_slots.len().to_string().bright_yellow(), "slots with transactions".bright_green());
        Ok(stored_slots)
    }

    pub async fn fetch_and_store_slot_leaders(&self, rpc_client: &RpcClient, slot: u64, count: u64) -> Result<()> {
        info!("{} {} {} {}", "🔄 Fetching".bright_cyan(), count.to_string().bright_yellow(), "slot leaders starting from slot".bright_cyan(), slot.to_string().bright_yellow());

        match rpc_client.get_slot_leaders(slot, count) {
            Ok(leaders) => {
                for (i, leader_pubkey) in leaders.iter().enumerate() {
                    let slot_number = slot + i as u64;
                    self.insert_slot_leader(
                        slot_number,
                        &leader_pubkey.to_string(),
                        None, // We don't have validator names from this API
                    ).await?;
                }
                info!("{} {} {}", "✅ Stored".bright_green(), leaders.len().to_string().bright_yellow(), "slot leaders".bright_green());
            }
            Err(e) => {
                error!("{} {}", "❌ Error fetching slot leaders:".bright_red(), e);
                return Err(e.into());
            }
        }

        Ok(())
    }

    pub async fn fetch_and_store_transaction(&self, rpc_client: &RpcClient, signature: &str) -> Result<()> {
        info!("{} {}", "🔄 Fetching transaction:".bright_cyan(), signature.bright_blue());

        match rpc_client.get_transaction(
            &signature.parse()?,
            UiTransactionEncoding::Json,
        ) {
            Ok(transaction) => {
                let slot = transaction.slot;
                let fee = transaction.transaction
                    .meta
                    .as_ref()
                    .map(|m| m.fee)
                    .unwrap_or(0);

                let status = if transaction.transaction
                    .meta
                    .as_ref()
                    .map(|m| m.err.is_none())
                    .unwrap_or(false) {
                    "success"
                } else {
                    "failed"
                };

                // Extract program IDs (simplified for now)
                let program_ids: Vec<String> = vec!["system".to_string()];

                let timestamp = if let Some(block_time) = transaction.block_time {
                    DateTime::from_timestamp(block_time, 0).unwrap_or_else(|| Utc::now())
                } else {
                    Utc::now()
                };

                self.insert_transaction(
                    signature,
                    slot,
                    fee,
                    status,
                    &program_ids,
                    timestamp,
                ).await?;

                info!("{} {}", "✅ Stored transaction:".bright_green(), signature.bright_blue());
            }
            Err(e) => {
                error!("{} {} - {}", "❌ Error fetching transaction:".bright_red(), signature, e);
                return Err(e.into());
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct SlotData {
    pub slot: u64,
    pub blockhash: String,
    pub parent_slot: u64,
    pub finalized: bool,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct TransactionData {
    pub signature: String,
    pub slot: u64,
    pub fee: u64,
    pub status: String,
    pub program_ids: Vec<String>,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct SlotLeaderData {
    pub slot: u64,
    pub leader_pubkey: String,
    pub validator_name: Option<String>,
}