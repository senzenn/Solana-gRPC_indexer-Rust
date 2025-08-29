use colored::*;
use std::sync::{Arc, Mutex};
use std::collections::VecDeque;
use chrono::{DateTime, Utc};
use std::io::{self, Write};

#[derive(Clone, Debug)]
pub struct LogEntry {
    pub timestamp: DateTime<Utc>,
    pub log_type: LogType,
    pub message: String,
    pub details: LogDetails,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LogType {
    SlotUpdate,
    AccountUpdate,
    TxConfirmed,
    BlockUpdate,
    SystemInfo,
    Error,
    Success,
}

#[derive(Clone, Debug)]
pub struct LogDetails {
    pub slot: Option<u64>,
    pub signature: Option<String>,
    pub pubkey: Option<String>,
    pub balance: Option<u64>,
    pub fee: Option<u64>,
    pub leader: Option<String>,
}

impl LogType {
    pub fn icon(&self) -> &'static str {
        match self {
            LogType::SlotUpdate => "",     // nf-fa-cube
            LogType::AccountUpdate => "",  // nf-fa-user
            LogType::TxConfirmed => "",    // nf-fa-check
            LogType::BlockUpdate => "",    // nf-fa-cubes
            LogType::SystemInfo => "",     // nf-fa-info
            LogType::Error => "",          // nf-fa-times
            LogType::Success => "",        // nf-fa-check_circle
        }
    }

    pub fn color(&self) -> Color {
        match self {
            LogType::SlotUpdate => Color::Yellow,
            LogType::AccountUpdate => Color::Green,
            LogType::TxConfirmed => Color::Blue,
            LogType::BlockUpdate => Color::Magenta,
            LogType::SystemInfo => Color::Cyan,
            LogType::Error => Color::Red,
            LogType::Success => Color::BrightGreen,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            LogType::SlotUpdate => "SLOT UPDATE",
            LogType::AccountUpdate => "ACCOUNT UPDATE",
            LogType::TxConfirmed => "TX CONFIRMED",
            LogType::BlockUpdate => "BLOCK UPDATE",
            LogType::SystemInfo => "SYSTEM INFO",
            LogType::Error => "ERROR",
            LogType::Success => "SUCCESS",
        }
    }
}

pub struct EnhancedLogger {
    logs: Arc<Mutex<VecDeque<LogEntry>>>,
    max_entries: usize,
}

impl EnhancedLogger {
    pub fn new(max_entries: usize) -> Self {
        Self {
            logs: Arc::new(Mutex::new(VecDeque::new())),
            max_entries,
        }
    }

    pub fn log_slot_update(&self, slot: u64, leader: &str) {
        let entry = LogEntry {
            timestamp: Utc::now(),
            log_type: LogType::SlotUpdate,
            message: format!("slot: {} | leader: {}", slot, leader),
            details: LogDetails {
                slot: Some(slot),
                leader: Some(leader.to_string()),
                signature: None,
                pubkey: None,
                balance: None,
                fee: None,
            },
        };
        self.store_and_print(entry);
    }

    pub fn log_account_update(&self, pubkey: &str, balance: u64, slot: u64) {
        let short_pubkey = if pubkey.len() > 16 {
            format!("{}...{}", &pubkey[..8], &pubkey[pubkey.len()-3..])
        } else {
            pubkey.to_string()
        };

        let entry = LogEntry {
            timestamp: Utc::now(),
            log_type: LogType::AccountUpdate,
            message: format!("pubkey: {} | balance: {} lamports | slot: {}",
                short_pubkey, balance, slot),
            details: LogDetails {
                slot: Some(slot),
                pubkey: Some(pubkey.to_string()),
                balance: Some(balance),
                signature: None,
                leader: None,
                fee: None,
            },
        };
        self.store_and_print(entry);
    }

    pub fn log_tx_confirmed(&self, signature: &str, slot: u64, fee: u64) {
        let short_sig = if signature.len() > 16 {
            format!("{}...{}", &signature[..8], &signature[signature.len()-3..])
        } else {
            signature.to_string()
        };

        let entry = LogEntry {
            timestamp: Utc::now(),
            log_type: LogType::TxConfirmed,
            message: format!("sig: {} | slot: {} | fee: {} lamports",
                short_sig, slot, fee),
            details: LogDetails {
                slot: Some(slot),
                signature: Some(signature.to_string()),
                fee: Some(fee),
                pubkey: None,
                balance: None,
                leader: None,
            },
        };
        self.store_and_print(entry);
    }

    pub fn log_system_info(&self, message: &str) {
        let entry = LogEntry {
            timestamp: Utc::now(),
            log_type: LogType::SystemInfo,
            message: message.to_string(),
            details: LogDetails {
                slot: None,
                signature: None,
                pubkey: None,
                balance: None,
                fee: None,
                leader: None,
            },
        };
        self.store_and_print(entry);
    }

    pub fn log_error(&self, message: &str) {
        let entry = LogEntry {
            timestamp: Utc::now(),
            log_type: LogType::Error,
            message: message.to_string(),
            details: LogDetails {
                slot: None,
                signature: None,
                pubkey: None,
                balance: None,
                fee: None,
                leader: None,
            },
        };
        self.store_and_print(entry);
    }

    pub fn log_success(&self, message: &str) {
        let entry = LogEntry {
            timestamp: Utc::now(),
            log_type: LogType::Success,
            message: message.to_string(),
            details: LogDetails {
                slot: None,
                signature: None,
                pubkey: None,
                balance: None,
                fee: None,
                leader: None,
            },
        };
        self.store_and_print(entry);
    }

    fn store_and_print(&self, entry: LogEntry) {
        self.print_log(&entry);

        let mut logs = self.logs.lock().unwrap();
        logs.push_back(entry);

        if logs.len() > self.max_entries {
            logs.pop_front();
        }
    }

    fn print_log(&self, entry: &LogEntry) {
        let timestamp = entry.timestamp.format("[%Y-%m-%dT%H:%M:%S.%3fZ]");
        let log_type_colored = entry.log_type.name().color(entry.log_type.color()).bold();

        // Create the log line with proper spacing and colors
        print!("{} {} {} {}",
            timestamp.to_string().bright_black(),
            log_type_colored,
            "|".bright_black(),
            entry.message.color(entry.log_type.color())
        );

        println!(); // New line
        io::stdout().flush().unwrap();
    }

    pub fn get_logs(&self) -> Vec<LogEntry> {
        self.logs.lock().unwrap().iter().cloned().collect()
    }

    pub fn clear_logs(&self) {
        self.logs.lock().unwrap().clear();
    }

    // Enhanced logging with visual effects
    pub fn log_with_effect(&self, log_type: LogType, message: &str, show_border: bool) {
        if show_border {
            self.print_border();
        }

        let entry = LogEntry {
            timestamp: Utc::now(),
            log_type,
            message: message.to_string(),
            details: LogDetails {
                slot: None,
                signature: None,
                pubkey: None,
                balance: None,
                fee: None,
                leader: None,
            },
        };

        self.store_and_print(entry);

        if show_border {
            self.print_border();
        }
    }

    fn print_border(&self) {
        println!("{}", "─".repeat(80).bright_black());
    }

    // Animated logging effect
    pub fn log_with_animation(&self, log_type: LogType, message: &str) {
        use std::thread;
        use std::time::Duration;

        // Show loading dots
        print!("{} {} {} ",
            Utc::now().format("[%Y-%m-%dT%H:%M:%S.%3fZ]").to_string().bright_black(),
            log_type.name().color(log_type.color()).bold(),
            "|".bright_black()
        );

        for _ in 0..3 {
            print!("{}", ".".color(log_type.color()));
            io::stdout().flush().unwrap();
            thread::sleep(Duration::from_millis(200));
        }

        // Clear the dots and print the actual message
        print!("\r{} {} {} {}\n",
            Utc::now().format("[%Y-%m-%dT%H:%M:%S.%3fZ]").to_string().bright_black(),
            log_type.name().color(log_type.color()).bold(),
            "|".bright_black(),
            message.color(log_type.color())
        );

        io::stdout().flush().unwrap();
    }
}


