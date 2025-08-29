use colored::*;
use std::io::{self, Write};
use std::thread;
use std::time::Duration;
use crate::logger::icons;

/// ASCII Art and Animations for the Solana Indexer CLI
pub struct CliAnimations;

impl CliAnimations {
    /// Cool Solana-themed startup banner with ASCII art
    pub fn show_startup_banner() {
        println!();

        // Solana-inspired ASCII art with gradient colors
        let lines = vec![
            "  .-')                             ('-.         .-') _    ('-.     ",
            " ( OO ).                          ( OO ).-.    ( OO ) )  ( OO ).-. ",
            "(_)---\\_) .-'),-----.  ,--.       / . --. /,--./ ,--,'   / . --. / ",
            "/    _ | ( OO'  .-.  ' |  |.-')   | \\-.  \\ |   \\ |  |\\   | \\-.  \\  ",
            "\\  :` `. /   |  | |  | |  | OO ).-'-'  |  ||    \\|  | ).-'-'  |  | ",
            " '..`''.)\\_) |  |\\|  | |  |`-' | \\| |_.'  ||  .     |/  \\| |_.'  | ",
            ".-._)   \\  \\ |  | |  |(|  '---.'  |  .-.  ||  |\\    |    |  .-.  | ",
            "\\       /   `'  '-'  ' |      |   |  | |  ||  | \\   |    |  | |  | ",
            " `-----'      `-----'  `------'   `--' `--'`--'  `--'    `--' `--' ",
        ];

        // Display ASCII art with Solana gradient colors using truecolor
        for (i, line) in lines.iter().enumerate() {
            match i {
                0 => println!("{}", line.truecolor(220, 38, 127)),   // Solana pink/purple
                1 => println!("{}", line.truecolor(156, 39, 176)),   // Purple
                2 => println!("{}", line.truecolor(103, 58, 183)),   // Deep purple
                3 => println!("{}", line.truecolor(63, 81, 181)),    // Indigo
                4 => println!("{}", line.truecolor(33, 150, 243)),   // Blue
                5 => println!("{}", line.truecolor(0, 188, 212)),    // Cyan
                6 => println!("{}", line.truecolor(0, 200, 83)),     // Solana green
                7 => println!("{}", line.truecolor(76, 175, 80)),    // Light green
                8 => println!("{}", line.truecolor(139, 195, 74)),   // Lime
                _ => println!("{}", line.bright_cyan()),             // Default fallback
            }
        }

        println!();
        println!("{}{}{}",
            "  ".bright_cyan(),
            " SOLANA BLOCKCHAIN INDEXER ".bright_white().on_magenta(),
            " ".bright_cyan()
        );
        println!("{}{}{}",
            "  ".bright_cyan(),
            "High-performance real-time blockchain data streaming".truecolor(0, 200, 83),
            ""
        );
        println!();

        // Show feature highlights with nerd icons and Solana colors
        println!("  {}  {}", icons::ROCKET.truecolor(220, 38, 127), "Features:".bright_white());
        println!("    {}  {}", icons::CLOCK.truecolor(0, 200, 83), "Real-time slot & transaction tracking".bright_cyan());
        println!("    {}  {}", icons::NETWORK.truecolor(103, 58, 183), "gRPC streaming with Protocol Buffers".bright_cyan());
        println!("    {}  {}", icons::DATABASE.truecolor(33, 150, 243), "Multi-blockchain support (Solana & Flow)".bright_cyan());
        println!("    {}  {}", icons::CACHE.truecolor(0, 188, 212), "Advanced caching & IPFS integration".bright_cyan());

        println!("    {}  {}", icons::CONNECTION.truecolor(156, 39, 176), "Webhook support for QuickNode & Yellowstone".bright_cyan());
        println!();

        // Animated loading dots
        print!("{}", "    Initializing".bright_white().bold());
        for _ in 0..6 {
            print!("{}", ".".truecolor(220, 38, 127));
            io::stdout().flush().unwrap();
            thread::sleep(Duration::from_millis(300));
        }
        println!("{}", " Ready!".truecolor(0, 200, 83).bold());
        println!();
    }



    /// Animated progress bar
    pub fn show_progress_bar(message: &str, current: usize, total: usize) {
        let width = 40;
        let progress = (current as f32 / total as f32 * width as f32) as usize;
        let percentage = (current as f32 / total as f32 * 100.0) as usize;

        let mut bar = String::new();
        for i in 0..width {
            if i < progress {
                bar.push('█');
            } else if i == progress {
                bar.push('▓');
            } else {
                bar.push('░');
            }
        }

        print!("\r{} [{}] {}% ({}/{})",
            message.bright_white().bold(),
            bar.bright_cyan(),
            percentage.to_string().bright_yellow().bold(),
            current.to_string().bright_green(),
            total.to_string().bright_blue()
        );
        io::stdout().flush().unwrap();

        if current >= total {
            println!(" {}", "Complete!".bright_green().bold());
        }
    }



    /// Cool wallet display with ASCII art
    pub fn show_wallet_art(address: &str, name: &str, balance: Option<f64>) {
        let wallet_art = format!(r#"
    +-------------------------------------+
    |  {} WALLET INFORMATION              |
    +-------------------------------------+"#, icons::WALLET);

        println!("{}", wallet_art.bright_blue());
        println!("    |  {} {}   |", "Name:".bright_yellow().bold(), name.bright_white().bold());
        println!("    |  {} {}...{} |",
            "Address:".bright_yellow().bold(),
            &address[..8].bright_cyan(),
            &address[address.len()-8..].bright_cyan()
        );

        if let Some(bal) = balance {
            println!("    |  {} {} SOL        |",
                "Balance:".bright_yellow().bold(),
                format!("{:.4}", bal).bright_green().bold()
            );
        }

        println!("    +-------------------------------------+");
        println!();
    }

    /// Cool account display with ASCII art
    pub fn show_account_art(address: &str, name: &str, program_id: Option<&str>, balance: Option<f64>) {
        let account_art = format!(r#"
    +-------------------------------------+
    |  {} ACCOUNT INFORMATION             |
    +-------------------------------------+"#, icons::DATABASE);

        println!("{}", account_art.bright_green());
        println!("    |  {} {}   |", "Name:".bright_yellow().bold(), name.bright_white().bold());
        println!("    |  {} {}...{} |",
            "Address:".bright_yellow().bold(),
            &address[..8].bright_cyan(),
            &address[address.len()-8..].bright_cyan()
        );

        if let Some(program) = program_id {
            println!("    |  {} {}...{} |",
                "Program:".bright_yellow().bold(),
                &program[..8].bright_blue(),
                &program[program.len()-8..].bright_blue()
            );
        }

        if let Some(bal) = balance {
            println!("    |  {} {} SOL        |",
                "Balance:".bright_yellow().bold(),
                format!("{:.4}", bal).bright_green().bold()
            );
        }

        println!("    +-------------------------------------+");
        println!();
    }

    /// Animated connection status
    pub fn show_connection_animation(rpc_url: &str) {
        let connection_frames = [
            format!("{} Connecting    ", icons::CONNECTION),
            format!("{} Connecting.   ", icons::CONNECTION),
            format!("{} Connecting..  ", icons::CONNECTION),
            format!("{} Connecting... ", icons::CONNECTION),
            format!("{} Connected!    ", icons::COMPLETE)
        ];
        let connection_frames: Vec<&str> = connection_frames.iter().map(|s| s.as_str()).collect();

        for (i, frame) in connection_frames.iter().enumerate() {
            print!("\r{} {}",
                frame.bright_yellow().bold(),
                rpc_url.bright_blue()
            );
            io::stdout().flush().unwrap();

            if i < connection_frames.len() - 1 {
                thread::sleep(Duration::from_millis(500));
            } else {
                thread::sleep(Duration::from_millis(1000));
                println!();
            }
        }
    }





    /// Status dashboard display
    pub fn show_status_dashboard(stats: &StatusStats) {
        let dashboard = format!(r#"
    +-----------------------------------------------------------------+
    |                    {} SYSTEM STATUS DASHBOARD                   |
    +-----------------------------------------------------------------+"#, icons::DASHBOARD);

        println!("{}", dashboard.bright_blue());

        println!("    | {} Wallets Tracked: {} | {} RPC Status: {} | {} Cache Hit: {}% |",
            icons::WALLET,
            stats.wallets_tracked.to_string().bright_green().bold(),
            icons::NETWORK,
            if stats.rpc_connected { "Online".bright_green().bold() } else { "Offline".bright_red().bold() },
            icons::CACHE,
            stats.cache_hit_rate.to_string().bright_yellow().bold()
        );

        println!("    | {} Transactions: {} | {} Avg Response: {}ms | {} Uptime: {} |",
            icons::TRANSACTION,
            stats.total_transactions.to_string().bright_cyan().bold(),
            icons::LIGHTNING,
            stats.avg_response_time.to_string().bright_blue().bold(),
            icons::ROCKET,
            stats.uptime.bright_magenta().bold()
        );

        println!("    +-----------------------------------------------------------------+");
        println!();
    }

    /// Interactive menu selector
    pub fn show_interactive_menu(title: &str, options: &[&str]) -> usize {
        loop {
            // Clear screen and show menu
            print!("\x1B[2J\x1B[1;1H"); // Clear screen and move cursor to top

            println!("{}", format!("    {} {}", icons::STAR, title).truecolor(220, 38, 127).bold());
            println!("    {}", "-".repeat(60).truecolor(103, 58, 183));
            println!();

            for (i, option) in options.iter().enumerate() {
                println!("    {} {}. {}",
                    icons::ROCKET.truecolor(0, 200, 83),
                    (i + 1).to_string().truecolor(220, 38, 127).bold(),
                    option.bright_white()
                );
            }

            println!();
            println!("    {}", "-".repeat(60).truecolor(103, 58, 183));
            print!("    {} ", format!("Select option (1-{}) or 'q' to quit: ", options.len()).truecolor(0, 200, 83));
            io::stdout().flush().unwrap();

            // Read user input
            let mut input = String::new();
            match std::io::stdin().read_line(&mut input) {
                Ok(_) => {
                    let input = input.trim();

                    // Handle quit
                    if input.eq_ignore_ascii_case("q") || input.eq_ignore_ascii_case("quit") {
                        return options.len(); // Return index that represents "exit"
                    }

                    // Parse number input
                    match input.parse::<usize>() {
                        Ok(num) if num >= 1 && num <= options.len() => {
                            return num - 1; // Convert to 0-based index
                        }
                        _ => {
                            println!("    {} {}",
                                icons::ERROR.truecolor(220, 38, 127),
                                format!("Invalid option '{}'. Please enter a number between 1 and {} or 'q' to quit.",
                                    input, options.len()).bright_red()
                            );
                            println!("    {} ", "Press Enter to continue...".bright_black());
                            let mut _temp = String::new();
                            std::io::stdin().read_line(&mut _temp).unwrap();
                        }
                    }
                }
                Err(e) => {
                    println!("    {} Error reading input: {}", icons::ERROR.bright_red(), e);
                    return options.len(); // Return exit index on error
                }
            }
        }
    }

    /// Cool error display
    pub fn show_error(error_type: &str, message: &str) {
        let error_box = format!(r#"
    +------------------------------------------------------------------+
    | {}  ERROR: {}                                                   |
    +------------------------------------------------------------------+
    | {}                                                              |
    +------------------------------------------------------------------+"#,
            icons::ERROR, error_type, message);

        println!("{}", error_box.bright_red());
    }

    /// Success notification
    pub fn show_success(message: &str) {
        println!("    {} {}", icons::SUCCESS.bright_green(), message.bright_green().bold());
    }


}

/// Statistics structure for dashboard
pub struct StatusStats {
    pub wallets_tracked: usize,
    pub rpc_connected: bool,
    pub cache_hit_rate: f32,
    pub total_transactions: usize,
    pub avg_response_time: u64,
    pub uptime: String,
}

impl Default for StatusStats {
    fn default() -> Self {
        Self {
            wallets_tracked: 0,
            rpc_connected: false,
            cache_hit_rate: 0.0,
            total_transactions: 0,
            avg_response_time: 0,
            uptime: "00:00:00".to_string(),
        }
    }
}