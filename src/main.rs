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

/// Loopback default: protected routes are open when no API key is set.
const DEFAULT_BIND: &str = "127.0.0.1:8080";
const DEFAULT_DB: &str = "sqlite:./index.db";
const DEFAULT_RPC_URL: &str = "https://api.mainnet-beta.solana.com";
const DEFAULT_IDL_DIR: &str = "./idls";
/// Under --tui the terminal belongs to the TUI, so logs go to this file.
const TUI_LOG_FILE: &str = "index.log";

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
        #[arg(short, long, env = "YELLOWSTONE_ENDPOINT", hide_env_values = true)]
        endpoint: Option<String>,
        /// x-token for the geyser endpoint
        #[arg(long, env = "YELLOWSTONE_AUTH_TOKEN", default_value = "", hide_env_values = true)]
        auth_token: String,
        // No default_value on these: a default would hide which value the user set,
        // so an explicit default could not override YAML. The help text names the default.
        /// SQLite url or path [default: sqlite:./index.db]
        #[arg(long, env = "DATABASE_URL", hide_env_values = true)]
        db: Option<String>,
        /// Directory of program IDL JSON files [default: ./idls]
        #[arg(long)]
        idl_dir: Option<std::path::PathBuf>,
        /// HTTP/WebSocket bind address [default: 127.0.0.1:8080]
        #[arg(long)]
        bind: Option<String>,
        /// Wallet addresses to include (optional filter)
        #[arg(long = "wallet")]
        wallets: Vec<String>,
        /// Terminal live view instead of just HTTP
        #[arg(long)]
        tui: bool,
        /// Optional gRPC export bind address (e.g. 127.0.0.1:50051)
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

    /// Probe a Yellowstone / Richat / Triton gRPC endpoint (GetVersion + replay)
    Probe {
        #[arg(long, value_name = "FILE")]
        config: Option<std::path::PathBuf>,
        #[arg(short, long, env = "YELLOWSTONE_ENDPOINT", hide_env_values = true)]
        endpoint: Option<String>,
        #[arg(long, env = "YELLOWSTONE_AUTH_TOKEN", default_value = "", hide_env_values = true)]
        auth_token: String,
    },

    /// Serve stored events only (no geyser) — demo / read-only mode
    Demo {
        #[arg(long, env = "DATABASE_URL", default_value = DEFAULT_DB, hide_env_values = true)]
        db: String,
        #[arg(long, default_value = DEFAULT_BIND)]
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
        #[arg(long, env = "SOLANA_RPC_URL", default_value = DEFAULT_RPC_URL, hide_env_values = true)]
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
        #[arg(long, env = "SOLANA_RPC_URL", default_value = DEFAULT_RPC_URL, hide_env_values = true)]
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
    replay: pipeline::config_file::ReplayMode,
    ping_secs: u64,
    compression: pipeline::config_file::CompressionKind,
}

struct RunCliOverrides {
    config_path: Option<std::path::PathBuf>,
    endpoint: Option<String>,
    auth_token: String,
    db: Option<String>,
    idl_dir: Option<std::path::PathBuf>,
    bind: Option<String>,
    wallets: Vec<String>,
    grpc_bind: Option<String>,
}

/// Resolve CLI > env > YAML. `env` must return `None` for unset or empty vars.
fn resolve_run_args(
    overrides: RunCliOverrides,
    env: impl Fn(&str) -> Option<String>,
) -> Result<ResolvedRunArgs> {
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
        .map(|c| pipeline::config_file::merge_with(c, &env))
        .transpose()?;

    // clap turns an empty env var into Some(""); that must not hide the YAML endpoint.
    let endpoint = endpoint
        .filter(|s| !s.trim().is_empty())
        .or_else(|| cfg.as_ref().map(|c| c.geyser.endpoint.clone()))
        .filter(|s| !s.trim().is_empty() && !s.contains("${"))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "geyser endpoint required: pass --endpoint, set YELLOWSTONE_ENDPOINT, or use --config"
            )
        })?;

    let auth_token = if auth_token.trim().is_empty() {
        cfg.as_ref()
            .map(|c| c.geyser.auth_token.clone())
            .unwrap_or_default()
    } else {
        auth_token
    };

    // CLI (or its env var), then YAML, then the default. Empty means unset.
    let db = db
        .filter(|s| !s.trim().is_empty())
        .or_else(|| cfg.as_ref().map(|c| c.store.url.clone()))
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_DB.to_string());

    let bind = bind
        .filter(|s| !s.trim().is_empty())
        .or_else(|| cfg.as_ref().map(|c| c.serve.bind.clone()))
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_BIND.to_string());

    let wallets = if wallets.is_empty() {
        cfg.as_ref()
            .map(|c| c.wallets.clone())
            .unwrap_or_default()
    } else {
        wallets
    };

    let idl_dir = idl_dir
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| {
            cfg.as_ref()
                .and_then(|c| c.programs.first())
                .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        })
        .unwrap_or_else(|| std::path::PathBuf::from(DEFAULT_IDL_DIR));

    bind.parse::<std::net::SocketAddr>()
        .map_err(|e| anyhow::anyhow!("serve.bind is not a valid socket address: {bind}: {e}"))?;

    let rpc_url = cfg
        .as_ref()
        .and_then(|c| c.rpc_url.clone())
        .filter(|s| !s.trim().is_empty())
        .or_else(|| env("SOLANA_RPC_URL"));

    let sinks = cfg.as_ref().map(|c| c.sinks.clone()).unwrap_or_default();

    // A present but empty key setting is an error, so a config slip cannot turn auth off.
    let api_keys = match cfg.as_ref().and_then(|c| c.auth.as_ref()) {
        Some(auth) => auth.keys()?,
        None => match env("INDEX_API_KEYS") {
            Some(raw) => pipeline::config_file::parse_api_keys(&raw)?.unwrap_or_default(),
            None => Vec::new(),
        },
    };

    let grpc_bind = grpc_bind
        .or_else(|| {
            cfg.as_ref()
                .and_then(|c| c.grpc.as_ref())
                .map(|g| g.bind.clone())
        })
        .map(|s| {
            s.parse::<std::net::SocketAddr>()
                .map_err(|e| anyhow::anyhow!("grpc bind is not a valid socket address: {s}: {e}"))
        })
        .transpose()?;

    let track_failed = cfg.as_ref().map(|c| c.geyser.track_failed).unwrap_or(false);
    let track_tokens = cfg.as_ref().map(|c| c.geyser.track_tokens).unwrap_or(false);
    let accounts_include = cfg
        .as_ref()
        .map(|c| c.geyser.accounts_include.clone())
        .unwrap_or_default();
    let replay = cfg
        .as_ref()
        .map(|c| c.geyser.replay)
        .unwrap_or_default();
    let ping_secs = cfg.as_ref().map(|c| c.geyser.ping_secs).unwrap_or(10);
    let compression = cfg
        .as_ref()
        .map(|c| c.geyser.compression)
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
        replay,
        ping_secs,
        compression,
    })
}

/// An empty SOLANA_RPC_URL reaches clap as "", not as unset.
fn or_default_rpc(rpc_url: String) -> String {
    if rpc_url.trim().is_empty() {
        DEFAULT_RPC_URL.to_string()
    } else {
        rpc_url
    }
}

/// solana-rpc-client logs reqwest responses at debug, with the full RPC URL and any key in it.
fn log_filter(verbose: bool) -> &'static str {
    if verbose {
        "debug,solana_rpc_client=info"
    } else {
        "info"
    }
}

fn print_completions<G: Generator>(generator: G, cmd: &mut clap::Command) {
    generate(generator, cmd, cmd.get_name().to_string(), &mut io::stdout());
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    match run_cli().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            // The error can quote IDL text from chain. Clean each line, but keep the line breaks.
            let text = format!("{err:?}");
            let lines: Vec<String> = text.lines().map(pipeline::idl::printable).collect();
            eprintln!("Error: {}", lines.join("\n"));
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run_cli() -> Result<()> {
    dotenv().ok();

    let cli = Cli::parse();

    let logs = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(log_filter(cli.verbose)))
        .with_span_events(FmtSpan::CLOSE);
    if matches!(&cli.command, Commands::Run { tui: true, .. }) {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(TUI_LOG_FILE)
            .map_err(|e| anyhow::anyhow!("open log file {TUI_LOG_FILE}: {e}"))?;
        eprintln!("--tui: logs go to {TUI_LOG_FILE}");
        logs.with_ansi(false)
            .with_writer(std::sync::Mutex::new(file))
            .init();
    } else {
        logs.init();
    }

    match cli.command {
        Commands::Completion { shell } => {
            print_completions(shell, &mut Cli::command());
            Ok(())
        }
        Commands::Config { action } => match action {
            ConfigCmd::Validate { file } => {
                let cfg = pipeline::config_file::load(&file)?;
                let cfg = pipeline::config_file::merge_with_env(cfg)?;
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
            let resolved = resolve_run_args(
                RunCliOverrides {
                    config_path: config,
                    endpoint,
                    auth_token,
                    db,
                    idl_dir,
                    bind,
                    wallets,
                    grpc_bind,
                },
                pipeline::config_file::env_var,
            )?;
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
                replay: resolved.replay,
                ping_secs: resolved.ping_secs,
                compression: resolved.compression,
            })
            .await
        }
        Commands::Probe {
            config,
            endpoint,
            auth_token,
        } => {
            let resolved = resolve_run_args(
                RunCliOverrides {
                    config_path: config,
                    endpoint,
                    auth_token,
                    db: None,
                    idl_dir: None,
                    bind: None,
                    wallets: Vec::new(),
                    grpc_bind: None,
                },
                pipeline::config_file::env_var,
            )?;
            let report = pipeline::geyser::probe(
                &resolved.endpoint,
                &resolved.auth_token,
                resolved.compression,
            )
            .await?;
            println!("{}", report.summary());
            Ok(())
        }
        Commands::Demo { db, bind } => {
            let bind: std::net::SocketAddr = bind
                .parse()
                .map_err(|e| anyhow::anyhow!("invalid --bind {bind}: {e}"))?;
            let db = if db.trim().is_empty() { DEFAULT_DB.to_string() } else { db };
            let api_keys = match pipeline::config_file::env_var("INDEX_API_KEYS") {
                Some(raw) => pipeline::config_file::parse_api_keys(&raw)?.unwrap_or_default(),
                None => Vec::new(),
            };
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
            } => {
                let rpc_url = or_default_rpc(rpc_url);
                pipeline::parser_cmd::fetch_idl(&rpc_url, &program_id, &idl_dir, force).await
            }
            ParserCmd::Watch {
                idl_dir,
                rpc_url,
                interval,
                force,
            } => {
                let rpc_url = or_default_rpc(rpc_url);
                pipeline::parser_cmd::watch_idls(&rpc_url, &idl_dir, interval, force).await
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overrides(config_path: Option<std::path::PathBuf>) -> RunCliOverrides {
        RunCliOverrides {
            config_path,
            endpoint: None,
            auth_token: String::new(),
            db: None,
            idl_dir: None,
            bind: None,
            wallets: Vec::new(),
            grpc_bind: None,
        }
    }

    fn write_config(name: &str, body: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("index-{name}-{}.yaml", std::process::id()));
        std::fs::write(&path, body).expect("write config");
        path
    }

    const CONFIG: &str = r#"
geyser:
  endpoint: "https://yaml.example"
  auth_token: "yaml-token"
store:
  url: "sqlite:./yaml.db"
serve:
  bind: "127.0.0.1:9090"
programs: []
wallets: []
sinks: []
"#;

    #[test]
    fn resolve_empty_cli_env_values_use_config() {
        let path = write_config("empty-env", CONFIG);
        let mut cli = overrides(Some(path.clone()));
        // What clap yields when YELLOWSTONE_ENDPOINT="" and DATABASE_URL="" are set.
        cli.endpoint = Some(String::new());
        cli.db = Some(String::new());
        cli.bind = Some(String::new());
        let env = |key: &str| matches!(key, "YELLOWSTONE_ENDPOINT" | "YELLOWSTONE_AUTH_TOKEN").then(String::new);
        let resolved = resolve_run_args(cli, env).expect("resolve");
        let _ = std::fs::remove_file(&path);
        assert_eq!(resolved.endpoint, "https://yaml.example");
        assert_eq!(resolved.auth_token, "yaml-token");
        assert_eq!(resolved.db, "sqlite:./yaml.db");
        assert_eq!(resolved.bind, "127.0.0.1:9090");
    }

    #[test]
    fn resolve_rejects_invalid_grpc_bind() {
        let mut cli = overrides(None);
        cli.endpoint = Some("https://geyser.example".into());
        cli.grpc_bind = Some("localhost:50051".into());
        assert!(resolve_run_args(cli, |_| None).is_err());
    }

    #[test]
    fn resolve_rejects_api_keys_without_a_key() {
        let mut cli = overrides(None);
        cli.endpoint = Some("https://geyser.example".into());
        let env = |key: &str| (key == "INDEX_API_KEYS").then(|| " , ".to_string());
        assert!(resolve_run_args(cli, env).is_err());

        let path = write_config(
            "empty-keys",
            &format!("{CONFIG}auth:\n  api_keys: [\"\"]\n"),
        );
        let result = resolve_run_args(overrides(Some(path.clone())), |_| None);
        let _ = std::fs::remove_file(&path);
        assert!(result.is_err());
    }

    #[test]
    fn explicit_cli_default_wins_over_yaml() {
        let yaml = CONFIG
            .replace("127.0.0.1:9090", "0.0.0.0:9090")
            .replace("programs: []", "programs: [\"./yaml-idls/p.json\"]");
        let path = write_config("cli-default", &yaml);
        let mut cli = overrides(Some(path.clone()));
        cli.bind = Some(DEFAULT_BIND.into());
        cli.db = Some(DEFAULT_DB.into());
        cli.idl_dir = Some(DEFAULT_IDL_DIR.into());
        let resolved = resolve_run_args(cli, |_| None).expect("resolve");
        let defaults = resolve_run_args(overrides(Some(path.clone())), |_| None).expect("resolve");
        let _ = std::fs::remove_file(&path);
        assert_eq!(resolved.bind, DEFAULT_BIND);
        assert_eq!(resolved.db, DEFAULT_DB);
        assert_eq!(resolved.idl_dir, std::path::PathBuf::from(DEFAULT_IDL_DIR));
        assert_eq!(defaults.bind, "0.0.0.0:9090");
        assert_eq!(defaults.idl_dir, std::path::PathBuf::from("./yaml-idls"));
    }

    #[test]
    fn help_names_run_defaults() {
        let mut cmd = Cli::command();
        let run = cmd.find_subcommand_mut("run").expect("run");
        for (id, default) in [("bind", DEFAULT_BIND), ("db", DEFAULT_DB), ("idl_dir", DEFAULT_IDL_DIR)] {
            let arg = run.get_arguments().find(|a| a.get_id() == id).expect("arg");
            let help = arg.get_help().map(|h| h.to_string()).unwrap_or_default();
            assert!(help.contains(default), "{id} help lacks {default}");
        }
    }

    #[test]
    fn verbose_logs_keep_rpc_client_at_info() {
        use tracing_subscriber::layer::SubscriberExt;
        let subscriber = tracing_subscriber::registry().with(EnvFilter::new(log_filter(true)));
        tracing::subscriber::with_default(subscriber, || {
            assert!(tracing::enabled!(target: "index::pipeline::run", tracing::Level::DEBUG));
            assert!(!tracing::enabled!(
                target: "solana_rpc_client::http_sender",
                tracing::Level::DEBUG
            ));
        });
    }

    #[test]
    fn default_bind_is_loopback() {
        let bind: std::net::SocketAddr = DEFAULT_BIND.parse().expect("parse");
        assert!(bind.ip().is_loopback());
    }

    #[test]
    fn help_hides_secret_env_values() {
        let mut cmd = Cli::command();
        for sub in ["run", "probe", "demo"] {
            let sub = cmd.find_subcommand_mut(sub).expect("subcommand");
            for arg in sub.get_arguments() {
                if arg.get_env().is_some() {
                    assert!(arg.is_hide_env_values_set(), "{} shows its env value", arg.get_id());
                }
            }
        }
        let parser = cmd.find_subcommand_mut("parser").expect("parser");
        for sub in ["fetch", "watch"] {
            let sub = parser.find_subcommand_mut(sub).expect("subcommand");
            for arg in sub.get_arguments() {
                if arg.get_env().is_some() {
                    assert!(arg.is_hide_env_values_set(), "{} shows its env value", arg.get_id());
                }
            }
        }
    }
}
