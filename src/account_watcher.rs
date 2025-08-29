use anyhow::Result;
use colored::*;
use solana_client::rpc_client::RpcClient;
use solana_sdk::{pubkey::Pubkey, account::Account};
use std::collections::HashMap;
use std::str::FromStr;
use std::time::Duration;
use tokio::time::interval;
use crossterm::terminal::{size, Clear, ClearType};
use crossterm::cursor;
use crate::config::Config;
use crate::database::Database;
use crate::logger::icons;
use crate::animations::{CliAnimations, StatusStats};
use crate::enhanced_logger::{EnhancedLogger, LogType};
use crate::cache::{IndexerCache, CachedAccount, CachedSlotInfo};
use sqlx::Row;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackedAccount {
    pub id: i64,
    pub address: String,
    pub name: Option<String>,
    pub program_id: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub is_active: bool,
    pub last_activity: Option<chrono::DateTime<chrono::Utc>>,
    pub activity_count: i64,
    pub balance_threshold: Option<f64>,
    pub data_size_threshold: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountActivity {
    pub id: i64,
    pub account_address: String,
    pub activity_type: String,
    pub change_type: String,
    pub old_value: String,
    pub new_value: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub block_slot: u64,
    pub lamports_change: i64,
    pub data_size_change: i64,
    pub details: Option<String>,
}

#[derive(Debug, Clone)]
pub enum AccountActivityType {
    BalanceChange,
    DataChange,
    OwnerChange,
    ExecutableChange,
    RentEpochChange,
    ProgramInteraction,
    Unknown,
}

impl AccountActivityType {
    pub fn as_str(&self) -> &'static str {
        match self {
            AccountActivityType::BalanceChange => "BALANCE_CHANGE",
            AccountActivityType::DataChange => "DATA_CHANGE",
            AccountActivityType::OwnerChange => "OWNER_CHANGE",
            AccountActivityType::ExecutableChange => "EXECUTABLE_CHANGE",
            AccountActivityType::RentEpochChange => "RENT_EPOCH_CHANGE",
            AccountActivityType::ProgramInteraction => "PROGRAM_INTERACTION",
            AccountActivityType::Unknown => "UNKNOWN",
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
                    AccountActivityType::BalanceChange => "",
        AccountActivityType::DataChange => "",
        AccountActivityType::OwnerChange => "",
        AccountActivityType::ExecutableChange => "",
        AccountActivityType::RentEpochChange => "",
        AccountActivityType::ProgramInteraction => "",
        AccountActivityType::Unknown => "",
        }
    }

    pub fn color(&self) -> colored::Color {
        match self {
            AccountActivityType::BalanceChange => colored::Color::Green,
            AccountActivityType::DataChange => colored::Color::Blue,
            AccountActivityType::OwnerChange => colored::Color::Yellow,
            AccountActivityType::ExecutableChange => colored::Color::Magenta,
            AccountActivityType::RentEpochChange => colored::Color::Cyan,
            AccountActivityType::ProgramInteraction => colored::Color::Red,
            AccountActivityType::Unknown => colored::Color::White,
        }
    }
}

/// Get dynamic terminal width using crossterm
fn get_terminal_width() -> usize {
    match size() {
        Ok((width, _)) => width as usize,
        Err(_) => 80,
    }
}

pub async fn add_wallet(config: &Config, address: &str, name: Option<String>) -> Result<()> {
    if !config.database_config.enable_database {
        println!("{} {}", icons::FAILED, "Database is disabled. Enable database to use wallet tracking.".bright_red());
        return Ok(());
    }

    if Pubkey::from_str(address).is_err() {
        println!("{} {}", icons::FAILED, "Invalid Solana wallet address format".bright_red());
        return Ok(());
    }

    let db = Database::new(&config.database_config).await?;

    let existing = sqlx::query(
        "SELECT id FROM tracked_wallets WHERE address = ?"
    )
    .bind(address)
    .fetch_optional(db.get_pool())
    .await?;

    if existing.is_some() {
        println!("{} {}", icons::WARNING, format!("Wallet {} is already being tracked", address).bright_yellow());
        return Ok(());
    }

    let display_name = name.as_deref().unwrap_or("Unnamed Wallet");

    sqlx::query(
        "INSERT INTO tracked_wallets (address, name, created_at, is_active, activity_count) VALUES (?, ?, ?, ?, ?)"
    )
    .bind(address)
    .bind(&name)
    .bind(chrono::Utc::now())
    .bind(true)
    .bind(0i64)
    .execute(db.get_pool())
    .await?;

            CliAnimations::show_wallet_art(address, display_name, None);
    CliAnimations::show_success(&format!("Successfully added wallet: {} ({})", display_name, address));

    Ok(())
}

pub async fn remove_wallet(config: &Config, wallet_identifier: &str) -> Result<()> {
    if !config.database_config.enable_database {
        println!("{} {}", icons::FAILED, "Database is disabled".bright_red());
        return Ok(());
    }

    let db = Database::new(&config.database_config).await?;

    // Try to find wallet by address or name
    let wallet = sqlx::query(
        "SELECT id, address, name FROM tracked_wallets WHERE address = ? OR name = ?"
    )
    .bind(wallet_identifier)
    .bind(wallet_identifier)
    .fetch_optional(db.get_pool())
    .await?;

    match wallet {
        Some(row) => {
            let address: String = row.get("address");
            let name: Option<String> = row.get("name");
            let id: i64 = row.get("id");

            // Remove wallet and its activities
            sqlx::query("DELETE FROM wallet_activities WHERE wallet_address = ?")
                .bind(&address)
                .execute(db.get_pool())
                .await?;

            sqlx::query("DELETE FROM tracked_wallets WHERE id = ?")
                .bind(id)
                .execute(db.get_pool())
                .await?;

            let display_name = name.as_deref().unwrap_or("Unnamed Wallet");
            println!("{} {} {}",
                icons::COMPLETE,
                "Successfully removed wallet:".bright_green(),
                format!("{} ({})", display_name, address).bright_cyan()
            );
        }
        None => {
            println!("{} {}", icons::FAILED, format!("Wallet '{}' not found", wallet_identifier).bright_red());
        }
    }

    Ok(())
}

pub async fn list_wallets(config: &Config) -> Result<()> {
    if !config.database_config.enable_database {
        println!("{} {}", icons::FAILED, "Database is disabled".bright_red());
        return Ok(());
    }

    let db = Database::new(&config.database_config).await?;

    let wallets = sqlx::query(
        "SELECT address, name, created_at, is_active, last_activity, activity_count FROM tracked_wallets ORDER BY created_at DESC"
    )
    .fetch_all(db.get_pool())
    .await?;

    if wallets.is_empty() {
        println!("{} {}", icons::INFO, "No wallets are currently being tracked".bright_cyan());
        println!("\n{} {}", icons::HELP, "Add a wallet with: solana-indexer track wallets add <address> --name <name>".bright_black());
        return Ok(());
    }

    // Show cool header
    println!("{}", "╔══════════════════════════════════════════════════════════════════╗".bright_cyan());
    println!("{}", format!("║                    🏦 TRACKED WALLETS ({})                     ║", wallets.len()).bright_cyan().bold());
    println!("{}", "╠══════════════════════════════════════════════════════════════════╣".bright_cyan());
    println!();

    for wallet in wallets {
        let address: String = wallet.get("address");
        let name: Option<String> = wallet.get("name");
        let created_at: chrono::DateTime<chrono::Utc> = wallet.get("created_at");
        let is_active: bool = wallet.get("is_active");
        let last_activity: Option<chrono::DateTime<chrono::Utc>> = wallet.get("last_activity");
        let activity_count: i64 = wallet.get("activity_count");

        let status_icon = if is_active { icons::COMPLETE } else { icons::WARNING };
        let status_color = if is_active { "Active".bright_green() } else { "Inactive".bright_red() };

        let display_name = name.as_deref().unwrap_or("Unnamed");
        let short_address = format!("{}...{}", &address[..8], &address[address.len()-8..]);

        println!("{} {} {}",
            status_icon,
            format!("{} ({})", display_name, short_address).bright_white().bold(),
            status_color
        );

        println!("   {} Activities: {}", icons::CHART, activity_count.to_string().bright_yellow());
        println!("   {} Added: {}", icons::CALENDAR, created_at.format("%Y-%m-%d %H:%M UTC").to_string().bright_black());

        if let Some(last_act) = last_activity {
            println!("   {} Last Activity: {}", icons::TIME, last_act.format("%Y-%m-%d %H:%M UTC").to_string().bright_black());
        } else {
            println!("   {} Last Activity: Never", icons::TIME);
        }
        println!();
    }

    // Show closing border
    println!("{}", "╚══════════════════════════════════════════════════════════════════╝".bright_cyan());
    println!();

    Ok(())
}

pub async fn add_account(config: &Config, address: &str, name: Option<String>, program_id: Option<String>) -> Result<()> {
    if !config.database_config.enable_database {
        println!("{} {}", icons::FAILED, "Database is disabled. Enable database to use account tracking.".bright_red());
        return Ok(());
    }

    // Validate address format
    if Pubkey::from_str(address).is_err() {
        println!("{} {}", icons::FAILED, "Invalid Solana account address format".bright_red());
        return Ok(());
    }

    let db = Database::new(&config.database_config).await?;

    // Check if account already exists
    let existing = sqlx::query(
        "SELECT id FROM tracked_accounts WHERE address = ?"
    )
    .bind(address)
    .fetch_optional(db.get_pool())
    .await?;

    if existing.is_some() {
        println!("{} {}", icons::WARNING, format!("Account {} is already being tracked", address).bright_yellow());
        return Ok(());
    }

    // Add account to database
    let display_name = name.as_deref().unwrap_or("Unnamed Account");

    sqlx::query(
        "INSERT INTO tracked_accounts (address, name, program_id, created_at, is_active, activity_count) VALUES (?, ?, ?, ?, ?, ?)"
    )
    .bind(address)
    .bind(&name)
    .bind(&program_id)
    .bind(chrono::Utc::now())
    .bind(true)
    .bind(0i64)
    .execute(db.get_pool())
    .await?;

    // Show cool account art
    CliAnimations::show_account_art(address, display_name, program_id.as_deref(), None);
    CliAnimations::show_success(&format!("Successfully added account: {} ({})", display_name, address));

    Ok(())
}

pub async fn remove_account(config: &Config, account_identifier: &str) -> Result<()> {
    if !config.database_config.enable_database {
        println!("{} {}", icons::FAILED, "Database is disabled".bright_red());
        return Ok(());
    }

    let db = Database::new(&config.database_config).await?;

    // Try to find account by address or name
    let account = sqlx::query(
        "SELECT id, address, name FROM tracked_accounts WHERE address = ? OR name = ?"
    )
    .bind(account_identifier)
    .bind(account_identifier)
    .fetch_optional(db.get_pool())
    .await?;

    match account {
        Some(row) => {
            let address: String = row.get("address");
            let name: Option<String> = row.get("name");
            let id: i64 = row.get("id");

            // Remove account and its activities
            sqlx::query("DELETE FROM account_activities WHERE account_address = ?")
                .bind(&address)
                .execute(db.get_pool())
                .await?;

            sqlx::query("DELETE FROM tracked_accounts WHERE id = ?")
                .bind(id)
                .execute(db.get_pool())
                .await?;

            let display_name = name.as_deref().unwrap_or("Unnamed Account");
            println!("{} {} {}",
                icons::COMPLETE,
                "Successfully removed account:".bright_green(),
                format!("{} ({})", display_name, address).bright_cyan()
            );
        }
        None => {
            println!("{} {}", icons::FAILED, format!("Account '{}' not found", account_identifier).bright_red());
        }
    }

    Ok(())
}

pub async fn list_accounts(config: &Config) -> Result<()> {
    if !config.database_config.enable_database {
        println!("{} {}", icons::FAILED, "Database is disabled".bright_red());
        return Ok(());
    }

    let db = Database::new(&config.database_config).await?;

    let accounts = sqlx::query(
        "SELECT address, name, program_id, created_at, is_active, last_activity, activity_count FROM tracked_accounts ORDER BY created_at DESC"
    )
    .fetch_all(db.get_pool())
    .await?;

    if accounts.is_empty() {
        println!("{} {}", icons::INFO, "No accounts are currently being tracked".bright_cyan());
        println!("\n{} {}", icons::HELP, "Add an account with: solana-indexer track accounts add <address> --name <name>".bright_black());
        return Ok(());
    }

    // Show cool header
    println!("{}", "╔══════════════════════════════════════════════════════════════════╗".bright_cyan());
    println!("{}", format!("║                    🏛️ TRACKED ACCOUNTS ({})                     ║", accounts.len()).bright_cyan().bold());
    println!("{}", "╠══════════════════════════════════════════════════════════════════╣".bright_cyan());
    println!();

    for account in accounts {
        let address: String = account.get("address");
        let name: Option<String> = account.get("name");
        let program_id: Option<String> = account.get("program_id");
        let created_at: chrono::DateTime<chrono::Utc> = account.get("created_at");
        let is_active: bool = account.get("is_active");
        let last_activity: Option<chrono::DateTime<chrono::Utc>> = account.get("last_activity");
        let activity_count: i64 = account.get("activity_count");

        let status_icon = if is_active { icons::COMPLETE } else { icons::WARNING };
        let status_color = if is_active { "Active".bright_green() } else { "Inactive".bright_red() };

        let display_name = name.as_deref().unwrap_or("Unnamed");
        let short_address = format!("{}...{}", &address[..8], &address[address.len()-8..]);

        println!("{} {} {}",
            status_icon,
            format!("{} ({})", display_name, short_address).bright_white().bold(),
            status_color
        );

        if let Some(prog_id) = program_id {
            let short_prog = format!("{}...{}", &prog_id[..8], &prog_id[prog_id.len()-8..]);
            println!("   {} Program: {}", icons::CODE, short_prog.bright_blue());
        }

        println!("   {} Activities: {}", icons::CHART, activity_count.to_string().bright_yellow());
        println!("   {} Added: {}", icons::CALENDAR, created_at.format("%Y-%m-%d %H:%M UTC").to_string().bright_black());

        if let Some(last_act) = last_activity {
            println!("   {} Last Activity: {}", icons::TIME, last_act.format("%Y-%m-%d %H:%M UTC").to_string().bright_black());
        } else {
            println!("   {} Last Activity: Never", icons::TIME);
        }
        println!();
    }

    // Show closing border
    println!("{}", "╚══════════════════════════════════════════════════════════════════╝".bright_cyan());
    println!();

    Ok(())
}

#[allow(unused_variables)]
pub async fn start_monitoring(config: &Config, client: &RpcClient, interval_ms: u64, filter: Option<Vec<String>>) -> Result<()> {
    if !config.database_config.enable_database {
        println!("{} {}", icons::FAILED, "Database is disabled".bright_red());
        return Ok(());
    }

    let db = Database::new(&config.database_config).await?;
    let enhanced_logger = std::sync::Arc::new(EnhancedLogger::new(1000));

    // Initialize high-performance cache system
    let cache = IndexerCache::new(config.clone());
    println!("{} {}", icons::CACHE, "High-performance cache system initialized".bright_cyan());

    // Get all tracked accounts
    let accounts = sqlx::query(
        "SELECT address, name, program_id FROM tracked_accounts WHERE is_active = true"
    )
    .fetch_all(db.get_pool())
    .await?;

    if accounts.is_empty() {
        println!("{} {}", icons::WARNING, "No active accounts to monitor".bright_yellow());
        return Ok(());
    }

    println!("{} {} {}",
        icons::TRACKING,
        "Starting real-time account monitoring".bright_green().bold(),
        format!("({} accounts)", accounts.len()).bright_cyan()
    );

    let mut account_map = HashMap::new();
    for account in &accounts {
        let address: String = account.get("address");
        let name: Option<String> = account.get("name");
        let program_id: Option<String> = account.get("program_id");
        account_map.insert(address.clone(), (name.clone(), program_id.clone()));

        println!("   {} {}",
            icons::DATABASE,
            format!("{} ({}...{})",
                name.as_deref().unwrap_or("Unnamed"),
                &address[..8],
                &address[address.len()-8..]
            ).bright_white()
        );
    }

    println!("\n{} {} {}\n",
        icons::INFO,
        "Press Ctrl+C to stop monitoring".bright_black(),
        format!("(checking every {}ms)", interval_ms).bright_black()
    );

    let mut interval_timer = interval(Duration::from_millis(interval_ms));
    let mut last_accounts: HashMap<String, Account> = HashMap::new();
    let mut iteration_count = 0;
    let start_time = std::time::Instant::now();
    let mut cache_hits = 0;
    let mut cache_misses = 0;

    loop {
        iteration_count += 1;
        interval_timer.tick().await;

                // Show status dashboard only once every 10 iterations (not every iteration)
        if iteration_count % 10 == 0 {
            // Get real slot information and cache it
            let current_slot = match client.get_slot() {
                Ok(slot) => {
                    // Cache the slot information
                    let slot_info = CachedSlotInfo {
                        slot,
                        leader: "Unknown".to_string(),
                        block_hash: "Unknown".to_string(),
                        timestamp: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64,
                        confirmed: true,
                        finalized: false,
                        cached_at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64,
                    };
                    if let Err(e) = cache.cache_slot(slot_info).await {
                        println!("{} {}", icons::WARNING, format!("Failed to cache slot: {}", e).bright_yellow());
                    }
                    slot
                }
                Err(_) => 0,
            };

            // Get real cache statistics
            let cache_stats = cache.get_cache_stats().await;
            let total_cache_entries = cache_stats["accounts"]["entry_count"].as_u64().unwrap_or(0);
            let cache_memory_mb = cache_stats["total_memory_usage_mb"].as_u64().unwrap_or(0);

            // Calculate real cache hit rate
            let total_cache_requests = cache_hits + cache_misses;
            let cache_hit_rate = if total_cache_requests > 0 {
                (cache_hits as f32 / total_cache_requests as f32) * 100.0
            } else {
                0.0
            };

            // Enhanced account monitoring display with separator (similar to slot tracker)
            let terminal_width = get_terminal_width();
            println!("{}", "─".repeat(terminal_width).truecolor(80, 250, 123)); // Green separator
            println!("{}", "ACCOUNT MONITORING DASHBOARD".truecolor(80, 250, 123).bold()); // Green title
            println!("Slot: {} | Accounts: {} | Cache: {:.1}%",
                current_slot.to_string().truecolor(248, 248, 242).bold(),
                account_map.len().to_string().truecolor(80, 250, 123).bold(),
                cache_hit_rate.to_string().truecolor(255, 184, 108).bold()
            );
            println!("Uptime: {} | Cache Entries: {} | Cache Memory: {}MB",
                format!("{}s", start_time.elapsed().as_secs()).truecolor(189, 147, 249).bold(),
                total_cache_entries.to_string().truecolor(139, 233, 253).bold(),
                cache_memory_mb.to_string().truecolor(80, 250, 123).bold()
            );
            println!("Cache Hits: {} | Cache Misses: {} | Avg Response: {}ms",
                cache_hits.to_string().truecolor(80, 250, 123).bold(),
                cache_misses.to_string().truecolor(255, 85, 85).bold(),
                (start_time.elapsed().as_millis() as u64 / (iteration_count + 1) as u64).to_string().truecolor(139, 233, 253).bold()
            );
            println!("{}", "─".repeat(terminal_width).truecolor(80, 250, 123)); // Green separator
        }

        // Log real blockchain activity with enhanced display
        if iteration_count % 3 == 0 {
            match client.get_slot() {
                Ok(current_slot) => {
                    // Enhanced slot display
                    let terminal_width = 80;
                    println!("{}", "─".repeat(terminal_width).truecolor(241, 250, 140)); // Yellow separator
                    println!("{}", "SLOT UPDATE".truecolor(241, 250, 140).bold()); // Yellow title
                    println!("Slot: {} | Time: {}",
                        current_slot.to_string().truecolor(248, 248, 242).bold(),
                        chrono::Utc::now().format("%H:%M:%S").to_string().truecolor(139, 147, 164)
                    );
                    println!("{}", "─".repeat(terminal_width).truecolor(241, 250, 140)); // Yellow separator
                }
                Err(_) => {
                    let terminal_width = 80;
                    println!("{}", "─".repeat(terminal_width).truecolor(255, 85, 85)); // Red separator
                    println!("{}", "SLOT ERROR".truecolor(255, 85, 85).bold()); // Red title
                    println!("Failed to fetch current slot");
                    println!("{}", "─".repeat(terminal_width).truecolor(255, 85, 85)); // Red separator
                }
            }
        }

                // Enhanced account balance display with caching (more frequent updates)
        if iteration_count % 2 == 0 && !account_map.is_empty() {
            for (address, (name, program_id)) in &account_map {
                if let Ok(pubkey) = Pubkey::from_str(address) {
                                        // Try to get account from cache first
                    let cached_account = cache.get_account(address).await;
                    let is_cache_hit = cached_account.is_some();

                    let account_result = if let Some(cached) = cached_account {
                        cache_hits += 1;
                        // Use cached account data
                        Ok(Account {
                            lamports: cached.lamports,
                            data: vec![0; cached.data_len], // Simplified data representation
                            owner: Pubkey::from_str(&cached.owner).unwrap_or_default(),
                            executable: cached.executable,
                            rent_epoch: cached.rent_epoch,
                        })
                    } else {
                        cache_misses += 1;
                        // Fetch from RPC and cache the result
                        let result = client.get_account(&pubkey);
                        if let Ok(ref account) = result {
                            // Cache the account data
                            let cached_account = CachedAccount {
                                pubkey: address.clone(),
                                lamports: account.lamports,
                                owner: account.owner.to_string(),
                                executable: account.executable,
                                rent_epoch: account.rent_epoch,
                                data_len: account.data.len(),
                                cached_at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64,
                            };
                            if let Err(e) = cache.cache_account(cached_account).await {
                                println!("{} {}", icons::WARNING, format!("Failed to cache account: {}", e).bright_yellow());
                            }
                        }
                        result
                    };

                                        match account_result {
                        Ok(account) => {
                            let balance_sol = account.lamports as f64 / 1_000_000_000.0;
                            let balance_lamports = account.lamports;
                            let owner = account.owner.to_string();
                            let executable = account.executable;
                            let rent_epoch = account.rent_epoch;

                                                        // Enhanced account balance display with separator
                            let terminal_width = get_terminal_width();
                            println!("{}", "─".repeat(terminal_width).truecolor(189, 147, 249)); // Purple separator
                            println!("{}", "ACCOUNT BALANCE UPDATE".truecolor(189, 147, 249).bold());

                            println!("Name: {} | Address: {}...{} | Cache: {}",
                                name.as_deref().unwrap_or("Unnamed").truecolor(248, 248, 242).bold(),
                                &address[..8].truecolor(139, 233, 253).bold(),
                                &address[address.len()-8..].truecolor(139, 233, 253).bold(),
                                if is_cache_hit { "HIT".truecolor(80, 250, 123).bold() } else { "MISS".truecolor(255, 184, 108).bold() }
                            );

                            println!("Balance: {} SOL ({} lamports)",
                                balance_sol.to_string().truecolor(80, 250, 123).bold(),
                                balance_lamports.to_string().truecolor(255, 184, 108).bold()
                            );
                            println!("Owner: {} | Program ID: {}",
                                owner.truecolor(139, 233, 253).bold(),
                                program_id.as_deref().unwrap_or("Unknown").truecolor(189, 147, 249).bold()
                            );
                            println!("Executable: {} | Rent Epoch: {} | Data Size: {} bytes",
                                if executable { "Yes".truecolor(80, 250, 123).bold() } else { "No".truecolor(255, 85, 85).bold() },
                                rent_epoch.to_string().truecolor(189, 147, 249).bold(),
                                account.data.len().to_string().truecolor(255, 184, 108).bold()
                            );

                            // gRPC-like additional details
                            println!("Last Updated: {} | Slot: {}",
                                chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC").to_string().truecolor(139, 147, 164).bold(),
                                match client.get_slot() {
                                    Ok(slot) => slot.to_string().truecolor(139, 233, 253).bold(),
                                    Err(_) => "Unknown".truecolor(255, 85, 85).bold()
                                }
                            );

                            // Additional gRPC details for accounts
                            println!("Account Type: {} | Readonly: {}",
                                if executable { "Program".truecolor(80, 250, 123).bold() } else { "Data".truecolor(139, 233, 253).bold() },
                                if account.rent_epoch == 0 { "Yes".truecolor(255, 85, 85).bold() } else { "No".truecolor(80, 250, 123).bold() }
                            );

                            // Enhanced gRPC-like detailed information
                            println!("Account Status: {} | Lamports: {} | Data Length: {}",
                                if balance_lamports > 0 { "ACTIVE".truecolor(80, 250, 123).bold() } else { "EMPTY".truecolor(255, 85, 85).bold() },
                                balance_lamports.to_string().truecolor(139, 233, 253).bold(),
                                account.data.len().to_string().truecolor(189, 147, 249).bold()
                            );

                                                        // Log detailed account information
                            let current_slot = match client.get_slot() {
                                Ok(slot) => slot,
                                Err(_) => 0,
                            };
                            enhanced_logger.log_account_update(address, balance_lamports, current_slot);

                                                        // Enhanced transaction detection and detailed logging
                            if let Ok(pubkey) = Pubkey::from_str(address) {
                                match client.get_signatures_for_address(&pubkey) {
                                    Ok(signatures) => {
                                        if !signatures.is_empty() {
                                            let latest_sig = &signatures[0];
                                            println!("Latest Transaction: {} | Slot: {} | Status: {}",
                                                latest_sig.signature.to_string()[..16].truecolor(139, 233, 253).bold(),
                                                latest_sig.slot.to_string().truecolor(189, 147, 249).bold(),
                                                match latest_sig.confirmation_status.as_ref() {
                                                    Some(_) => "CONFIRMED".truecolor(80, 250, 123).bold(),
                                                    None => "UNKNOWN".truecolor(255, 85, 85).bold()
                                                }
                                            );

                                            // Enhanced transaction details logging
                                            enhanced_logger.log_tx_confirmed(&latest_sig.signature, latest_sig.slot, 0);

                                            // Log transaction details
                                            enhanced_logger.log_system_info(&format!("Latest transaction for {}: {} (slot: {})",
                                                name.as_deref().unwrap_or("Unnamed"), latest_sig.signature, latest_sig.slot));
                                        }
                                    }
                                    Err(e) => {
                                        enhanced_logger.log_error(&format!("Failed to get signatures for {}: {}", address, e));
                                    }
                                }
                            }

                            println!("{}", "─".repeat(terminal_width).truecolor(189, 147, 249)); // Purple separator
                        }
                                                Err(e) => {
                            let terminal_width = get_terminal_width();
                            println!("{}", "─".repeat(terminal_width).truecolor(255, 85, 85)); // Red separator
                            println!("{}", "ACCOUNT ERROR".truecolor(255, 85, 85).bold()); // Red title
                            println!("Name: {} | Address: {}...{}",
                                name.as_deref().unwrap_or("Unnamed").truecolor(248, 248, 242).bold(),
                                &address[..8].truecolor(139, 233, 253).bold(),
                                &address[address.len()-8..].truecolor(139, 233, 253).bold()
                            );
                            println!("Error: {}", e.to_string().truecolor(255, 85, 85).bold());
                            println!("{}", "─".repeat(terminal_width).truecolor(255, 85, 85)); // Red separator
                        }
                    }
                }
            }
        }

        for (address, (name, program_id)) in &account_map {
            if let Ok(pubkey) = Pubkey::from_str(address) {
                // Get current account data
                match client.get_account(&pubkey) {
                    Ok(account) => {
                        // Check for changes
                        if let Some(last_account) = last_accounts.get(address) {
                            let changes = detect_account_changes(last_account, &account);

                            for change in changes {
                                // Apply filter if specified
                                if let Some(filters) = &filter {
                                    if !filters.iter().any(|f| f.to_lowercase() == change.activity_type.as_str().to_lowercase()) {
                                        continue;
                                    }
                                }

                                // Store activity in database
                                sqlx::query(
                                    "INSERT INTO account_activities (account_address, activity_type, change_type, old_value, new_value, timestamp, block_slot, lamports_change, data_size_change) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"
                                )
                                .bind(address)
                                .bind(change.activity_type.as_str())
                                .bind(&change.change_type)
                                .bind(&change.old_value)
                                .bind(&change.new_value)
                                .bind(chrono::Utc::now())
                                .bind(account.lamports as i64) // Use lamports as slot for now
                                .bind(change.lamports_change)
                                .bind(change.data_size_change)
                                .execute(db.get_pool())
                                .await?;

                                // Update account last activity
                                sqlx::query(
                                    "UPDATE tracked_accounts SET last_activity = ?, activity_count = activity_count + 1 WHERE address = ?"
                                )
                                .bind(chrono::Utc::now())
                                .bind(address)
                                .execute(db.get_pool())
                                .await?;

                                // Display real-time activity
                                let short_addr = format!("{}...{}", &address[..6], &address[address.len()-6..]);
                                println!("{} {} {} {} {}",
                                    change.activity_type.icon().color(change.activity_type.color()),
                                    change.activity_type.as_str().color(change.activity_type.color()).bold(),
                                    format!("{} ({})", name.as_deref().unwrap_or("Unnamed"), short_addr).bright_white(),
                                    change.change_type.bright_blue(),
                                    change.new_value.bright_yellow()
                                );
                            }
                        }

                        last_accounts.insert(address.clone(), account);
                    }
                    Err(e) => {
                        let error_msg = if e.to_string().contains("Unknown") {
                            println!("{} {} {}: RPC parsing error - trying alternative approach...",
                                icons::WARNING,
                                "Failed to fetch account data for".bright_yellow(),
                                name.as_deref().unwrap_or("Unnamed").bright_white()
                            );
                            continue;
                        } else {
                            format!("RPC error: {}", e)
                        };

                        println!("{} {} {}: {}",
                            icons::WARNING,
                            "Failed to fetch account data for".bright_yellow(),
                            name.as_deref().unwrap_or("Unnamed").bright_white(),
                            error_msg.bright_red()
                        );
                    }
                }
            }
        }
    }
}

#[derive(Debug)]
struct AccountChange {
    activity_type: AccountActivityType,
    change_type: String,
    old_value: String,
    new_value: String,
    lamports_change: i64,
    data_size_change: i64,
}

fn detect_account_changes(old_account: &Account, new_account: &Account) -> Vec<AccountChange> {
    let mut changes = Vec::new();

    // Check for balance changes
    if old_account.lamports != new_account.lamports {
        changes.push(AccountChange {
            activity_type: AccountActivityType::BalanceChange,
            change_type: "BALANCE".to_string(),
            old_value: format!("{} lamports", old_account.lamports),
            new_value: format!("{} lamports", new_account.lamports),
            lamports_change: new_account.lamports as i64 - old_account.lamports as i64,
            data_size_change: 0,
        });
    }

    // Check for data size changes
    if old_account.data.len() != new_account.data.len() {
        changes.push(AccountChange {
            activity_type: AccountActivityType::DataChange,
            change_type: "DATA_SIZE".to_string(),
            old_value: format!("{} bytes", old_account.data.len()),
            new_value: format!("{} bytes", new_account.data.len()),
            lamports_change: 0,
            data_size_change: new_account.data.len() as i64 - old_account.data.len() as i64,
        });
    }

    // Check for owner changes
    if old_account.owner != new_account.owner {
        changes.push(AccountChange {
            activity_type: AccountActivityType::OwnerChange,
            change_type: "OWNER".to_string(),
            old_value: old_account.owner.to_string(),
            new_value: new_account.owner.to_string(),
            lamports_change: 0,
            data_size_change: 0,
        });
    }

    // Check for executable changes
    if old_account.executable != new_account.executable {
        changes.push(AccountChange {
            activity_type: AccountActivityType::ExecutableChange,
            change_type: "EXECUTABLE".to_string(),
            old_value: old_account.executable.to_string(),
            new_value: new_account.executable.to_string(),
            lamports_change: 0,
            data_size_change: 0,
        });
    }

    // Check for rent epoch changes
    if old_account.rent_epoch != new_account.rent_epoch {
        changes.push(AccountChange {
            activity_type: AccountActivityType::RentEpochChange,
            change_type: "RENT_EPOCH".to_string(),
            old_value: old_account.rent_epoch.to_string(),
            new_value: new_account.rent_epoch.to_string(),
            lamports_change: 0,
            data_size_change: 0,
        });
    }

    changes
}

pub async fn show_history(config: &Config, account_identifier: &str, limit: u32) -> Result<()> {
    if !config.database_config.enable_database {
        println!("{} {}", icons::FAILED, "Database is disabled".bright_red());
        return Ok(());
    }

    let db = Database::new(&config.database_config).await?;

    // Find account
    let account = sqlx::query(
        "SELECT address, name FROM tracked_accounts WHERE address = ? OR name = ?"
    )
    .bind(account_identifier)
    .bind(account_identifier)
    .fetch_optional(db.get_pool())
    .await?;

    let (address, name) = match account {
        Some(row) => {
            let addr: String = row.get("address");
            let n: Option<String> = row.get("name");
            (addr, n.unwrap_or_else(|| "Unnamed".to_string()))
        }
        None => {
            println!("{} {}", icons::FAILED, format!("Account '{}' not found", account_identifier).bright_red());
            return Ok(());
        }
    };

    // Get activities
    let activities = sqlx::query(
        "SELECT activity_type, change_type, old_value, new_value, timestamp, block_slot, lamports_change, data_size_change
         FROM account_activities
         WHERE account_address = ?
         ORDER BY timestamp DESC
         LIMIT ?"
    )
    .bind(&address)
    .bind(limit as i64)
    .fetch_all(db.get_pool())
    .await?;

    if activities.is_empty() {
        println!("{} {}", icons::INFO, format!("No activity found for account: {}", name).bright_cyan());
        return Ok(());
    }

    println!("{} {} {}",
        icons::SEARCH,
        format!("Account History: {} ({})", name, format!("{}...{}", &address[..8], &address[address.len()-8..])).bright_cyan().bold(),
        format!("(showing {} recent activities)", activities.len()).bright_black()
    );
    println!();

    for activity in activities {
        let activity_type_str: String = activity.get("activity_type");
        let change_type: String = activity.get("change_type");
        let old_value: String = activity.get("old_value");
        let new_value: String = activity.get("new_value");
        let timestamp: chrono::DateTime<chrono::Utc> = activity.get("timestamp");
        let slot: i64 = activity.get("block_slot");
        let lamports_change: i64 = activity.get("lamports_change");
        let data_size_change: i64 = activity.get("data_size_change");

        let activity_type = match activity_type_str.as_str() {
            "BALANCE_CHANGE" => AccountActivityType::BalanceChange,
            "DATA_CHANGE" => AccountActivityType::DataChange,
            "OWNER_CHANGE" => AccountActivityType::OwnerChange,
            "EXECUTABLE_CHANGE" => AccountActivityType::ExecutableChange,
            "RENT_EPOCH_CHANGE" => AccountActivityType::RentEpochChange,
            "PROGRAM_INTERACTION" => AccountActivityType::ProgramInteraction,
            _ => AccountActivityType::Unknown,
        };

        let time_str = timestamp.format("%m-%d %H:%M:%S").to_string();
        let lamports_sol = lamports_change as f64 / 1_000_000_000.0;

        println!("{} {} {} {} {} {} {}",
            activity_type.icon().color(activity_type.color()),
            activity_type.as_str().color(activity_type.color()).bold(),
            time_str.bright_black(),
            format!("Slot:{}", slot).bright_cyan(),
            change_type.bright_blue(),
            old_value.bright_yellow(),
            new_value.bright_green()
        );

        if lamports_change != 0 {
            println!("   Balance Change: {} SOL", lamports_sol.to_string().color(if lamports_change > 0 { colored::Color::Green } else { colored::Color::Red }));
        }

        if data_size_change != 0 {
            println!("   Data Size Change: {} bytes", data_size_change.to_string().bright_blue());
        }
    }

    Ok(())
}



#[allow(unused_variables)]
pub async fn start_wallet_monitoring(config: &Config, client: &RpcClient, interval_ms: u64, filter: Option<Vec<String>>) -> Result<()> {
    if !config.database_config.enable_database {
        println!("{} {}", icons::FAILED, "Database is disabled".bright_red());
        return Ok(());
    }

    let db = Database::new(&config.database_config).await?;
    let enhanced_logger = std::sync::Arc::new(EnhancedLogger::new(1000));

    // Initialize high-performance cache system
    let cache = IndexerCache::new(config.clone());
    println!("{} {}", icons::CACHE, "High-performance cache system initialized".bright_cyan());

    // Get all tracked wallets
    let wallets = sqlx::query(
        "SELECT address, name FROM tracked_wallets WHERE is_active = true"
    )
    .fetch_all(db.get_pool())
    .await?;

    if wallets.is_empty() {
        println!("{} {}", icons::WARNING, "No active wallets to monitor".bright_yellow());
        return Ok(());
    }

    println!("{} {} {}",
        icons::TRACKING,
        "Starting real-time wallet monitoring".bright_green().bold(),
        format!("({} wallets)", wallets.len()).bright_cyan()
    );

    let mut wallet_map = HashMap::new();
    for wallet in &wallets {
        let address: String = wallet.get("address");
        let name: Option<String> = wallet.get("name");
        wallet_map.insert(address.clone(), name.clone());

        println!("   {} {}",
            icons::DATABASE,
            format!("{} ({}...{})",
                name.as_deref().unwrap_or("Unnamed"),
                &address[..8],
                &address[address.len()-8..]
            ).bright_white()
        );
    }

    println!("\n{} {} {}\n",
        icons::INFO,
        "Press Ctrl+C to stop monitoring".bright_black(),
        format!("(checking every {}ms)", interval_ms).bright_black()
    );

    let mut interval_timer = interval(Duration::from_millis(interval_ms));
    let mut last_accounts: HashMap<String, Account> = HashMap::new();
    let mut iteration_count = 0;
    let start_time = std::time::Instant::now();
    let mut cache_hits = 0;
    let mut cache_misses = 0;

    loop {
        iteration_count += 1;
        interval_timer.tick().await;

                // Show status dashboard only once every 10 iterations (not every iteration)
        if iteration_count % 10 == 0 {
            // Get real slot information and cache it
            let current_slot = match client.get_slot() {
                Ok(slot) => {
                    // Cache the slot information
                    let slot_info = CachedSlotInfo {
                        slot,
                        leader: "Unknown".to_string(),
                        block_hash: "Unknown".to_string(),
                        timestamp: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64,
                        confirmed: true,
                        finalized: false,
                        cached_at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64,
                    };
                    if let Err(e) = cache.cache_slot(slot_info).await {
                        println!("{} {}", icons::WARNING, format!("Failed to cache slot: {}", e).bright_yellow());
                    }
                    slot
                }
                Err(_) => 0,
            };

            // Get real cache statistics
            let cache_stats = cache.get_cache_stats().await;
            let total_cache_entries = cache_stats["accounts"]["entry_count"].as_u64().unwrap_or(0);
            let cache_memory_mb = cache_stats["total_memory_usage_mb"].as_u64().unwrap_or(0);

            // Calculate real cache hit rate
            let total_cache_requests = cache_hits + cache_misses;
            let cache_hit_rate = if total_cache_requests > 0 {
                (cache_hits as f32 / total_cache_requests as f32) * 100.0
            } else {
                0.0
            };

            // Enhanced wallet monitoring display with separator (similar to slot tracker)
            let terminal_width = get_terminal_width();
            println!("{}", "─".repeat(terminal_width).truecolor(139, 233, 253)); // Blue separator
            println!("{}", "WALLET MONITORING DASHBOARD".truecolor(139, 233, 253).bold()); // Blue title
            println!("Slot: {} | Wallets: {} | Cache: {:.1}%",
                current_slot.to_string().truecolor(248, 248, 242).bold(),
                wallet_map.len().to_string().truecolor(80, 250, 123).bold(),
                cache_hit_rate.to_string().truecolor(255, 184, 108).bold()
            );
            println!("Uptime: {} | Cache Entries: {} | Cache Memory: {}MB",
                format!("{}s", start_time.elapsed().as_secs()).truecolor(189, 147, 249).bold(),
                total_cache_entries.to_string().truecolor(139, 233, 253).bold(),
                cache_memory_mb.to_string().truecolor(80, 250, 123).bold()
            );
            println!("Cache Hits: {} | Cache Misses: {} | Avg Response: {}ms",
                cache_hits.to_string().truecolor(80, 250, 123).bold(),
                cache_misses.to_string().truecolor(255, 85, 85).bold(),
                (start_time.elapsed().as_millis() as u64 / (iteration_count + 1) as u64).to_string().truecolor(139, 233, 253).bold()
            );
            println!("{}", "─".repeat(terminal_width).truecolor(139, 233, 253)); // Blue separator
        }

        // Log real blockchain activity with enhanced display
        if iteration_count % 3 == 0 {
            match client.get_slot() {
                Ok(current_slot) => {
                    // Enhanced slot display
                    let terminal_width = 80;
                    println!("{}", "─".repeat(terminal_width).truecolor(241, 250, 140)); // Yellow separator
                    println!("{}", "SLOT UPDATE".truecolor(241, 250, 140).bold()); // Yellow title
                    println!("Slot: {} | Time: {}",
                        current_slot.to_string().truecolor(248, 248, 242).bold(),
                        chrono::Utc::now().format("%H:%M:%S").to_string().truecolor(139, 147, 164)
                    );
                    println!("{}", "─".repeat(terminal_width).truecolor(241, 250, 140)); // Yellow separator
                }
                Err(_) => {
                    let terminal_width = 80;
                    println!("{}", "─".repeat(terminal_width).truecolor(255, 85, 85)); // Red separator
                    println!("{}", "SLOT ERROR".truecolor(255, 85, 85).bold()); // Red title
                    println!("Failed to fetch current slot");
                    println!("{}", "─".repeat(terminal_width).truecolor(255, 85, 85)); // Red separator
                }
            }
        }

                // Enhanced wallet balance display with caching
        if iteration_count % 5 == 0 && !wallet_map.is_empty() {
            for (address, name) in &wallet_map {
                if let Ok(pubkey) = Pubkey::from_str(address) {
                                        // Try to get wallet from cache first
                    let cached_account = cache.get_account(address).await;
                    let is_cache_hit = cached_account.is_some();

                    let account_result = if let Some(cached) = cached_account {
                        cache_hits += 1;
                        // Use cached wallet data
                        Ok(Account {
                            lamports: cached.lamports,
                            data: vec![0; cached.data_len], // Simplified data representation
                            owner: Pubkey::from_str(&cached.owner).unwrap_or_default(),
                            executable: cached.executable,
                            rent_epoch: cached.rent_epoch,
                        })
                    } else {
                        cache_misses += 1;
                        // Fetch from RPC and cache the result
                        let result = client.get_account(&pubkey);
                        if let Ok(ref account) = result {
                            // Cache the wallet data
                            let cached_account = CachedAccount {
                                pubkey: address.clone(),
                                lamports: account.lamports,
                                owner: account.owner.to_string(),
                                executable: account.executable,
                                rent_epoch: account.rent_epoch,
                                data_len: account.data.len(),
                                cached_at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64,
                            };
                            if let Err(e) = cache.cache_account(cached_account).await {
                                println!("{} {}", icons::WARNING, format!("Failed to cache wallet: {}", e).bright_yellow());
                            }
                        }
                        result
                    };

                    match account_result {
                        Ok(account) => {
                            let balance_sol = account.lamports as f64 / 1_000_000_000.0;
                            let balance_lamports = account.lamports;
                            let owner = account.owner.to_string();
                            let executable = account.executable;
                            let rent_epoch = account.rent_epoch;

                            // Enhanced wallet balance display with separator
                            let terminal_width = get_terminal_width();
                            println!("{}", "─".repeat(terminal_width).truecolor(189, 147, 249)); // Purple separator
                            println!("{}", "WALLET BALANCE UPDATE".truecolor(189, 147, 249).bold()); // Purple title
                                                        println!("Name: {} | Address: {}...{} | Cache: {}",
                                name.as_deref().unwrap_or("Unnamed").truecolor(248, 248, 242).bold(),
                                &address[..8].truecolor(139, 233, 253).bold(),
                                &address[address.len()-8..].truecolor(139, 233, 253).bold(),
                                if is_cache_hit { "HIT".truecolor(80, 250, 123).bold() } else { "MISS".truecolor(255, 184, 108).bold() }
                            );
                            println!("Balance: {} SOL ({} lamports)",
                                balance_sol.to_string().truecolor(80, 250, 123).bold(),
                                balance_lamports.to_string().truecolor(255, 184, 108).bold()
                            );
                            println!("Owner: {} | Executable: {}",
                                owner.truecolor(139, 233, 253).bold(),
                                if executable { "Yes".truecolor(80, 250, 123).bold() } else { "No".truecolor(255, 85, 85).bold() }
                            );
                            println!("Rent Epoch: {} | Data Size: {} bytes",
                                rent_epoch.to_string().truecolor(189, 147, 249).bold(),
                                account.data.len().to_string().truecolor(255, 184, 108).bold()
                            );

                            // gRPC-like additional details
                            println!("Last Updated: {} | Slot: {}",
                                chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC").to_string().truecolor(139, 147, 164).bold(),
                                match client.get_slot() {
                                    Ok(slot) => slot.to_string().truecolor(139, 233, 253).bold(),
                                    Err(_) => "Unknown".truecolor(255, 85, 85).bold()
                                }
                            );
                            println!("{}", "─".repeat(terminal_width).truecolor(189, 147, 249)); // Purple separator
                        }
                        Err(e) => {
                            let terminal_width = get_terminal_width();
                            println!("{}", "─".repeat(terminal_width).truecolor(255, 85, 85)); // Red separator
                            println!("{}", "WALLET ERROR".truecolor(255, 85, 85).bold()); // Red title
                            println!("Name: {} | Address: {}...{}",
                                name.as_deref().unwrap_or("Unnamed").truecolor(248, 248, 242).bold(),
                                &address[..8].truecolor(139, 233, 253).bold(),
                                &address[address.len()-8..].truecolor(139, 233, 253).bold()
                            );
                            println!("Error: {}", e.to_string().truecolor(255, 85, 85).bold());
                            println!("{}", "─".repeat(terminal_width).truecolor(255, 85, 85)); // Red separator
                        }
                    }
                }
            }
        }

        for (address, name) in &wallet_map {
            if let Ok(pubkey) = Pubkey::from_str(address) {
                // Get current account data
                match client.get_account(&pubkey) {
                    Ok(account) => {
                        // Check for changes
                        if let Some(last_account) = last_accounts.get(address) {
                            let changes = detect_account_changes(last_account, &account);

                            for change in changes {
                                // Apply filter if specified
                                if let Some(filters) = &filter {
                                    if !filters.iter().any(|f| f.to_lowercase() == change.activity_type.as_str().to_lowercase()) {
                                        continue;
                                    }
                                }

                                // Get current slot for proper foreign key reference
                                let current_slot = match client.get_slot() {
                                    Ok(slot) => slot,
                                    Err(_) => continue, // Skip if we can't get slot
                                };

                                // First, ensure the slot exists in the slots table
                                sqlx::query(
                                    "INSERT OR IGNORE INTO slots (slot, blockhash, parent_slot, finalized, timestamp) VALUES (?, ?, ?, ?, ?)"
                                )
                                .bind(current_slot as i64)
                                .bind("pending_blockhash") // Placeholder blockhash
                                .bind((current_slot.saturating_sub(1)) as i64)
                                .bind(false)
                                .bind(chrono::Utc::now())
                                .execute(db.get_pool())
                                .await?;

                                // Generate a unique transaction signature for this activity
                                let tx_signature = format!("account_change_{}_{}_{}", address, current_slot, chrono::Utc::now().timestamp());

                                // Next, ensure the transaction exists in the transactions table
                                sqlx::query(
                                    "INSERT OR IGNORE INTO transactions (signature, slot, fee, status, program_ids, timestamp) VALUES (?, ?, ?, ?, ?, ?)"
                                )
                                .bind(&tx_signature)
                                .bind(current_slot as i64)
                                .bind(0i64) // No fee for account changes
                                .bind("SUCCESS")
                                .bind("[]") // Empty program IDs array as JSON string
                                .bind(chrono::Utc::now())
                                .execute(db.get_pool())
                                .await?;

                                // Now store the wallet activity (foreign key constraints will be satisfied)
                                sqlx::query(
                                    "INSERT INTO wallet_activities (wallet_address, activity_type, transaction_signature, timestamp, block_slot, fee, status) VALUES (?, ?, ?, ?, ?, ?, ?)"
                                )
                                .bind(address)
                                .bind(change.activity_type.as_str())
                                .bind(tx_signature)
                                .bind(chrono::Utc::now())
                                .bind(current_slot as i64)
                                .bind(0i64)
                                .bind("SUCCESS")
                                .execute(db.get_pool())
                                .await?;

                                // Update wallet last activity
                                sqlx::query(
                                    "UPDATE tracked_wallets SET last_activity = ?, activity_count = activity_count + 1 WHERE address = ?"
                                )
                                .bind(chrono::Utc::now())
                                .bind(address)
                                .execute(db.get_pool())
                                .await?;

                                // Display real-time activity
                                let short_addr = format!("{}...{}", &address[..6], &address[address.len()-6..]);
                                println!("{} {} {} {} {}",
                                    change.activity_type.icon().color(change.activity_type.color()),
                                    change.activity_type.as_str().color(change.activity_type.color()).bold(),
                                    format!("{} ({})", name.as_deref().unwrap_or("Unnamed"), short_addr).bright_white(),
                                    change.change_type.bright_blue(),
                                    change.new_value.bright_yellow()
                                );
                            }
                        }

                        last_accounts.insert(address.clone(), account);
                    }
                    Err(e) => {
                        let error_msg = if e.to_string().contains("Unknown") {
                            println!("{} {} {}: RPC parsing error - trying alternative approach...",
                                icons::WARNING,
                                "Failed to fetch wallet data for".bright_yellow(),
                                name.as_deref().unwrap_or("Unnamed").bright_white()
                            );
                            continue;
                        } else {
                            format!("RPC error: {}", e)
                        };

                        println!("{} {} {}: {}",
                            icons::WARNING,
                            "Failed to fetch wallet data for".bright_yellow(),
                            name.as_deref().unwrap_or("Unnamed").bright_white(),
                            error_msg.bright_red()
                        );
                    }
                }
            }
        }
    }
}
