use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use reqwest::{Client, Url, redirect};
use tracing::warn;

use super::config_file::SinkConfig;
use super::event::IndexEvent;

#[derive(Debug, Clone, Default)]
pub struct SinkFilter {
    pub kinds: Option<Vec<String>>,
    pub programs: Option<Vec<String>>,
    pub instructions: Option<Vec<String>>,
}

impl From<&SinkConfig> for SinkFilter {
    fn from(cfg: &SinkConfig) -> Self {
        Self {
            kinds: cfg.types.clone(),
            programs: cfg.programs.clone(),
            instructions: cfg.instructions.clone(),
        }
    }
}

/// Returns true when `event` passes optional kind/program/instruction filters.
pub fn matches_filter(event: &IndexEvent, filter: &SinkFilter) -> bool {
    if let Some(kinds) = &filter.kinds
        && !kinds.is_empty()
        && !kinds.iter().any(|k| k == event.kind())
    {
        return false;
    }
    if let Some(programs) = &filter.programs
        && !programs.is_empty()
        && !programs.iter().any(|p| p == event.program())
    {
        return false;
    }
    if let Some(instructions) = &filter.instructions
        && !instructions.is_empty()
    {
        let ix_name = match event {
            IndexEvent::ProgramIx { instruction, .. } => Some(instruction.as_str()),
            _ => None,
        };
        match ix_name {
            Some(name) if instructions.iter().any(|i| i == name) => {}
            _ => return false,
        }
    }
    true
}

#[async_trait]
pub trait EventSink: Send + Sync {
    async fn send(&self, event: &IndexEvent) -> Result<()>;
}

pub struct SinkFanout {
    sinks: Vec<Box<dyn EventSink + Send + Sync>>,
}

impl SinkFanout {
    pub fn new(sinks: Vec<Box<dyn EventSink + Send + Sync>>) -> Self {
        Self { sinks }
    }

    pub async fn send(&self, event: &IndexEvent) -> Result<()> {
        for sink in &self.sinks {
            if let Err(err) = sink.send(event).await {
                warn!(error = %format_args!("{err:#}"), "event sink failed");
            }
        }
        Ok(())
    }
}

struct StdoutSink {
    filter: SinkFilter,
}

impl StdoutSink {
    fn new(cfg: &SinkConfig) -> Self {
        Self {
            filter: SinkFilter::from(cfg),
        }
    }
}

#[async_trait]
impl EventSink for StdoutSink {
    async fn send(&self, event: &IndexEvent) -> Result<()> {
        if !matches_filter(event, &self.filter) {
            return Ok(());
        }
        let line = serde_json::to_string(event)?;
        println!("{line}");
        Ok(())
    }
}

const WEBHOOK_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const WEBHOOK_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const WEBHOOK_ATTEMPTS: u32 = 3;

/// A slow endpoint must not hold delivery, and a redirect must not resend the body to another host.
fn webhook_client() -> Result<Client> {
    Client::builder()
        .connect_timeout(WEBHOOK_CONNECT_TIMEOUT)
        .timeout(WEBHOOK_REQUEST_TIMEOUT)
        .redirect(redirect::Policy::none())
        .build()
        .context("build webhook http client")
}

struct WebhookSink {
    url: Url,
    /// Scheme and host only, because the path or query can hold a secret token.
    label: String,
    filter: SinkFilter,
    client: Client,
}

fn webhook_url(cfg: &SinkConfig) -> Result<Url> {
    let url = cfg.url.as_deref().context("webhook sink requires url")?;
    // The parse error text does not contain the URL.
    let url = Url::parse(url).map_err(|err| anyhow!("invalid webhook url: {err}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        bail!("webhook url must use http or https");
    }
    Ok(url)
}

impl WebhookSink {
    fn new(url: Url, cfg: &SinkConfig, client: Client) -> Self {
        let host = url.host_str().unwrap_or_default();
        let label = match url.port() {
            Some(port) => format!("{}://{host}:{port}", url.scheme()),
            None => format!("{}://{host}", url.scheme()),
        };
        Self {
            url,
            label,
            filter: SinkFilter::from(cfg),
            client,
        }
    }

    async fn post_with_retry(&self, event: &IndexEvent) -> Result<()> {
        let mut delay = Duration::from_millis(250);
        let mut last_err = None;

        for attempt in 1..=WEBHOOK_ATTEMPTS {
            match self.client.post(self.url.clone()).json(event).send().await {
                Ok(resp) if resp.status().is_success() => return Ok(()),
                Ok(resp) => {
                    last_err = Some(anyhow!("webhook returned HTTP {}", resp.status()));
                }
                Err(err) => {
                    // reqwest error text contains the full URL.
                    last_err = Some(err.without_url().into());
                }
            }

            if attempt < WEBHOOK_ATTEMPTS {
                tokio::time::sleep(delay).await;
                delay *= 2;
            }
        }

        let err = last_err.unwrap_or_else(|| anyhow!("webhook failed"));
        Err(err.context(format!("webhook {}", self.label)))
    }
}

#[async_trait]
impl EventSink for WebhookSink {
    async fn send(&self, event: &IndexEvent) -> Result<()> {
        if !matches_filter(event, &self.filter) {
            return Ok(());
        }
        self.post_with_retry(event).await
    }
}

/// The error names every bad entry.
pub fn validate_sinks(configs: &[SinkConfig]) -> Result<()> {
    let errors: Vec<String> = configs
        .iter()
        .enumerate()
        .filter_map(|(index, cfg)| {
            check_sink(cfg)
                .err()
                .map(|err| format!("sinks[{index}]: {err}"))
        })
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        bail!("{}", errors.join("; "))
    }
}

fn check_sink(cfg: &SinkConfig) -> Result<()> {
    match cfg.kind.as_str() {
        "stdout" => Ok(()),
        "webhook" => webhook_url(cfg).map(|_| ()),
        other => bail!("unknown sink kind: {other}"),
    }
}

/// A bad entry stops startup, so a config mistake is never silently dropped.
pub fn build_sinks(configs: &[SinkConfig]) -> Result<SinkFanout> {
    validate_sinks(configs)?;
    if configs.is_empty() {
        return Ok(SinkFanout::new(Vec::new()));
    }
    let client = webhook_client()?;
    let sinks = configs
        .iter()
        .map(|cfg| build_sink(cfg, &client))
        .collect::<Result<Vec<_>>>()?;
    Ok(SinkFanout::new(sinks))
}

fn build_sink(cfg: &SinkConfig, client: &Client) -> Result<Box<dyn EventSink + Send + Sync>> {
    match cfg.kind.as_str() {
        "stdout" => Ok(Box::new(StdoutSink::new(cfg))),
        "webhook" => Ok(Box::new(WebhookSink::new(
            webhook_url(cfg)?,
            cfg,
            client.clone(),
        ))),
        other => bail!("unknown sink kind: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::event::IndexEvent;

    fn token_transfer_event() -> IndexEvent {
        IndexEvent::TokenTransfer {
            slot: 1,
            signature: "sig".into(),
            ix_index: 0,
            ix_path: "0".into(),
            program: "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA".into(),
            source: "a".into(),
            destination: "b".into(),
            authority: None,
            mint: None,
            amount: 1,
            decimals: None,
        }
    }

    fn program_ix_event() -> IndexEvent {
        IndexEvent::ProgramIx {
            slot: 2,
            signature: "sig2".into(),
            ix_index: 0,
            ix_path: "0".into(),
            program_id: "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P".into(),
            program_name: "pump".into(),
            instruction: "buy".into(),
            accounts: vec![],
            args: None,
            named_accounts: None,
            failed: false,
        }
    }

    #[test]
    fn empty_filter_matches_all() {
        let filter = SinkFilter::default();
        assert!(matches_filter(&token_transfer_event(), &filter));
        assert!(matches_filter(&program_ix_event(), &filter));
    }

    #[test]
    fn kind_filter_rejects_other_kinds() {
        let filter = SinkFilter {
            kinds: Some(vec!["token_transfer".into()]),
            programs: None,
            instructions: None,
        };
        assert!(matches_filter(&token_transfer_event(), &filter));
        assert!(!matches_filter(&program_ix_event(), &filter));
    }

    #[test]
    fn program_filter_matches_program_field() {
        let filter = SinkFilter {
            kinds: None,
            programs: Some(vec!["pump".into()]),
            instructions: None,
        };
        assert!(matches_filter(&program_ix_event(), &filter));
        assert!(!matches_filter(&token_transfer_event(), &filter));
    }

    #[test]
    fn combined_filters_require_both() {
        let filter = SinkFilter {
            kinds: Some(vec!["program_ix".into()]),
            programs: Some(vec!["pump".into()]),
            instructions: None,
        };
        assert!(matches_filter(&program_ix_event(), &filter));
        assert!(!matches_filter(&token_transfer_event(), &filter));
    }

    #[test]
    fn instruction_filter_matches_program_ix() {
        let filter = SinkFilter {
            kinds: None,
            programs: None,
            instructions: Some(vec!["buy".into()]),
        };
        assert!(matches_filter(&program_ix_event(), &filter));
        assert!(!matches_filter(&token_transfer_event(), &filter));
    }

    #[test]
    fn instruction_filter_rejects_other_instructions() {
        let filter = SinkFilter {
            kinds: None,
            programs: None,
            instructions: Some(vec!["sell".into()]),
        };
        assert!(!matches_filter(&program_ix_event(), &filter));
    }

    fn sink_config(value: serde_json::Value) -> SinkConfig {
        serde_json::from_value(value).unwrap()
    }

    #[tokio::test]
    async fn build_sinks_rejects_and_names_every_bad_entry() {
        let configs = [
            sink_config(serde_json::json!({ "kind": "stdout" })),
            sink_config(serde_json::json!({ "kind": "bogus" })),
            sink_config(serde_json::json!({ "kind": "webhook" })),
            sink_config(serde_json::json!({ "kind": "webhook", "url": "not a url" })),
            sink_config(serde_json::json!({ "kind": "webhook", "url": "ftp://example.com/x" })),
            sink_config(serde_json::json!({ "kind": "webhook", "url": "https://example.com/x" })),
        ];
        let err = build_sinks(&configs).err().unwrap().to_string();
        for index in 1..=4 {
            assert!(err.contains(&format!("sinks[{index}]")), "{err}");
        }
        assert!(
            !err.contains("sinks[0]") && !err.contains("sinks[5]"),
            "{err}"
        );

        let valid = [configs[0].clone(), configs[5].clone()];
        assert_eq!(build_sinks(&valid).unwrap().sinks.len(), 2);
    }

    #[tokio::test]
    async fn webhook_error_hides_url_secret() {
        // Bind then drop, so the port refuses connections.
        let addr = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let cfg = sink_config(serde_json::json!({
            "kind": "webhook",
            "url": format!("http://{addr}/hook/secret-token?key=hunter2"),
        }));
        let sink = WebhookSink::new(webhook_url(&cfg).unwrap(), &cfg, webhook_client().unwrap());
        let err = sink.send(&token_transfer_event()).await.unwrap_err();
        let text = format!("{err:#}");
        assert!(
            !text.contains("secret-token") && !text.contains("hunter2"),
            "{text}"
        );
        assert!(text.contains("127.0.0.1"), "{text}");
    }

    #[tokio::test]
    async fn webhook_does_not_follow_redirects() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let followed = Arc::new(AtomicUsize::new(0));
        let hits = followed.clone();
        let app = axum::Router::new()
            .route(
                "/hook",
                axum::routing::post(|| async { axum::response::Redirect::temporary("/ok") }),
            )
            .route(
                "/ok",
                axum::routing::post(move || {
                    hits.fetch_add(1, Ordering::SeqCst);
                    async { "ok" }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let cfg = sink_config(serde_json::json!({
            "kind": "webhook",
            "url": format!("http://{addr}/hook"),
        }));
        let sink = WebhookSink::new(webhook_url(&cfg).unwrap(), &cfg, webhook_client().unwrap());
        let err = sink.send(&token_transfer_event()).await.unwrap_err();
        assert!(format!("{err:#}").contains("307"), "{err:#}");
        assert_eq!(followed.load(Ordering::SeqCst), 0);
    }
}
