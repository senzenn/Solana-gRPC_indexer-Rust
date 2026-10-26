use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

// Unknown keys are errors at every level: a misspelled or misplaced `auth:` would
// otherwise turn API auth off.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IndexConfig {
    pub geyser: GeyserConfig,
    pub store: StoreConfig,
    pub serve: ServeConfig,
    /// IDL JSON file paths (one program per file).
    pub programs: Vec<PathBuf>,
    pub wallets: Vec<String>,
    pub sinks: Vec<SinkConfig>,
    /// Solana JSON-RPC URL for backfill / gap repair.
    pub rpc_url: Option<String>,
    pub auth: Option<AuthConfig>,
    pub grpc: Option<GrpcConfig>,
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
    pub endpoint: String,
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

fn default_ping_secs() -> u64 {
    10
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StoreConfig {
    pub url: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ServeConfig {
    pub bind: String,
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
pub fn merge_with(mut cfg: IndexConfig, env: impl Fn(&str) -> Option<String>) -> Result<IndexConfig> {
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

#[allow(dead_code)]
pub fn default_config() -> IndexConfig {
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
        store: StoreConfig {
            url: "sqlite:./index.db".to_string(),
        },
        serve: ServeConfig {
            bind: "127.0.0.1:8080".to_string(),
        },
        programs: vec![PathBuf::from("./idls/pump_fun.json")],
        wallets: Vec::new(),
        sinks: Vec::new(),
        rpc_url: None,
        auth: None,
        grpc: None,
    }
}

impl IndexConfig {
    pub fn validate(&self) -> Result<()> {
        if self.geyser.endpoint.trim().is_empty() {
            bail!("geyser.endpoint must be non-empty (set in YAML or YELLOWSTONE_ENDPOINT)");
        }
        self.serve
            .bind
            .parse::<SocketAddr>()
            .with_context(|| format!("serve.bind is not a valid socket address: {}", self.serve.bind))?;
        if let Some(grpc) = &self.grpc {
            grpc.bind
                .parse::<SocketAddr>()
                .with_context(|| format!("grpc.bind is not a valid socket address: {}", grpc.bind))?;
        }
        if let Some(auth) = &self.auth {
            auth.keys()?;
        }
        super::sink::build_sinks(&self.sinks).context("invalid sinks config")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_index_example_yaml() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("index.example.yaml");
        let cfg = load(&path).expect("example yaml should parse");
        assert_eq!(cfg.store.url, "sqlite:./index.db");
        assert_eq!(cfg.serve.bind, "127.0.0.1:8080");
        assert!(
            cfg.programs
                .iter()
                .any(|p| p.ends_with("pump_fun.json")),
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
    fn default_config_matches_run_defaults() {
        let cfg = default_config();
        assert_eq!(cfg.store.url, "sqlite:./index.db");
        assert_eq!(cfg.serve.bind, "127.0.0.1:8080");
        assert_eq!(cfg.geyser.auth_token, "");
        assert!(!cfg.geyser.track_failed);
        assert!(!cfg.geyser.track_tokens);
        assert!(cfg.geyser.accounts_include.is_empty());
        assert!(cfg.wallets.is_empty());
        assert!(cfg.sinks.is_empty());
        assert!(cfg.rpc_url.is_none());
        assert!(cfg.auth.is_none());
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
            "YELLOWSTONE_ENDPOINT" | "YELLOWSTONE_AUTH_TOKEN" | "DATABASE_URL" => Some(String::new()),
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
    fn validate_rejects_bad_sink_and_grpc_bind() {
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
    }
}
