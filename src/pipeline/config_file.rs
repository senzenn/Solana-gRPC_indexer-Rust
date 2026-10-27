use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

// Unknown keys are errors at every level: a misspelled or misplaced `auth:` would
// otherwise turn API auth off.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IndexConfig {
    pub geyser: GeyserConfig,
    #[serde(default)]
    pub store: StoreConfig,
    #[serde(default)]
    pub serve: ServeConfig,
    /// IDL JSON file paths (one program per file).
    #[serde(default)]
    pub programs: Vec<PathBuf>,
    #[serde(default)]
    pub wallets: Vec<String>,
    #[serde(default)]
    pub sinks: Vec<SinkConfig>,
    /// Solana JSON-RPC URL for backfill / gap repair.
    pub rpc_url: Option<String>,
    pub auth: Option<AuthConfig>,
    pub grpc: Option<GrpcConfig>,
    pub clickhouse: Option<ClickHouseConfig>,
}

/// Copy finalized events to ClickHouse. The password comes only from
/// CLICKHOUSE_PASSWORD, so it stays out of the config file.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ClickHouseConfig {
    /// HTTP interface, for example http://127.0.0.1:8123.
    pub url: String,
    #[serde(default = "default_ch_database")]
    pub database: String,
    #[serde(default = "default_ch_table")]
    pub table: String,
    #[serde(default)]
    pub user: Option<String>,
}

fn default_ch_database() -> String {
    "default".to_string()
}

fn default_ch_table() -> String {
    "index_events".to_string()
}

impl ClickHouseConfig {
    /// The names go into SQL text, so allow only plain identifiers.
    pub fn validate(&self) -> Result<()> {
        let url = url::Url::parse(&self.url).context("clickhouse.url is not a valid URL")?;
        if !matches!(url.scheme(), "http" | "https") {
            bail!("clickhouse.url must use http or https");
        }
        for (field, name) in [("database", &self.database), ("table", &self.table)] {
            let mut chars = name.chars();
            let ok = chars
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
            if !ok {
                bail!("clickhouse.{field} must be letters, digits or _: {name:?}");
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GrpcConfig {
    pub bind: String,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ReplayMode {
    /// Probe `SubscribeReplayInfo`; omit `from_slot` if the server has no buffer.
    #[default]
    Auto,
    On,
    Off,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum CompressionKind {
    /// Original Yellowstone / many gateways (QuickNode default).
    Gzip,
    /// Triton Dragon's Mouth and Richat on port 443.
    #[default]
    Zstd,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GeyserConfig {
    /// Empty here is allowed: YELLOWSTONE_ENDPOINT can set it.
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub auth_token: String,
    #[serde(default)]
    pub track_failed: bool,
    #[serde(default)]
    pub track_tokens: bool,
    #[serde(default)]
    pub accounts_include: Vec<String>,
    /// `auto` (default) probes replay; `off` never sends `from_slot`; `on` always tries.
    #[serde(default)]
    pub replay: ReplayMode,
    /// Stream keepalive ping interval in seconds. `0` disables client pings.
    #[serde(default = "default_ping_secs")]
    pub ping_secs: u64,
    /// Payload compression to accept from the server.
    #[serde(default)]
    pub compression: CompressionKind,
}

pub const DEFAULT_PING_SECS: u64 = 10;

fn default_ping_secs() -> u64 {
    DEFAULT_PING_SECS
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StoreConfig {
    #[serde(default = "default_store_url")]
    pub url: String,
}

pub const DEFAULT_STORE_URL: &str = "sqlite:./index.db";

fn default_store_url() -> String {
    DEFAULT_STORE_URL.to_string()
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            url: default_store_url(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ServeConfig {
    #[serde(default = "default_bind")]
    pub bind: String,
}

/// Loopback default: protected routes are open when no API key is set.
pub const DEFAULT_BIND: &str = "127.0.0.1:8080";

fn default_bind() -> String {
    DEFAULT_BIND.to_string()
}

impl Default for ServeConfig {
    fn default() -> Self {
        Self {
            bind: default_bind(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SinkConfig {
    pub kind: String,
    pub url: Option<String>,
    pub types: Option<Vec<String>>,
    pub programs: Option<Vec<String>>,
    pub instructions: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AuthConfig {
    pub api_keys: Vec<String>,
}

impl AuthConfig {
    /// Non-empty keys. An auth block with no usable key is an error, so it cannot turn auth off.
    pub fn keys(&self) -> Result<Vec<String>> {
        let keys: Vec<String> = self
            .api_keys
            .iter()
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())
            .collect();
        if keys.is_empty() {
            bail!("auth.api_keys is set but has no non-empty key");
        }
        Ok(keys)
    }
}

/// Parse a comma-separated `INDEX_API_KEYS` value. Empty means unset.
pub fn parse_api_keys(raw: &str) -> Result<Option<Vec<String>>> {
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let keys: Vec<String> = raw
        .split(',')
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
        .collect();
    if keys.is_empty() {
        bail!("INDEX_API_KEYS is set but has no non-empty key");
    }
    Ok(Some(keys))
}

/// Keys from `INDEX_API_KEYS`. Unset gives no keys.
pub fn env_api_keys(env: impl Fn(&str) -> Option<String>) -> Result<Vec<String>> {
    match env("INDEX_API_KEYS") {
        Some(raw) => Ok(parse_api_keys(&raw)?.unwrap_or_default()),
        None => Ok(Vec::new()),
    }
}

/// Read an env var. An empty or blank value counts as unset.
pub fn env_var(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

pub fn load(path: &Path) -> Result<IndexConfig> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read config file {}", path.display()))?;
    serde_yaml::from_str(&raw).with_context(|| format!("parse YAML config {}", path.display()))
}

pub fn merge_with_env(cfg: IndexConfig) -> Result<IndexConfig> {
    merge_with(cfg, env_var)
}

/// Apply env overrides from `env`. Empty values must not hide values from YAML.
pub fn merge_with(
    mut cfg: IndexConfig,
    env: impl Fn(&str) -> Option<String>,
) -> Result<IndexConfig> {
    let get = |key: &str| env(key).filter(|v| !v.trim().is_empty());
    if let Some(endpoint) = get("YELLOWSTONE_ENDPOINT") {
        cfg.geyser.endpoint = endpoint;
    }
    if let Some(token) = get("YELLOWSTONE_AUTH_TOKEN") {
        cfg.geyser.auth_token = token;
    }
    if let Some(url) = get("DATABASE_URL") {
        cfg.store.url = url;
    }
    if let Some(rpc) = get("SOLANA_RPC_URL") {
        cfg.rpc_url = Some(rpc);
    }
    if let Some(raw) = get("INDEX_API_KEYS")
        && let Some(api_keys) = parse_api_keys(&raw)?
    {
        cfg.auth = Some(AuthConfig { api_keys });
    }
    Ok(cfg)
}

impl IndexConfig {
    pub fn validate(&self) -> Result<()> {
        if self.geyser.endpoint.trim().is_empty() {
            bail!("geyser.endpoint must be non-empty (set in YAML or YELLOWSTONE_ENDPOINT)");
        }
        self.serve.bind.parse::<SocketAddr>().with_context(|| {
            format!(
                "serve.bind is not a valid socket address: {}",
                self.serve.bind
            )
        })?;
        if let Some(grpc) = &self.grpc {
            grpc.bind.parse::<SocketAddr>().with_context(|| {
                format!("grpc.bind is not a valid socket address: {}", grpc.bind)
            })?;
        }
        if let Some(auth) = &self.auth {
            auth.keys()?;
        }
        super::sink::validate_sinks(&self.sinks).context("invalid sinks config")?;
        if let Some(clickhouse) = &self.clickhouse {
            clickhouse.validate()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> IndexConfig {
        IndexConfig {
            geyser: GeyserConfig {
                endpoint: String::new(),
                auth_token: String::new(),
                track_failed: false,
                track_tokens: false,
                accounts_include: Vec::new(),
                replay: ReplayMode::Auto,
                ping_secs: default_ping_secs(),
                compression: CompressionKind::Zstd,
            },
            store: StoreConfig::default(),
            serve: ServeConfig::default(),
            programs: vec![PathBuf::from("./idls/pumpFun.json")],
            wallets: Vec::new(),
            sinks: Vec::new(),
            rpc_url: None,
            auth: None,
            grpc: None,
            clickhouse: None,
        }
    }

    #[test]
    fn parses_index_example_yaml() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("index.example.yaml");
        let cfg = load(&path).expect("example yaml should parse");
        assert_eq!(cfg.store.url, "sqlite:./index.db");
        assert_eq!(cfg.serve.bind, "127.0.0.1:8080");
        assert!(
            cfg.programs.iter().any(|p| p.ends_with("pumpFun.json")),
            "expected pump.fun IDL path"
        );
        assert!(cfg.geyser.endpoint.is_empty());
        assert!(!cfg.geyser.track_failed);
        assert!(!cfg.geyser.track_tokens);
        assert!(cfg.geyser.accounts_include.is_empty());
        assert_eq!(cfg.geyser.replay, ReplayMode::Auto);
        assert_eq!(cfg.geyser.ping_secs, 10);
        assert_eq!(cfg.geyser.compression, CompressionKind::Zstd);
        assert!(default_config().validate().is_err());
    }

    #[test]
    fn optional_fields_take_documented_defaults() {
        let cfg: IndexConfig =
            serde_yaml::from_str("geyser:\n  endpoint: \"http://127.0.0.1:10000\"\n")
                .expect("minimal yaml should parse");
        let defaults = default_config();
        assert_eq!(cfg.store, defaults.store);
        assert_eq!(cfg.serve, defaults.serve);
        assert_eq!(cfg.geyser.auth_token, "");
        assert!(cfg.programs.is_empty() && cfg.wallets.is_empty() && cfg.sinks.is_empty());
        cfg.validate().expect("minimal config is valid");
    }

    #[test]
    fn default_config_matches_run_defaults() {
        assert_eq!(StoreConfig::default().url, "sqlite:./index.db");
        assert_eq!(ServeConfig::default().bind, "127.0.0.1:8080");
    }

    #[test]
    fn validate_accepts_nonempty_endpoint_and_valid_bind() {
        let mut cfg = default_config();
        cfg.geyser.endpoint = "https://geyser.example:10000".into();
        cfg.validate().expect("valid config should pass");
    }

    fn yaml_config() -> IndexConfig {
        let mut cfg = default_config();
        cfg.geyser.endpoint = "https://yaml.example".into();
        cfg.geyser.auth_token = "yaml-token".into();
        cfg.store.url = "sqlite:./yaml.db".into();
        cfg.auth = Some(AuthConfig {
            api_keys: vec!["yaml-key".into()],
        });
        cfg
    }

    #[test]
    fn empty_env_values_do_not_override_yaml() {
        let env = |key: &str| match key {
            "YELLOWSTONE_ENDPOINT" | "YELLOWSTONE_AUTH_TOKEN" | "DATABASE_URL" => {
                Some(String::new())
            }
            "INDEX_API_KEYS" => Some("  ".to_string()),
            _ => None,
        };
        let cfg = merge_with(yaml_config(), env).expect("merge");
        assert_eq!(cfg, yaml_config());
    }

    #[test]
    fn nonempty_env_values_override_yaml() {
        let env = |key: &str| match key {
            "YELLOWSTONE_AUTH_TOKEN" => Some("env-token".to_string()),
            "INDEX_API_KEYS" => Some("a, b".to_string()),
            _ => None,
        };
        let cfg = merge_with(yaml_config(), env).expect("merge");
        assert_eq!(cfg.geyser.auth_token, "env-token");
        assert_eq!(cfg.auth.expect("auth").api_keys, vec!["a", "b"]);
    }

    #[test]
    fn api_keys_without_a_key_are_an_error() {
        assert_eq!(parse_api_keys("").expect("empty is unset"), None);
        assert!(parse_api_keys(",").is_err());
        assert!(parse_api_keys(" , ").is_err());
        let env = |key: &str| (key == "INDEX_API_KEYS").then(|| ",".to_string());
        assert!(merge_with(yaml_config(), env).is_err());

        let mut cfg = yaml_config();
        cfg.auth = Some(AuthConfig {
            api_keys: vec![" ".into()],
        });
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn misspelled_top_level_key_is_rejected() {
        let raw = std::fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("index.example.yaml"),
        )
        .expect("read example");
        let typo = format!("{raw}\nauht:\n  api_keys: [\"secret\"]\n");
        assert!(serde_yaml::from_str::<IndexConfig>(&typo).is_err());
    }

    #[test]
    fn misplaced_nested_key_is_rejected() {
        let raw = std::fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("index.example.yaml"),
        )
        .expect("read example");
        assert!(serde_yaml::from_str::<IndexConfig>(&raw).is_ok());
        let nested = format!(
            "{raw}\ngrpc:\n  bind: \"127.0.0.1:50051\"\n  auth:\n    api_keys: [\"secret\"]\n"
        );
        assert!(serde_yaml::from_str::<IndexConfig>(&nested).is_err());
        let sink_typo = raw.replace("  - kind: stdout", "  - kind: stdout\n    type: [\"gap\"]");
        assert_ne!(sink_typo, raw);
        assert!(serde_yaml::from_str::<IndexConfig>(&sink_typo).is_err());
    }

    #[test]
    fn validate_rejects_bad_sink_grpc_bind_and_clickhouse_name() {
        let mut cfg = yaml_config();
        cfg.grpc = Some(GrpcConfig {
            bind: "localhost:50051".into(),
        });
        assert!(cfg.validate().is_err());

        let mut cfg = yaml_config();
        cfg.sinks = vec![SinkConfig {
            kind: "webhok".into(),
            url: None,
            types: None,
            programs: None,
            instructions: None,
        }];
        assert!(cfg.validate().is_err());

        // The table name goes into SQL text.
        let mut cfg = yaml_config();
        cfg.clickhouse = Some(ClickHouseConfig {
            url: "http://127.0.0.1:8123".into(),
            database: "default".into(),
            table: "events; DROP TABLE x".into(),
            user: None,
        });
        assert!(cfg.validate().is_err());
    }
}
