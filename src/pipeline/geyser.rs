//! Yellowstone gRPC connect helpers for Triton Dragon's Mouth and Richat.
//!
//! Both speak `geyser.Geyser/Subscribe`. Differences are TLS, compression,
//! replay (`from_slot`), and idle pings; not a second protocol.

use std::time::Duration;

use anyhow::{Context, Result};
use tonic::codec::CompressionEncoding;
use tracing::{info, warn};
use yellowstone_grpc_client::{ClientTlsConfig, GeyserGrpcClient, Interceptor};

use super::config_file::{CompressionKind, ReplayMode};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const HTTP2_KEEPALIVE: Duration = Duration::from_secs(20);
const HTTP2_KEEPALIVE_TIMEOUT: Duration = Duration::from_secs(10);
const TCP_KEEPALIVE: Duration = Duration::from_secs(20);
const MAX_DECODE_BYTES: usize = 64 * 1024 * 1024;

/// Add a scheme for tonic. Only loopback gets http://: a guess for a remote host
/// would send the x-token in clear text.
pub fn normalize_endpoint(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return trimmed.to_string();
    }
    let hostport = trimmed.trim_start_matches("grpc://");
    if is_loopback(&format!("http://{hostport}")) {
        format!("http://{hostport}")
    } else {
        format!("https://{hostport}")
    }
}

/// Parse the URL, so userinfo such as `localhost:x@evil.com` is not read as the host.
fn is_loopback(endpoint: &str) -> bool {
    match url::Url::parse(endpoint)
        .ok()
        .as_ref()
        .and_then(url::Url::host)
    {
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

pub fn endpoint_label(endpoint: &str) -> String {
    match url::Url::parse(endpoint) {
        Ok(url) => url_label(&url),
        Err(_) => "<invalid endpoint>".to_string(),
    }
}

/// Scheme, host and port only. Providers often put the API key in the URL
/// path or query, so log lines and reports must not show the full URL.
pub fn url_label(url: &url::Url) -> String {
    let host = url.host_str().unwrap_or_default();
    match url.port() {
        Some(port) => format!("{}://{host}:{port}", url.scheme()),
        None => format!("{}://{host}", url.scheme()),
    }
}

pub fn uses_tls(endpoint: &str) -> bool {
    endpoint.starts_with("https://")
}

pub fn is_replay_error(err: &str) -> bool {
    let err = err.to_ascii_lowercase();
    err.contains("from_slot")
        || err.contains("fromslot")
        || err.contains("from slot")
        // Yellowstone: "broadcast from N is not available, last available: M".
        || (err.contains("broadcast from") && err.contains("not available"))
        || (err.contains("replay")
            && (err.contains("not")
                || err.contains("unavailable")
                || err.contains("invalid")
                || err.contains("fail")))
}

pub fn compression_encoding(kind: CompressionKind) -> CompressionEncoding {
    match kind {
        CompressionKind::Gzip => CompressionEncoding::Gzip,
        CompressionKind::Zstd => CompressionEncoding::Zstd,
    }
}

pub struct ProbeReport {
    pub endpoint: String,
    pub version: Option<String>,
    pub replay_first_available: Option<u64>,
}

impl ProbeReport {
    pub fn summary(&self) -> String {
        let version = self.version.as_deref().unwrap_or("unknown");
        let replay = match self.replay_first_available {
            Some(slot) => format!("yes (first_available={slot})"),
            None => "no: from_slot will be omitted".into(),
        };
        format!(
            "geyser probe\n  endpoint: {}\n  version:  {version}\n  replay:   {replay}",
            self.endpoint
        )
    }
}

pub async fn connect(
    endpoint: &str,
    auth_token: &str,
    compression: CompressionKind,
) -> Result<GeyserGrpcClient<impl Interceptor + use<>>> {
    let endpoint = normalize_endpoint(endpoint);
    let token = if auth_token.trim().is_empty() {
        None
    } else {
        Some(auth_token.trim().to_string())
    };

    info!(
        endpoint = %endpoint_label(&endpoint),
        tls = uses_tls(&endpoint),
        compression = ?compression,
        has_token = token.is_some(),
        "connecting geyser"
    );
    if token.is_some() && endpoint.starts_with("http://") && !is_loopback(&endpoint) {
        warn!(
            "geyser x-token is sent without TLS to a remote host; use https:// if the server supports it"
        );
    }

    let mut builder = GeyserGrpcClient::build_from_shared(endpoint.clone())?
        .x_token(token)?
        .connect_timeout(CONNECT_TIMEOUT)
        .http2_keep_alive_interval(HTTP2_KEEPALIVE)
        .keep_alive_timeout(HTTP2_KEEPALIVE_TIMEOUT)
        .keep_alive_while_idle(true)
        .tcp_keepalive(Some(TCP_KEEPALIVE))
        .tcp_nodelay(true)
        .http2_adaptive_window(true)
        .max_decoding_message_size(MAX_DECODE_BYTES)
        .accept_compressed(compression_encoding(compression));

    if uses_tls(&endpoint) {
        builder = builder.tls_config(ClientTlsConfig::new().with_native_roots())?;
    }

    builder.connect().await.context("geyser connect")
}

/// `GetVersion` + `SubscribeReplayInfo`. Returns the server version and the first
/// replay slot. Safe on Triton and Richat: missing replay RPCs mean no replay.
pub async fn server_info<F: Interceptor>(
    client: &mut GeyserGrpcClient<F>,
) -> (Option<String>, Option<u64>) {
    let version = match client.get_version().await {
        Ok(resp) => {
            info!(version = %resp.version, "geyser GetVersion");
            Some(resp.version)
        }
        Err(err) => {
            warn!(error = %err, "GetVersion not available");
            None
        }
    };
    let first_available = match client.subscribe_replay_info().await {
        // Yellowstone answers Ok with no slot when its replay buffer is off.
        Ok(info) => {
            info!(first_available = ?info.first_available, "geyser replay info");
            info.first_available
        }
        Err(err) => {
            info!(error = %err, "geyser replay not advertised");
            None
        }
    };
    (version, first_available)
}

pub async fn probe(
    endpoint: &str,
    auth_token: &str,
    compression: CompressionKind,
) -> Result<ProbeReport> {
    let endpoint = normalize_endpoint(endpoint);
    let mut client = connect(&endpoint, auth_token, compression).await?;
    let (version, replay_first_available) = server_info(&mut client).await;
    Ok(ProbeReport {
        endpoint: endpoint_label(&endpoint),
        version,
        replay_first_available,
    })
}

pub fn resolve_from_slot(
    requested: Option<u64>,
    replay: ReplayMode,
    first_available: Option<u64>,
) -> Option<u64> {
    let want = match replay {
        ReplayMode::Off => return None,
        ReplayMode::On => requested,
        ReplayMode::Auto => {
            if first_available.is_some() {
                requested
            } else {
                None
            }
        }
    };
    let slot = want?;
    if let Some(first) = first_available
        && slot < first
    {
        warn!(
            requested = slot,
            first_available = first,
            "from_slot older than replay buffer; starting from tip"
        );
        return None;
    }
    Some(slot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_https_triton_style() {
        assert_eq!(
            normalize_endpoint("https://grpc.triton.one"),
            "https://grpc.triton.one"
        );
        assert_eq!(
            normalize_endpoint("grpc.triton.one"),
            "https://grpc.triton.one"
        );
        assert_eq!(
            normalize_endpoint("grpc.triton.one:443"),
            "https://grpc.triton.one:443"
        );
    }

    #[test]
    fn normalize_http_richat_local() {
        assert_eq!(
            normalize_endpoint("http://127.0.0.1:10000"),
            "http://127.0.0.1:10000"
        );
        assert_eq!(
            normalize_endpoint("127.0.0.1:10000"),
            "http://127.0.0.1:10000"
        );
        assert_eq!(
            normalize_endpoint("grpc://127.0.0.1:10000"),
            "http://127.0.0.1:10000"
        );
        assert_eq!(
            normalize_endpoint("localhost:10000"),
            "http://localhost:10000"
        );
        assert_eq!(normalize_endpoint("[::1]:10000"), "http://[::1]:10000");
    }

    #[test]
    fn normalize_remote_custom_port_keeps_tls() {
        assert_eq!(
            normalize_endpoint("myname.provider.example:10000"),
            "https://myname.provider.example:10000"
        );
        assert_eq!(
            normalize_endpoint("grpc://10.0.0.5:10000"),
            "https://10.0.0.5:10000"
        );
        assert_eq!(
            normalize_endpoint("localhost:x@evil.com:10000"),
            "https://localhost:x@evil.com:10000"
        );
        assert!(!is_loopback("http://127.0.0.1:1@evil.com:10000"));
    }

    #[test]
    fn endpoint_label_drops_path_and_query() {
        assert_eq!(
            endpoint_label("https://grpc.example.com/abc-SECRET?api-key=SECRET"),
            "https://grpc.example.com"
        );
        assert_eq!(
            endpoint_label("http://user:SECRET@10.0.0.5:10000/SECRET"),
            "http://10.0.0.5:10000"
        );
        assert_eq!(endpoint_label("http://[::1]:10000"), "http://[::1]:10000");
        assert_eq!(endpoint_label("not a url SECRET"), "<invalid endpoint>");
    }

    #[test]
    fn tls_only_for_https() {
        assert!(uses_tls("https://grpc.triton.one"));
        assert!(!uses_tls("http://127.0.0.1:10000"));
    }

    #[test]
    fn replay_error_detects_provider_messages() {
        assert!(is_replay_error("from_slot is not available"));
        assert!(is_replay_error("Failed to replay from slot"));
        assert!(is_replay_error("replay unavailable"));
        assert!(is_replay_error(
            "broadcast from 100 is not available, last available: 250"
        ));
        assert!(!is_replay_error("connection reset"));
    }

    #[test]
    fn resolve_from_slot_auto_requires_replay() {
        assert_eq!(resolve_from_slot(Some(10), ReplayMode::Auto, None), None);
        assert_eq!(
            resolve_from_slot(Some(10), ReplayMode::Auto, Some(5)),
            Some(10)
        );
        assert_eq!(resolve_from_slot(Some(3), ReplayMode::Auto, Some(5)), None);
        assert_eq!(resolve_from_slot(Some(10), ReplayMode::Off, Some(1)), None);
        assert_eq!(resolve_from_slot(Some(10), ReplayMode::On, None), Some(10));
    }
}
