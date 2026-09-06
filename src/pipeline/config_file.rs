use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
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
pub struct GrpcConfig {
    pub bind: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct GeyserConfig {
    pub endpoint: String,
    pub auth_token: String,
    #[serde(default)]
    pub track_failed: bool,
    #[serde(default)]
    pub track_tokens: bool,
    #[serde(default)]
    pub accounts_include: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct StoreConfig {
    pub url: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ServeConfig {
    pub bind: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct SinkConfig {
    pub kind: String,
    pub url: Option<String>,
    pub types: Option<Vec<String>>,
    pub programs: Option<Vec<String>>,
    pub instructions: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct AuthConfig {
    pub api_keys: Vec<String>,
}

pub fn load(path: &Path) -> Result<IndexConfig> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read config file {}", path.display()))?;
    serde_yaml::from_str(&raw).with_context(|| format!("parse YAML config {}", path.display()))
}

pub fn merge_with_env(mut cfg: IndexConfig) -> IndexConfig {
    if let Ok(endpoint) = std::env::var("YELLOWSTONE_ENDPOINT")
        && !endpoint.is_empty() {
            cfg.geyser.endpoint = endpoint;
        }
    if let Ok(token) = std::env::var("YELLOWSTONE_AUTH_TOKEN") {
        cfg.geyser.auth_token = token;
    }
    if let Ok(url) = std::env::var("DATABASE_URL")
        && !url.is_empty() {
            cfg.store.url = url;
        }
    if let Ok(rpc) = std::env::var("SOLANA_RPC_URL")
        && !rpc.is_empty() {
            cfg.rpc_url = Some(rpc);
        }
    if let Ok(keys) = std::env::var("INDEX_API_KEYS")
        && !keys.is_empty() {
            let api_keys: Vec<String> = keys.split(',').map(|s| s.trim().to_string()).collect();
            cfg.auth = Some(AuthConfig { api_keys });
        }
    cfg
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
        },
        store: StoreConfig {
            url: "sqlite:./index.db".to_string(),
        },
        serve: ServeConfig {
            bind: "0.0.0.0:8080".to_string(),
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
        assert_eq!(cfg.serve.bind, "0.0.0.0:8080");
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
        assert!(default_config().validate().is_err());
    }

    #[test]
    fn default_config_matches_run_defaults() {
        let cfg = default_config();
        assert_eq!(cfg.store.url, "sqlite:./index.db");
        assert_eq!(cfg.serve.bind, "0.0.0.0:8080");
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
}
