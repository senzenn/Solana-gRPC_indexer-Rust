use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use futures::{sink::SinkExt, StreamExt};
use tokio::sync::mpsc;
use tonic::transport::ClientTlsConfig;
use tracing::{info, warn};
use yellowstone_grpc_client::GeyserGrpcClient;
use yellowstone_grpc_proto::prelude::{
    subscribe_update::UpdateOneof, CommitmentLevel, SubscribeRequest,
    SubscribeRequestFilterSlots, SubscribeRequestFilterTransactions, SubscribeUpdate,
};

use super::event::RawInstruction;
use super::idl::IdlRouter;
use super::event::{TOKEN_2022_PROGRAM, TOKEN_PROGRAM};

pub enum IngestMsg {
    Slot {
        slot: u64,
        #[allow(dead_code)]
        parent: Option<u64>,
    },
    Instructions(Vec<RawInstruction>),
    Reconnect,
    GeyserError,
}

pub struct IngestConfig {
    pub endpoint: String,
    pub auth_token: String,
    pub from_slot: Option<u64>,
    pub wallets: Vec<String>,
    pub extra_programs: Vec<String>,
    pub accounts_include: Vec<String>,
    pub track_tokens: bool,
    pub track_failed: bool,
}

pub async fn run_ingest(
    cfg: IngestConfig,
    tx: mpsc::Sender<IngestMsg>,
    last_slot: Arc<AtomicU64>,
) -> Result<()> {
    let mut backoff = Duration::from_millis(500);
    let max_backoff = Duration::from_secs(15);

    loop {
        let from_slot = {
            let slot = last_slot.load(Ordering::Relaxed);
            if slot == 0 {
                cfg.from_slot
            } else {
                Some(slot.saturating_add(1))
            }
        };
        match stream_once(&cfg, from_slot, &tx).await {
            Ok(()) => {
                warn!("geyser stream closed; reconnecting");
                let _ = tx.send(IngestMsg::Reconnect).await;
                backoff = Duration::from_millis(500);
            }
            Err(err) => {
                warn!(error = %err, "geyser stream error; reconnecting");
                let _ = tx.send(IngestMsg::GeyserError).await;
                let _ = tx.send(IngestMsg::Reconnect).await;
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(max_backoff);
    }
}

async fn stream_once(
    cfg: &IngestConfig,
    from_slot: Option<u64>,
    tx: &mpsc::Sender<IngestMsg>,
) -> Result<()> {
    info!(endpoint = %cfg.endpoint, from_slot = ?from_slot, "connecting geyser");
    let mut builder = GeyserGrpcClient::build_from_shared(cfg.endpoint.clone())?
        .x_token(Some(cfg.auth_token.clone()))?;
    if cfg.endpoint.starts_with("https://") {
        builder = builder.tls_config(ClientTlsConfig::new().with_native_roots())?;
    }
    let mut client = builder.connect().await.context("geyser connect")?;
    let (mut subscribe_tx, mut stream) = client.subscribe().await?;

    subscribe_tx
        .send(build_request(cfg, from_slot))
        .await
        .context("subscribe send")?;

    backoff_reset_note();

    while let Some(msg) = stream.next().await {
        let update = msg.context("geyser message")?;
        if let Some(out) = decode_update(update)
            && tx.send(out).await.is_err() {
                anyhow::bail!("ingest consumer dropped");
            }
    }
    Ok(())
}

fn backoff_reset_note() {
    info!("geyser subscribed");
}

fn build_request(cfg: &IngestConfig, from_slot: Option<u64>) -> SubscribeRequest {
    let mut account_include = cfg.wallets.clone();
    account_include.extend(cfg.accounts_include.iter().cloned());
    account_include.extend(cfg.extra_programs.iter().cloned());
    if cfg.track_tokens {
        account_include.push(TOKEN_PROGRAM.into());
        account_include.push(TOKEN_2022_PROGRAM.into());
    }

    let mut transactions = HashMap::new();
    transactions.insert(
        "ix".to_string(),
        SubscribeRequestFilterTransactions {
            vote: Some(false),
            failed: Some(cfg.track_failed),
            account_include,
            account_exclude: vec![],
            account_required: vec![],
            signature: None,
        },
    );

    let mut slots = HashMap::new();
    slots.insert(
        "slots".to_string(),
        SubscribeRequestFilterSlots {
            filter_by_commitment: Some(true),
            ..Default::default()
        },
    );

    SubscribeRequest {
        transactions,
        slots,
        commitment: Some(CommitmentLevel::Confirmed as i32),
        from_slot,
        ..Default::default()
    }
}

fn decode_update(update: SubscribeUpdate) -> Option<IngestMsg> {
    match update.update_oneof? {
        UpdateOneof::Slot(slot) => Some(IngestMsg::Slot {
            slot: slot.slot,
            parent: slot.parent.filter(|p| *p != 0),
        }),
        UpdateOneof::Transaction(tx_update) => {
            let info = tx_update.transaction?;
            if info.is_vote {
                return None;
            }
            let signature = bs58::encode(&info.signature).into_string();
            let slot = tx_update.slot;
            let tx = info.transaction.as_ref()?;
            let message = tx.message.as_ref()?;
            let meta = info.meta.as_ref();

            let mut keys: Vec<String> = message
                .account_keys
                .iter()
                .map(|k| bs58::encode(k).into_string())
                .collect();
            if let Some(meta) = meta {
                keys.extend(
                    meta.loaded_writable_addresses
                        .iter()
                        .map(|k| bs58::encode(k).into_string()),
                );
                keys.extend(
                    meta.loaded_readonly_addresses
                        .iter()
                        .map(|k| bs58::encode(k).into_string()),
                );
            }

            let mut out = Vec::new();
            let mut ix_index = 0u32;
            for compiled in &message.instructions {
                if let Some(raw) = compiled_to_raw(
                    slot,
                    &signature,
                    ix_index,
                    &keys,
                    compiled.program_id_index,
                    &compiled.accounts,
                    &compiled.data,
                ) {
                    out.push(raw);
                }
                ix_index += 1;
            }
            if let Some(meta) = meta {
                for inner in &meta.inner_instructions {
                    for compiled in &inner.instructions {
                        if let Some(raw) = compiled_to_raw(
                            slot,
                            &signature,
                            ix_index,
                            &keys,
                            compiled.program_id_index,
                            &compiled.accounts,
                            &compiled.data,
                        ) {
                            out.push(raw);
                        }
                        ix_index += 1;
                    }
                }
            }
            if out.is_empty() {
                None
            } else {
                Some(IngestMsg::Instructions(out))
            }
        }
        _ => None,
    }
}

fn compiled_to_raw(
    slot: u64,
    signature: &str,
    ix_index: u32,
    keys: &[String],
    program_id_index: u32,
    accounts: &[u8],
    data: &[u8],
) -> Option<RawInstruction> {
    let program_id = keys.get(program_id_index as usize)?.clone();
    let accounts = accounts
        .iter()
        .filter_map(|idx| keys.get(*idx as usize).cloned())
        .collect();
    Some(RawInstruction {
        slot,
        signature: signature.to_string(),
        ix_index,
        program_id,
        accounts,
        data: data.to_vec(),
    })
}

pub fn decode_instructions(
    router: &IdlRouter,
    ixs: &[RawInstruction],
    track_tokens: bool,
) -> Vec<super::event::IndexEvent> {
    let mut events = Vec::new();
    for ix in ixs {
        if track_tokens
            && let Some(event) = super::token::parse_token_ix(ix)
        {
            events.push(event);
            continue;
        }
        if let Some(event) = router.parse(ix) {
            events.push(event);
        }
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::event::IndexEvent;

    #[test]
    fn compiled_to_raw_resolves_accounts() {
        let keys = vec!["prog".into(), "a".into(), "b".into()];
        let raw = compiled_to_raw(1, "sig", 0, &keys, 0, &[1, 2], &[3, 0]).unwrap();
        assert_eq!(raw.program_id, "prog");
        assert_eq!(raw.accounts, vec!["a", "b"]);
        assert_eq!(raw.data, vec![3, 0]);
    }

    #[test]
    fn build_request_includes_configured_accounts_only() {
        let cfg = IngestConfig {
            endpoint: "http://localhost".into(),
            auth_token: String::new(),
            from_slot: None,
            wallets: vec!["wallet1".into()],
            extra_programs: vec!["prog1".into()],
            accounts_include: vec!["acct1".into()],
            track_tokens: false,
            track_failed: true,
        };
        let req = build_request(&cfg, None);
        let filter = req.transactions.get("ix").unwrap();
        assert_eq!(filter.failed, Some(true));
        assert!(filter.account_include.contains(&"wallet1".into()));
        assert!(filter.account_include.contains(&"acct1".into()));
        assert!(filter.account_include.contains(&"prog1".into()));
        assert!(!filter
            .account_include
            .contains(&TOKEN_PROGRAM.to_string()));
    }

    #[test]
    fn build_request_adds_token_programs_when_enabled() {
        let cfg = IngestConfig {
            endpoint: "http://localhost".into(),
            auth_token: String::new(),
            from_slot: None,
            wallets: vec![],
            extra_programs: vec![],
            accounts_include: vec![],
            track_tokens: true,
            track_failed: false,
        };
        let req = build_request(&cfg, None);
        let filter = req.transactions.get("ix").unwrap();
        assert!(filter
            .account_include
            .contains(&TOKEN_PROGRAM.to_string()));
        assert!(filter
            .account_include
            .contains(&TOKEN_2022_PROGRAM.to_string()));
    }

    #[test]
    fn decode_instructions_skips_token_ix_when_disabled() {
        let router = IdlRouter::default();
        let mut data = vec![3u8]; // SPL Token transfer tag
        data.extend_from_slice(&1_000u64.to_le_bytes());
        let ixs = vec![RawInstruction {
            slot: 42,
            signature: "sig".into(),
            ix_index: 0,
            program_id: TOKEN_PROGRAM.into(),
            accounts: vec!["src".into(), "dst".into(), "auth".into()],
            data,
        }];
        let events = decode_instructions(&router, &ixs, false);
        assert!(events.is_empty());
    }

    #[test]
    fn decode_instructions_parses_token_ix_when_enabled() {
        let router = IdlRouter::default();
        let mut data = vec![3u8]; // SPL Token transfer tag
        data.extend_from_slice(&1_000u64.to_le_bytes());
        let ixs = vec![RawInstruction {
            slot: 42,
            signature: "sig".into(),
            ix_index: 0,
            program_id: TOKEN_PROGRAM.into(),
            accounts: vec!["src".into(), "dst".into(), "auth".into()],
            data,
        }];
        let events = decode_instructions(&router, &ixs, true);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], IndexEvent::TokenTransfer { .. }));
    }
}
