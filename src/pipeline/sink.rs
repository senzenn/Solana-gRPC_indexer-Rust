use std::time::Duration;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use reqwest::Client;
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
        && !kinds.is_empty() && !kinds.iter().any(|k| k == event.kind()) {
            return false;
        }
    if let Some(programs) = &filter.programs
        && !programs.is_empty() && !programs.iter().any(|p| p == event.program()) {
            return false;
        }
    if let Some(instructions) = &filter.instructions
        && !instructions.is_empty() {
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
                warn!(error = %err, "event sink failed");
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

struct WebhookSink {
    url: String,
    filter: SinkFilter,
    client: Client,
}

impl WebhookSink {
    fn new(url: String, cfg: &SinkConfig) -> Self {
        Self {
            url,
            filter: SinkFilter::from(cfg),
            client: Client::new(),
        }
    }

    async fn post_with_retry(&self, event: &IndexEvent) -> Result<()> {
        let mut delay = Duration::from_millis(250);
        let mut last_err = None;

        for attempt in 0..3 {
            match self
                .client
                .post(&self.url)
                .json(event)
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => return Ok(()),
                Ok(resp) => {
                    last_err = Some(anyhow::anyhow!(
                        "webhook returned HTTP {}",
                        resp.status()
                    ));
                }
                Err(err) => {
                    last_err = Some(err.into());
                }
            }

            if attempt < 2 {
                tokio::time::sleep(delay).await;
                delay *= 2;
            }
        }

        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("webhook failed")))
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

pub fn build_sinks(configs: &[SinkConfig]) -> Result<SinkFanout> {
    let mut sinks: Vec<Box<dyn EventSink + Send + Sync>> = Vec::new();
    for cfg in configs {
        match cfg.kind.as_str() {
            "stdout" => sinks.push(Box::new(StdoutSink::new(cfg))),
            "webhook" => {
                let url = cfg
                    .url
                    .as_ref()
                    .context("webhook sink requires url")?
                    .clone();
                sinks.push(Box::new(WebhookSink::new(url, cfg)));
            }
            other => bail!("unknown sink kind: {other}"),
        }
    }
    Ok(SinkFanout::new(sinks))
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
            program_id: "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P".into(),
            program_name: "pump".into(),
            instruction: "buy".into(),
            accounts: vec![],
            args: None,
            named_accounts: None,
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
}
