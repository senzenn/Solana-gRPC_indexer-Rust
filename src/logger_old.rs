use colored::*;
use std::sync::{Arc, Mutex};
use std::collections::VecDeque;
use chrono::{DateTime, Utc};

#[derive(Clone, Debug)]
pub struct LogEntry {
    pub timestamp: DateTime<Utc>,
    pub level: LogLevel,
    pub message: String,
    pub module: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
    Success,
    Debug,
    Trace,
}

impl LogLevel {
    pub fn icon(&self) -> &'static str {
        match self {
            LogLevel::Info => icons::INFO,
            LogLevel::Warn => icons::WARNING,
            LogLevel::Error => icons::ERROR,
            LogLevel::Success => icons::SUCCESS,
            LogLevel::Debug => icons::DEBUG,
            LogLevel::Trace => icons::SEARCH,
        }
    }

    pub fn color(&self) -> Color {
        match self {
            LogLevel::Info => Color::Cyan,
            LogLevel::Warn => Color::Yellow,
            LogLevel::Error => Color::Red,
            LogLevel::Success => Color::Green,
            LogLevel::Debug => Color::Magenta,
            LogLevel::Trace => Color::White,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
            LogLevel::Success => "SUCCESS",
            LogLevel::Debug => "DEBUG",
            LogLevel::Trace => "TRACE",
        }
    }
}

pub struct NerdLogger {
    logs: Arc<Mutex<VecDeque<LogEntry>>>,
    max_entries: usize,
}

impl NerdLogger {
    pub fn new(max_entries: usize) -> Self {
        Self {
            logs: Arc::new(Mutex::new(VecDeque::new())),
            max_entries,
        }
    }

    pub fn log(&self, level: LogLevel, message: String, module: String) {
        let entry = LogEntry {
            timestamp: Utc::now(),
            level: level.clone(),
            message: message.clone(),
            module: module.clone(),
        };

        // Print to console with beautiful formatting
        self.print_log(&entry);

        // Store in memory
        let mut logs = self.logs.lock().unwrap();
        logs.push_back(entry);

        if logs.len() > self.max_entries {
            logs.pop_front();
        }
    }

    fn print_log(&self, entry: &LogEntry) {
        let timestamp = entry.timestamp.format("%H:%M:%S");
        let icon = entry.level.icon();
        let level_name = entry.level.name();
        let module_short = if entry.module.len() > 12 {
            format!("{}...", &entry.module[..9])
        } else {
            entry.module.clone()
        };

        // Enhanced log formatting with better visual hierarchy
        match entry.level {
            LogLevel::Info => {
                println!("{} {} {} {} {}",
                    format!("[{}]", timestamp).bright_black(),
                    icon.bright_cyan().bold(),
                    format!("{:<7}", level_name).bright_cyan().bold(),
                    format!("[{}]", module_short).bright_black(),
                    entry.message.white()
                );
            }
            LogLevel::Success => {
                println!("{} {} {} {} {}",
                    format!("[{}]", timestamp).bright_black(),
                    icon.bright_green().bold(),
                    format!("{:<7}", level_name).bright_green().bold(),
                    format!("[{}]", module_short).bright_black(),
                    entry.message.bright_white().bold()
                );
            }
            LogLevel::Warn => {
                println!("{} {} {} {} {}",
                    format!("[{}]", timestamp).bright_black(),
                    icon.bright_yellow().bold(),
                    format!("{:<7}", level_name).bright_yellow().bold(),
                    format!("[{}]", module_short).bright_black(),
                    entry.message.yellow()
                );
            }
            LogLevel::Error => {
                println!("{} {} {} {} {}",
                    format!("[{}]", timestamp).bright_black(),
                    icon.bright_red().bold(),
                    format!("{:<7}", level_name).bright_red().bold(),
                    format!("[{}]", module_short).bright_black(),
                    entry.message.bright_red().bold()
                );
            }
            LogLevel::Debug => {
                println!("{} {} {} {} {}",
                    format!("[{}]", timestamp).bright_black(),
                    icon.bright_magenta().bold(),
                    format!("{:<7}", level_name).bright_magenta().bold(),
                    format!("[{}]", module_short).bright_black(),
                    entry.message.magenta()
                );
            }
            LogLevel::Trace => {
                println!("{} {} {} {} {}",
                    format!("[{}]", timestamp).bright_black(),
                    icon.bright_white(),
                    format!("{:<7}", level_name).bright_white().bold(),
                    format!("[{}]", module_short).bright_black(),
                    entry.message.bright_black()
                );
            }
        }
    }

    pub fn info(&self, message: &str, module: &str) {
        self.log(LogLevel::Info, message.to_string(), module.to_string());
    }

    pub fn success(&self, message: &str, module: &str) {
        self.log(LogLevel::Success, message.to_string(), module.to_string());
    }

    pub fn warn(&self, message: &str, module: &str) {
        self.log(LogLevel::Warn, message.to_string(), module.to_string());
    }

    pub fn error(&self, message: &str, module: &str) {
        self.log(LogLevel::Error, message.to_string(), module.to_string());
    }

    pub fn debug(&self, message: &str, module: &str) {
        self.log(LogLevel::Debug, message.to_string(), module.to_string());
    }

    pub fn get_logs(&self) -> Vec<LogEntry> {
        self.logs.lock().unwrap().iter().cloned().collect()
    }

    pub fn get_logs_arc(&self) -> Arc<Mutex<VecDeque<LogEntry>>> {
        Arc::clone(&self.logs)
    }
}

// Essential Nerd Font Icons Collection (only used icons)
pub mod icons {
    // Blockchain & Crypto
    pub const WALLET: &str = "";          // nf-fa-wallet

    // Network & Infrastructure
    pub const NETWORK: &str = "";         // nf-fa-globe
    pub const SERVER: &str = "";          // nf-fa-server
    pub const DATABASE: &str = "";        // nf-fa-database
    pub const CACHE: &str = "";           // nf-fa-bolt
    pub const CONNECTION: &str = "";      // nf-fa-plug

    // Monitoring & Analytics
    pub const METRICS: &str = "";         // nf-fa-chart_line
    pub const TRACKING: &str = "";        // nf-fa-crosshairs
    pub const MONITOR: &str = "";         // nf-fa-desktop
    pub const CHART: &str = "";           // nf-fa-chart_bar
    pub const DASHBOARD: &str = "";       // nf-fa-tachometer_alt

    // Blockchain Specific
    pub const LEADER: &str = "";          // nf-fa-crown
    pub const SLOT: &str = "";            // nf-fa-cube
    pub const TRANSACTION: &str = "";     // nf-fa-credit_card
    pub const FLOW: &str = "";            // nf-fa-water

    // System & Operations
    pub const CONFIG: &str = "";          // nf-fa-cog
    pub const TEST: &str = "";            // nf-fa-flask
    pub const CLOCK: &str = "";           // nf-fa-clock
    pub const COMPLETE: &str = "";        // nf-fa-check
    pub const FAILED: &str = "";          // nf-fa-times
    pub const WARNING: &str = "";         // nf-fa-exclamation_triangle
    pub const ERROR: &str = "";           // nf-fa-exclamation_circle
    pub const SUCCESS: &str = "";         // nf-fa-check_circle

    // User Interface
    pub const INFO: &str = "";            // nf-fa-info
    pub const DEBUG: &str = "";           // nf-fa-bug
    pub const HELP: &str = "";            // nf-fa-question_circle
    pub const ROCKET: &str = "";          // nf-fa-rocket
    pub const STAR: &str = "";            // nf-fa-star
    pub const LIGHTNING: &str = "";       // nf-fa-bolt

    // Time & Organization
    pub const TIME: &str = "";            // nf-fa-clock
    pub const CALENDAR: &str = "";        // nf-fa-calendar

    // Search & Navigation
    pub const SEARCH: &str = "";          // nf-fa-search
    pub const LIST: &str = "";            // nf-fa-list

    // Development & Tools
    pub const CODE: &str = "";            // nf-fa-code
    pub const KEY: &str = "";             // nf-fa-key
}

// Convenience macros for logging
#[macro_export]
macro_rules! log_info {
    ($logger:expr, $msg:expr) => {
        $logger.info($msg, module_path!())
    };
    ($logger:expr, $fmt:expr, $($arg:tt)*) => {
        $logger.info(&format!($fmt, $($arg)*), module_path!())
    };
}

#[macro_export]
macro_rules! log_success {
    ($logger:expr, $msg:expr) => {
        $logger.success($msg, module_path!())
    };
    ($logger:expr, $fmt:expr, $($arg:tt)*) => {
        $logger.success(&format!($fmt, $($arg)*), module_path!())
    };
}

#[macro_export]
macro_rules! log_warn {
    ($logger:expr, $msg:expr) => {
        $logger.warn($msg, module_path!())
    };
    ($logger:expr, $fmt:expr, $($arg:tt)*) => {
        $logger.warn(&format!($fmt, $($arg)*), module_path!())
    };
}

#[macro_export]
macro_rules! log_error {
    ($logger:expr, $msg:expr) => {
        $logger.error($msg, module_path!())
    };
    ($logger:expr, $fmt:expr, $($arg:tt)*) => {
        $logger.error(&format!($fmt, $($arg)*), module_path!())
    };
}

#[macro_export]
macro_rules! log_debug {
    ($logger:expr, $msg:expr) => {
        $logger.debug($msg, module_path!())
    };
    ($logger:expr, $fmt:expr, $($arg:tt)*) => {
        $logger.debug(&format!($fmt, $($arg)*), module_path!())
    };
}
