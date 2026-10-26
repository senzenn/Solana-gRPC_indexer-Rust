use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use futures::{StreamExt, sink::SinkExt};
use tokio::sync::mpsc;
use tracing::{info, warn};
use yellowstone_grpc_proto::prelude::{
    CommitmentLevel, SubscribeRequest, SubscribeRequestFilterSlots,
    SubscribeRequestFilterTransactions, SubscribeRequestPing, SubscribeUpdate,
    subscribe_update::UpdateOneof,
};

use super::config_file::{CompressionKind, ReplayMode};
use super::event::RawInstruction;
use super::event::{
    TOKEN_2022_PROGRAM, TOKEN_PROGRAM, failed_tx_policy, resolve_accounts, signature_from_bytes,
};
use super::geyser;
use super::idl::IdlRouter;
use super::ix_path::{IxPath, assign_inner_paths};
pub use super::reorg::SlotCommitment;

pub enum IngestMsg {
    Slot {
        slot: u64,
        parent: Option<u64>,
        status: SlotCommitment,
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
    pub replay: ReplayMode,
    pub ping_secs: u64,
    pub compression: CompressionKind,
}

impl Default for IngestConfig {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            auth_token: String::new(),
            from_slot: None,
            wallets: Vec::new(),
            extra_programs: Vec::new(),
            accounts_include: Vec::new(),
            track_tokens: false,
            track_failed: false,
            replay: ReplayMode::Auto,
            ping_secs: 10,
            compression: CompressionKind::Zstd,
        }
    }
}

pub async fn run_ingest(
    cfg: IngestConfig,
    tx: mpsc::Sender<IngestMsg>,
    last_slot: Arc<AtomicU64>,
) -> Result<()> {
    let mut backoff = Backoff::default();

    loop {
        let from_slot = resume_slot(last_slot.load(Ordering::Relaxed), cfg.from_slot);
        let mut received = false;
        match stream_once(&cfg, from_slot, &tx, &mut received).await {
            Ok(()) => {
                warn!("geyser stream closed; reconnecting");
                let _ = tx.send(IngestMsg::Reconnect).await;
            }
            Err(err) => {
                warn!(error = %err, "geyser stream error; reconnecting");
                let _ = tx.send(IngestMsg::GeyserError).await;
                let _ = tx.send(IngestMsg::Reconnect).await;
            }
        }
        tokio::time::sleep(backoff.after_session(received)).await;
    }
}

const MIN_BACKOFF: Duration = Duration::from_millis(500);
const MAX_BACKOFF: Duration = Duration::from_secs(15);

/// Resume at the last seen slot, not the next one. Its transactions can be
/// partly received, and store writes are idempotent upserts.
fn resume_slot(last_seen: u64, configured: Option<u64>) -> Option<u64> {
    if last_seen == 0 {
        configured
    } else {
        Some(last_seen)
    }
}

struct Backoff {
    next: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self { next: MIN_BACKOFF }
    }
}

impl Backoff {
    /// Delay before the next connect. It resets only after a session that
    /// delivered a slot or transaction: the server Ping alone proves nothing.
    fn after_session(&mut self, received: bool) -> Duration {
        let delay = if received { MIN_BACKOFF } else { self.next };
        self.next = (delay * 2).min(MAX_BACKOFF);
        delay
    }
}

enum Action {
    ReplyPing,
    Forward(IngestMsg),
    Ignore,
}

/// Yellowstone sends a Ping first on every Subscribe, so only decoded slot or
/// transaction data sets `received`.
fn on_update(update: SubscribeUpdate, track_failed: bool, received: &mut bool) -> Action {
    match &update.update_oneof {
        Some(UpdateOneof::Ping(_)) => Action::ReplyPing,
        Some(UpdateOneof::Pong(_)) | None => Action::Ignore,
        Some(_) => match decode_update(update, track_failed) {
            Some(out) => {
                *received = true;
                Action::Forward(out)
            }
            None => Action::Ignore,
        },
    }
}

async fn stream_once(
    cfg: &IngestConfig,
    from_slot: Option<u64>,
    tx: &mpsc::Sender<IngestMsg>,
    received: &mut bool,
) -> Result<()> {
    let mut client = geyser::connect(&cfg.endpoint, &cfg.auth_token, cfg.compression).await?;

    match client.get_version().await {
        Ok(info) => info!(version = %info.version, "geyser GetVersion"),
        Err(err) => warn!(error = %err, "GetVersion skipped"),
    }

    let (replay_supported, first_available) = match client.subscribe_replay_info().await {
        // Yellowstone answers Ok with no slot when its replay buffer is off.
        Ok(info) => {
            info!(first_available = ?info.first_available, "geyser replay info");
            (info.first_available.is_some(), info.first_available)
        }
        Err(err) => {
            info!(error = %err, "geyser replay not advertised; from_slot off");
            (false, None)
        }
    };

    let mut from_slot =
        geyser::resolve_from_slot(from_slot, cfg.replay, replay_supported, first_available);
    let mut request = build_request(cfg, from_slot);
    if request.transactions.is_empty() {
        warn!("no program/wallet filters; subscribing to slots only (no transactions)");
    }

    let (mut subscribe_tx, mut stream) =
        match client.subscribe_with_request(Some(request.clone())).await {
            Ok(pair) => pair,
            Err(err) if from_slot.is_some() && geyser::is_replay_error(&err.to_string()) => {
                warn!(error = %err, "from_slot rejected; resubscribing from tip");
                from_slot = None;
                request = build_request(cfg, None);
                client
                    .subscribe_with_request(Some(request))
                    .await
                    .context("subscribe after from_slot fallback")?
            }
            Err(err) => return Err(err).context("subscribe"),
        };

    info!(from_slot = ?from_slot, "geyser subscribed");

    let mut ping_id = 0i32;
    let mut ping_tick = if cfg.ping_secs == 0 {
        None
    } else {
        Some(tokio::time::interval(Duration::from_secs(cfg.ping_secs)))
    };

    loop {
        tokio::select! {
            _ = async {
                if let Some(tick) = ping_tick.as_mut() {
                    tick.tick().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {
                ping_id = ping_id.wrapping_add(1);
                let _ = subscribe_tx
                    .send(SubscribeRequest {
                        ping: Some(SubscribeRequestPing { id: ping_id }),
                        ..Default::default()
                    })
                    .await;
            }
            msg = stream.next() => {
                let Some(msg) = msg else {
                    break;
                };
                let update = match msg {
                    Ok(update) => update,
                    // Yellowstone accepts Subscribe, then sends the from_slot rejection on
                    // the stream before any data (a Ping may come first).
                    Err(status)
                        if from_slot.is_some()
                            && !*received
                            && geyser::is_replay_error(status.message()) =>
                    {
                        warn!(error = %status, "from_slot rejected; resubscribing from tip");
                        from_slot = None;
                        (subscribe_tx, stream) = client
                            .subscribe_with_request(Some(build_request(cfg, None)))
                            .await
                            .context("subscribe after from_slot fallback")?;
                        continue;
                    }
                    Err(status) => return Err(status).context("geyser message"),
                };
                match on_update(update, cfg.track_failed, received) {
                    Action::ReplyPing => {
                        ping_id = ping_id.wrapping_add(1);
                        let _ = subscribe_tx
                            .send(SubscribeRequest {
                                ping: Some(SubscribeRequestPing { id: ping_id }),
                                ..Default::default()
                            })
                            .await;
                    }
                    Action::Forward(out) => {
                        if tx.send(out).await.is_err() {
                            anyhow::bail!("ingest consumer dropped");
                        }
                    }
                    Action::Ignore => {}
                }
            }
        }
    }
    Ok(())
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
    if !account_include.is_empty() {
        transactions.insert(
            "ix".to_string(),
            SubscribeRequestFilterTransactions {
                vote: Some(false),
                // Some(true) would stream failed transactions only; None streams both.
                failed: if cfg.track_failed { None } else { Some(false) },
                account_include,
                account_exclude: vec![],
                account_required: vec![],
                signature: None,
            },
        );
    }

    let mut slots = HashMap::new();
    slots.insert(
        "slots".to_string(),
        SubscribeRequestFilterSlots {
            // All slot statuses so we can track finalized vs confirmed.
            filter_by_commitment: Some(false),
            // Without this the server never sends SLOT_DEAD.
            interslot_updates: Some(true),
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

fn decode_update(update: SubscribeUpdate, track_failed: bool) -> Option<IngestMsg> {
    match update.update_oneof? {
        UpdateOneof::Slot(slot) => Some(IngestMsg::Slot {
            slot: slot.slot,
            parent: slot.parent.filter(|p| *p != 0),
            status: slot_commitment(slot.status)?,
        }),
        UpdateOneof::Transaction(tx_update) => {
            let info = tx_update.transaction?;
            if info.is_vote {
                return None;
            }
            let failed = info.meta.as_ref().is_some_and(|meta| meta.err.is_some());
            if failed && !track_failed {
                return None;
            }
            let slot = tx_update.slot;
            let signature = signature_from_bytes(&info.signature, slot)?;
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
            for (outer_index, compiled) in message.instructions.iter().enumerate() {
                if let Some(raw) = compiled_to_raw(
                    slot,
                    &signature,
                    IxPath::root(outer_index as u32),
                    &keys,
                    compiled.program_id_index,
                    &compiled.accounts,
                    &compiled.data,
                ) {
                    out.push(raw);
                }
            }
            if let Some(meta) = meta {
                for group in &meta.inner_instructions {
                    let heights: Vec<Option<u32>> = group
                        .instructions
                        .iter()
                        .map(|compiled| compiled.stack_height)
                        .collect();
                    let paths = assign_inner_paths(group.index, &heights);
                    for (compiled, path) in group.instructions.iter().zip(paths) {
                        if let Some(raw) = compiled_to_raw(
                            slot,
                            &signature,
                            path,
                            &keys,
                            compiled.program_id_index,
                            &compiled.accounts,
                            &compiled.data,
                        ) {
                            out.push(raw);
                        }
                    }
                }
            }
            let out = failed_tx_policy(out, failed, track_failed);
            if out.is_empty() {
                None
            } else {
                Some(IngestMsg::Instructions(out))
            }
        }
        _ => None,
    }
}

fn slot_commitment(status: i32) -> Option<SlotCommitment> {
    // geyser.SlotStatus: PROCESSED=0 CONFIRMED=1 FINALIZED=2 FIRST_SHRED_RECEIVED=3
    // COMPLETED=4 CREATED_BANK=5 DEAD=6. Skip 3-5: they only add slot-table writes.
    match status {
        0 => Some(SlotCommitment::Processed),
        1 => Some(SlotCommitment::Confirmed),
        2 => Some(SlotCommitment::Finalized),
        6 => Some(SlotCommitment::Dead),
        _ => None,
    }
}

fn compiled_to_raw(
    slot: u64,
    signature: &str,
    path: IxPath,
    keys: &[String],
    program_id_index: u32,
    accounts: &[u8],
    data: &[u8],
) -> Option<RawInstruction> {
    let program_id = keys.get(program_id_index as usize)?.clone();
    let accounts = resolve_accounts(keys, accounts, slot, signature)?;
    Some(RawInstruction {
        slot,
        signature: signature.to_string(),
        path,
        program_id,
        accounts,
        data: data.to_vec(),
        failed: false,
    })
}

pub fn decode_instructions(
    router: &IdlRouter,
    ixs: &[RawInstruction],
    track_tokens: bool,
) -> Vec<super::event::IndexEvent> {
    let mut events = Vec::new();
    for ix in ixs {
        if track_tokens && let Some(event) = super::token::parse_token_ix(ix) {
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
        let raw = compiled_to_raw(1, "sig", IxPath::root(0), &keys, 0, &[1, 2], &[3, 0]).unwrap();
        assert_eq!(raw.program_id, "prog");
        assert_eq!(raw.accounts, vec!["a", "b"]);
        assert_eq!(raw.data, vec![3, 0]);
        // A bad index must not shift "b" into the first account slot.
        assert!(compiled_to_raw(1, "sig", IxPath::root(0), &keys, 0, &[9, 2], &[3, 0]).is_none());
    }

    #[test]
    fn build_request_includes_configured_accounts_only() {
        let cfg = IngestConfig {
            wallets: vec!["wallet1".into()],
            extra_programs: vec!["prog1".into()],
            accounts_include: vec!["acct1".into()],
            track_failed: true,
            ..Default::default()
        };
        let req = build_request(&cfg, None);
        let filter = req.transactions.get("ix").unwrap();
        let slots = req.slots.get("slots").unwrap();
        assert_eq!(slots.filter_by_commitment, Some(false));
        assert_eq!(slots.interslot_updates, Some(true));
        // Both outcomes: Some(true) would drop every successful transaction.
        assert_eq!(filter.failed, None);
        assert!(filter.account_include.contains(&"wallet1".into()));
        assert!(filter.account_include.contains(&"acct1".into()));
        assert!(filter.account_include.contains(&"prog1".into()));
        assert!(!filter.account_include.contains(&TOKEN_PROGRAM.to_string()));
    }

    #[test]
    fn build_request_adds_token_programs_when_enabled() {
        let cfg = IngestConfig {
            track_tokens: true,
            ..Default::default()
        };
        let req = build_request(&cfg, None);
        let filter = req.transactions.get("ix").unwrap();
        assert_eq!(filter.failed, Some(false));
        assert!(filter.account_include.contains(&TOKEN_PROGRAM.to_string()));
        assert!(
            filter
                .account_include
                .contains(&TOKEN_2022_PROGRAM.to_string())
        );
    }

    #[test]
    fn decode_instructions_skips_token_ix_when_disabled() {
        let router = IdlRouter::default();
        let mut data = vec![3u8]; // SPL Token transfer tag
        data.extend_from_slice(&1_000u64.to_le_bytes());
        let ixs = vec![RawInstruction {
            slot: 42,
            signature: "sig".into(),
            path: IxPath::root(0),
            program_id: TOKEN_PROGRAM.into(),
            accounts: vec!["src".into(), "dst".into(), "auth".into()],
            data,
            failed: false,
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
            path: IxPath::root(0),
            program_id: TOKEN_PROGRAM.into(),
            accounts: vec!["src".into(), "dst".into(), "auth".into()],
            data,
            failed: false,
        }];
        let events = decode_instructions(&router, &ixs, true);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], IndexEvent::TokenTransfer { .. }));
        assert_eq!(events[0].ix_path(), Some("0"));
    }

    #[test]
    fn empty_filters_subscribe_slots_only() {
        let req = build_request(&IngestConfig::default(), None);
        assert!(req.transactions.is_empty());
        assert!(req.slots.contains_key("slots"));
    }

    #[test]
    fn slot_commitment_maps_geyser_status() {
        assert_eq!(slot_commitment(1), Some(SlotCommitment::Confirmed));
        assert_eq!(slot_commitment(2), Some(SlotCommitment::Finalized));
        assert_eq!(slot_commitment(6), Some(SlotCommitment::Dead));
        assert_eq!(slot_commitment(0), Some(SlotCommitment::Processed));
        assert_eq!(slot_commitment(4), None);
    }

    #[test]
    fn resume_replays_last_seen_slot() {
        assert_eq!(resume_slot(0, Some(7)), Some(7));
        assert_eq!(resume_slot(100, Some(7)), Some(100));
    }

    #[test]
    fn ping_then_error_keeps_growing_backoff() {
        use yellowstone_grpc_proto::prelude::{SubscribeUpdatePing, SubscribeUpdateSlot};

        let ping = SubscribeUpdate {
            update_oneof: Some(UpdateOneof::Ping(SubscribeUpdatePing {})),
            ..Default::default()
        };
        let mut backoff = Backoff::default();
        let mut delays = Vec::new();
        // Each session gets the server Ping, then the stream fails.
        for _ in 0..4 {
            let mut received = false;
            assert!(matches!(
                on_update(ping.clone(), false, &mut received),
                Action::ReplyPing
            ));
            delays.push(backoff.after_session(received));
        }
        let ms = Duration::from_millis;
        assert_eq!(delays, [ms(500), ms(1_000), ms(2_000), ms(4_000)]);

        let slot = SubscribeUpdate {
            update_oneof: Some(UpdateOneof::Slot(SubscribeUpdateSlot {
                slot: 7,
                status: 1,
                ..Default::default()
            })),
            ..Default::default()
        };
        let mut received = false;
        assert!(matches!(
            on_update(slot, false, &mut received),
            Action::Forward(IngestMsg::Slot { slot: 7, .. })
        ));
        assert_eq!(backoff.after_session(received), MIN_BACKOFF);
    }

    #[test]
    fn failed_tx_never_emits_token_instructions() {
        use yellowstone_grpc_proto::prelude::{
            CompiledInstruction, Message, SubscribeUpdateTransaction,
            SubscribeUpdateTransactionInfo, Transaction, TransactionError, TransactionStatusMeta,
        };

        let mut transfer = vec![3u8];
        transfer.extend_from_slice(&1_000_000u64.to_le_bytes());
        let token = bs58::decode(TOKEN_PROGRAM).into_vec().unwrap();
        let keys = vec![token, vec![1; 32], vec![2; 32], vec![3; 32], vec![4; 32]];
        let update = |err: Option<TransactionError>| SubscribeUpdate {
            update_oneof: Some(UpdateOneof::Transaction(SubscribeUpdateTransaction {
                slot: 9,
                transaction: Some(SubscribeUpdateTransactionInfo {
                    signature: vec![1; 64],
                    transaction: Some(Transaction {
                        message: Some(Message {
                            account_keys: keys.clone(),
                            instructions: vec![
                                CompiledInstruction {
                                    program_id_index: 0,
                                    accounts: vec![1, 2, 3],
                                    data: transfer.clone(),
                                },
                                CompiledInstruction {
                                    program_id_index: 4,
                                    accounts: vec![],
                                    data: vec![0],
                                },
                            ],
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                    meta: Some(TransactionStatusMeta {
                        err,
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            })),
            ..Default::default()
        };
        let failed = || Some(TransactionError { err: vec![1] });

        assert!(decode_update(update(failed()), false).is_none());
        let Some(IngestMsg::Instructions(ixs)) = decode_update(update(failed()), true) else {
            panic!("failed tx must still emit non-token instructions when tracked");
        };
        assert_eq!(ixs.len(), 1);
        assert_eq!(ixs[0].path, IxPath::root(1));
        assert!(ixs[0].failed);
        let Some(IngestMsg::Instructions(ixs)) = decode_update(update(None), false) else {
            panic!("successful tx must emit instructions");
        };
        assert_eq!(ixs[0].program_id, TOKEN_PROGRAM);
        assert!(!ixs[0].failed);
    }

    #[test]
    fn bad_signature_drops_transaction() {
        use crate::pipeline::event::signature_from_base58;
        use yellowstone_grpc_proto::prelude::{
            CompiledInstruction, Message, SubscribeUpdateTransaction,
            SubscribeUpdateTransactionInfo, Transaction,
        };

        let update = |signature: Vec<u8>| SubscribeUpdate {
            update_oneof: Some(UpdateOneof::Transaction(SubscribeUpdateTransaction {
                slot: 3,
                transaction: Some(SubscribeUpdateTransactionInfo {
                    signature,
                    transaction: Some(Transaction {
                        message: Some(Message {
                            account_keys: vec![vec![1; 32]],
                            instructions: vec![CompiledInstruction {
                                program_id_index: 0,
                                accounts: vec![],
                                data: vec![0],
                            }],
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            })),
            ..Default::default()
        };
        assert!(decode_update(update(vec![1; 64]), false).is_some());
        assert!(decode_update(update(vec![1; 65]), false).is_none());
        assert!(decode_update(update(vec![1; 4096]), false).is_none());
        // Backfill takes the RPC text; it must decode to exactly 64 bytes.
        let good = bs58::encode([7u8; 64]).into_string();
        assert_eq!(signature_from_base58(&good, 3), Some(good.clone()));
        assert_eq!(signature_from_base58(&"2".repeat(1_000), 3), None);
        assert_eq!(signature_from_base58("unknown", 3), None);
    }

    #[test]
    fn inner_paths_follow_stack_heights() {
        use yellowstone_grpc_proto::prelude::{
            CompiledInstruction, InnerInstruction, InnerInstructions, Message,
            SubscribeUpdateTransaction, SubscribeUpdateTransactionInfo, Transaction,
            TransactionStatusMeta,
        };

        let outer = |program_id_index| CompiledInstruction {
            program_id_index,
            accounts: vec![],
            data: vec![0],
        };
        let inner = |program_id_index, stack_height| InnerInstruction {
            program_id_index,
            accounts: vec![],
            data: vec![0],
            stack_height,
        };
        // Groups out of order, nested CPI, and missing heights.
        let groups = vec![
            InnerInstructions {
                index: 1,
                instructions: vec![
                    inner(1, Some(2)),
                    inner(2, Some(3)),
                    inner(1, Some(4)),
                    inner(2, Some(3)),
                    inner(1, Some(2)),
                ],
            },
            InnerInstructions {
                index: 0,
                instructions: vec![inner(2, None), inner(1, None)],
            },
        ];
        let update = SubscribeUpdate {
            update_oneof: Some(UpdateOneof::Transaction(SubscribeUpdateTransaction {
                slot: 5,
                transaction: Some(SubscribeUpdateTransactionInfo {
                    signature: vec![1; 64],
                    transaction: Some(Transaction {
                        message: Some(Message {
                            account_keys: vec![vec![1; 32], vec![2; 32], vec![3; 32]],
                            instructions: vec![outer(0), outer(1)],
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                    meta: Some(TransactionStatusMeta {
                        inner_instructions: groups,
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            })),
            ..Default::default()
        };
        let Some(IngestMsg::Instructions(ixs)) = decode_update(update, false) else {
            panic!("transaction must emit instructions");
        };
        let paths: Vec<String> = ixs.iter().map(|ix| ix.ix_path()).collect();
        assert_eq!(
            paths,
            [
                "0", "1", "1.0", "1.0.0", "1.0.0.0", "1.0.1", "1.1", "0.0", "0.1"
            ]
        );
    }
}
