use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use solana_client::client_error::{ClientError, ClientErrorKind};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_config::RpcBlockConfig;
use solana_client::rpc_custom_error::{
    JSON_RPC_SERVER_ERROR_BLOCK_CLEANED_UP, JSON_RPC_SERVER_ERROR_LONG_TERM_STORAGE_SLOT_SKIPPED,
    JSON_RPC_SERVER_ERROR_SLOT_SKIPPED, JSON_RPC_SERVER_ERROR_TRANSACTION_HISTORY_NOT_AVAILABLE,
};
use solana_client::rpc_request::RpcError;
use solana_commitment_config::CommitmentConfig;
use solana_transaction_status::option_serializer::OptionSerializer;
use solana_transaction_status::{
    EncodedTransaction, EncodedTransactionWithStatusMeta, TransactionDetails,
    UiCompiledInstruction, UiConfirmedBlock, UiInstruction, UiMessage, UiParsedInstruction,
    UiTransactionEncoding, UiTransactionStatusMeta,
};
use tokio::sync::{broadcast, mpsc};
use tokio::time::MissedTickBehavior;
use tracing::{info, warn};

use super::event::{
    failed_tx_policy, resolve_accounts, signature_from_base58, IndexEvent, RawInstruction,
};
use super::idl::IdlRouter;
use super::ingest::decode_instructions;
use super::ix_path::{assign_inner_paths, IxPath};
use super::store::EventStore;

const BACKFILL_RATE: Duration = Duration::from_millis(200); // 5 req/s
const RETRY_BASE: Duration = Duration::from_millis(500);
const RETRY_MAX: Duration = Duration::from_secs(8);
/// About 25s of retries: a recent slot is not served until the RPC node confirms or roots it.
const MAX_ATTEMPTS: u32 = 7;
/// Open gaps are read from the store again at this interval, so a gap that
/// failed or was not queued gets a retry while the process runs.
const GAP_RESCAN: Duration = Duration::from_secs(60);
/// Slots per repair step. The rest of a larger gap is saved as a new open gap.
const CHUNK_SLOTS: u64 = 1_000;
/// Times a gap end may answer "skipped" before it is taken as really skipped.
const MAX_END_SKIPS: u32 = 3;

pub struct BackfillWorker {
    rpc_url: String,
    store: Arc<EventStore>,
    router: IdlRouter,
    track_tokens: bool,
    track_failed: bool,
    bus: Option<broadcast::Sender<IndexEvent>>,
    sinks: Option<mpsc::Sender<IndexEvent>>,
    max_attempts: u32,
    chunk_slots: u64,
}

/// Result of one slot fetch, after retries.
enum Fetched {
    Block(UiConfirmedBlock),
    Skipped,
    /// The node will never serve the slot. It is logged and not retried.
    Unavailable(String),
    /// A later attempt can still succeed.
    Failed(String),
}

impl BackfillWorker {
    pub fn new(
        rpc_url: String,
        store: Arc<EventStore>,
        router: IdlRouter,
        track_tokens: bool,
    ) -> Self {
        Self {
            rpc_url,
            store,
            router,
            track_tokens,
            track_failed: false,
            bus: None,
            sinks: None,
            max_attempts: MAX_ATTEMPTS,
            chunk_slots: CHUNK_SLOTS,
        }
    }

    /// Index failed transactions the same way live ingest does.
    pub fn with_track_failed(mut self, track_failed: bool) -> Self {
        self.track_failed = track_failed;
        self
    }

    /// Send new repaired events to the live bus and sinks. Sinks get `try_send`
    /// as in live ingest, so a full sink queue drops the event and never stalls repair.
    pub fn with_publish(
        mut self,
        bus: broadcast::Sender<IndexEvent>,
        sinks: Option<mpsc::Sender<IndexEvent>>,
    ) -> Self {
        self.bus = Some(bus);
        self.sinks = sinks;
        self
    }

    pub async fn run(self, gap_rx: mpsc::Receiver<(u64, u64)>) -> Result<()> {
        let client = RpcClient::new(self.rpc_url.clone());
        self.repair(gap_rx, GAP_RESCAN, |slot| {
            client.get_block_with_config(slot, block_config())
        })
        .await;
        Ok(())
    }

    /// Repair gaps one chunk at a time until `gap_rx` closes. When it has no work,
    /// the worker reads open gaps from the store, at most once per `rescan`. The
    /// first read is at once, so gaps still open from an earlier run are repaired too.
    async fn repair<F, Fut>(
        &self,
        mut gap_rx: mpsc::Receiver<(u64, u64)>,
        rescan: Duration,
        fetch: F,
    ) where
        F: Fn(u64) -> Fut,
        Fut: Future<Output = Result<UiConfirmedBlock, ClientError>>,
    {
        let (queue_tx, mut incoming) = mpsc::unbounded_channel();
        // Drain the bounded channel at once, so a long repair never fills it.
        tokio::spawn(async move {
            while let Some(gap) = gap_rx.recv().await {
                if queue_tx.send(gap).is_err() {
                    break;
                }
            }
        });
        let mut queue = VecDeque::new();
        // Gaps queued since the last scan. A late copy from the channel of a gap
        // that is already done is not repaired again.
        let mut queued = HashSet::new();
        let mut end_skips = HashMap::new();
        let mut scan = tokio::time::interval(rescan);
        scan.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            while let Ok(gap) = incoming.try_recv() {
                push_gap(&mut queue, &mut queued, gap);
            }
            let Some((from, to)) = queue.pop_front() else {
                tokio::select! {
                    gap = incoming.recv() => match gap {
                        Some(gap) => push_gap(&mut queue, &mut queued, gap),
                        None => return,
                    },
                    // Nothing is queued or in progress here, so the store list has no duplicate.
                    _ = scan.tick() => {
                        queued.clear();
                        self.queue_open_gaps(&mut queue, &mut queued).await;
                    }
                }
                continue;
            };
            if let Some(rest) = self.repair_chunk(from, to, &fetch, &mut end_skips).await {
                push_gap(&mut queue, &mut queued, rest);
            }
        }
    }

    /// The store is the record of open gaps. This read also gets gaps that failed
    /// before, and gaps that run.rs could not queue.
    async fn queue_open_gaps(
        &self,
        queue: &mut VecDeque<(u64, u64)>,
        queued: &mut HashSet<(u64, u64)>,
    ) {
        match self.store.backfill_gaps().await {
            Ok(gaps) => {
                for gap in gaps {
                    push_gap(queue, queued, (gap.from_slot, gap.to_slot));
                }
            }
            Err(err) => warn!(error = %err, "backfill could not load open gaps"),
        }
    }

    /// Repair the first chunk of gap `(from, to)`. Slots that can still succeed and
    /// the rest of the gap are saved as new open gaps, then the original gap is
    /// closed. Returns the rest, so the caller can queue it behind other gaps.
    async fn repair_chunk<F, Fut>(
        &self,
        from: u64,
        to: u64,
        fetch: &F,
        end_skips: &mut HashMap<u64, u32>,
    ) -> Option<(u64, u64)>
    where
        F: Fn(u64) -> Fut,
        Fut: Future<Output = Result<UiConfirmedBlock, ClientError>>,
    {
        let end = to.min(from.saturating_add(self.chunk_slots.saturating_sub(1)));
        info!(from, to, end, "backfill gap started");
        let mut retry = Vec::new();
        for slot in from..=end {
            match fetch_with_retry(slot, fetch, self.max_attempts).await {
                Fetched::Block(block) => {
                    end_skips.remove(&slot);
                    if !self.store_block(slot, block).await {
                        retry.push(slot);
                    }
                }
                // A gap ends at a confirmed parent, so "skipped" there can mean the
                // node lags or lacks the block. Retry it a few times first.
                Fetched::Skipped if slot == to => {
                    let strikes = end_skips.entry(slot).or_insert(0);
                    *strikes += 1;
                    if *strikes < MAX_END_SKIPS {
                        warn!(slot, "backfill gap end reported skipped; retry later");
                        retry.push(slot);
                    } else {
                        end_skips.remove(&slot);
                        warn!(
                            slot,
                            "backfill gap end still reported skipped; taken as skipped"
                        );
                    }
                }
                Fetched::Skipped => {}
                Fetched::Unavailable(err) => {
                    warn!(slot, error = %err, "backfill slot not served by the RPC node; not retried");
                }
                Fetched::Failed(err) => {
                    warn!(slot, error = %err, "backfill slot fetch failed; retry later");
                    retry.push(slot);
                }
            }
        }

        let failed = slot_ranges(&retry);
        let rest = (end < to).then(|| (end + 1, to));
        for &(a, b) in failed.iter().chain(rest.iter()) {
            if let Err(err) = self.store.record_gap(a, b).await {
                // The original row stays open, so a later scan repairs the whole gap again.
                warn!(from = a, to = b, error = %err, "backfill could not save open range");
                return None;
            }
        }
        // `record_gap` keeps an existing row, so the original stays open if it is the failed range.
        if failed.contains(&(from, to)) {
            warn!(from, to, "backfill gap left open; retry later");
            return None;
        }
        match self.store.mark_gap_filled(from, to).await {
            Ok(()) => info!(
                from,
                end,
                open_ranges = failed.len(),
                "backfill chunk complete"
            ),
            Err(err) => warn!(from, to, error = %err, "backfill could not mark gap filled"),
        }
        rest
    }

    /// False when any event of the block could not be stored.
    async fn store_block(&self, slot: u64, block: UiConfirmedBlock) -> bool {
        let mut ixs = Vec::new();
        for tx in block.transactions.unwrap_or_default() {
            ixs.extend(failed_tx_policy(
                extract_instructions(slot, &tx),
                tx_failed(&tx),
                self.track_failed,
            ));
        }
        let events = decode_instructions(&self.router, &ixs, self.track_tokens);
        let event_count = events.len();
        let mut stored = true;
        for event in events {
            match self.store.write_event(&event).await {
                Ok(true) => self.publish(event),
                Ok(false) => {}
                Err(err) => {
                    warn!(slot, error = %err, "backfill event write failed");
                    stored = false;
                }
            }
        }
        info!(
            slot,
            instructions = ixs.len(),
            events = event_count,
            "backfilled slot"
        );
        stored
    }

    fn publish(&self, event: IndexEvent) {
        if let Some(bus) = &self.bus {
            let _ = bus.send(event.clone());
        }
        if let Some(sinks) = &self.sinks {
            let _ = sinks.try_send(event);
        }
    }
}

fn push_gap(queue: &mut VecDeque<(u64, u64)>, queued: &mut HashSet<(u64, u64)>, gap: (u64, u64)) {
    if queued.insert(gap) {
        queue.push_back(gap);
    }
}

/// Sorted slots as contiguous `(from, to)` ranges.
fn slot_ranges(slots: &[u64]) -> Vec<(u64, u64)> {
    let mut out: Vec<(u64, u64)> = Vec::new();
    for &slot in slots {
        match out.last_mut() {
            Some((_, end)) if end.checked_add(1) == Some(slot) => *end = slot,
            _ => out.push((slot, slot)),
        }
    }
    out
}

/// Confirmed matches the live stream. The RPC default (finalized) fails for
/// every slot of a fresh gap that is not rooted yet.
fn block_config() -> RpcBlockConfig {
    RpcBlockConfig {
        encoding: Some(UiTransactionEncoding::Json),
        transaction_details: Some(TransactionDetails::Full),
        rewards: Some(false),
        commitment: Some(CommitmentConfig::confirmed()),
        max_supported_transaction_version: Some(0),
    }
}

/// Error texts in the result have no RPC URL.
async fn fetch_with_retry<F, Fut>(slot: u64, fetch: &F, max_attempts: u32) -> Fetched
where
    F: Fn(u64) -> Fut,
    Fut: Future<Output = Result<UiConfirmedBlock, ClientError>>,
{
    let mut delay = RETRY_BASE;
    let mut attempt = 1;
    loop {
        tokio::time::sleep(BACKFILL_RATE).await;
        let err = match fetch(slot).await {
            Ok(block) => return Fetched::Block(block),
            Err(err) => err,
        };
        match rpc_error_code(&err) {
            Some(
                JSON_RPC_SERVER_ERROR_SLOT_SKIPPED
                | JSON_RPC_SERVER_ERROR_LONG_TERM_STORAGE_SLOT_SKIPPED,
            ) => return Fetched::Skipped,
            // The node will never serve these slots, so a retry only wastes quota.
            Some(
                JSON_RPC_SERVER_ERROR_BLOCK_CLEANED_UP
                | JSON_RPC_SERVER_ERROR_TRANSACTION_HISTORY_NOT_AVAILABLE,
            ) => return Fetched::Unavailable(redact(err)),
            _ if attempt >= max_attempts => return Fetched::Failed(redact(err)),
            _ => {}
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(RETRY_MAX);
        attempt += 1;
    }
}

fn rpc_error_code(err: &ClientError) -> Option<i64> {
    match err.kind() {
        ClientErrorKind::RpcError(RpcError::RpcResponseError { code, .. }) => Some(*code),
        _ => None,
    }
}

/// reqwest errors print the request URL, and RPC URLs often hold an API key.
pub(crate) fn redact(err: ClientError) -> String {
    match *err.kind {
        ClientErrorKind::Reqwest(err) => err.without_url().to_string(),
        kind => kind.to_string(),
    }
}

fn tx_failed(tx: &EncodedTransactionWithStatusMeta) -> bool {
    tx.meta.as_ref().is_some_and(|meta| meta.err.is_some())
}

fn extract_instructions(slot: u64, tx: &EncodedTransactionWithStatusMeta) -> Vec<RawInstruction> {
    let EncodedTransaction::Json(ui_tx) = &tx.transaction else {
        return Vec::new();
    };
    let first = ui_tx.signatures.first().map_or("", String::as_str);
    let Some(signature) = signature_from_base58(first, slot) else {
        return Vec::new();
    };
    let keys = account_keys(&ui_tx.message, tx.meta.as_ref());
    let mut out = Vec::new();

    match &ui_tx.message {
        UiMessage::Raw(raw) => {
            for (outer_index, ix) in raw.instructions.iter().enumerate() {
                if let Some(raw_ix) =
                    compiled_ui_to_raw(slot, &signature, IxPath::root(outer_index as u32), &keys, ix)
                {
                    out.push(raw_ix);
                }
            }
        }
        UiMessage::Parsed(parsed) => {
            for (outer_index, ui_ix) in parsed.instructions.iter().enumerate() {
                if let Some(raw_ix) = ui_instruction_to_raw(
                    slot,
                    &signature,
                    IxPath::root(outer_index as u32),
                    &keys,
                    ui_ix,
                ) {
                    out.push(raw_ix);
                }
            }
        }
    }

    if let Some(meta) = tx.meta.as_ref() {
        let inner: Option<Vec<_>> = meta.inner_instructions.clone().into();
        if let Some(groups) = inner {
            for group in groups {
                let heights: Vec<Option<u32>> =
                    group.instructions.iter().map(ui_stack_height).collect();
                let paths = assign_inner_paths(u32::from(group.index), &heights);
                for (ui_ix, path) in group.instructions.iter().zip(paths) {
                    if let Some(raw_ix) =
                        ui_instruction_to_raw(slot, &signature, path, &keys, ui_ix)
                    {
                        out.push(raw_ix);
                    }
                }
            }
        }
    }

    out
}

fn ui_stack_height(ix: &UiInstruction) -> Option<u32> {
    match ix {
        UiInstruction::Compiled(compiled) => compiled.stack_height,
        UiInstruction::Parsed(UiParsedInstruction::PartiallyDecoded(partial)) => {
            partial.stack_height
        }
        UiInstruction::Parsed(UiParsedInstruction::Parsed(parsed)) => parsed.stack_height,
    }
}

fn account_keys(message: &UiMessage, meta: Option<&UiTransactionStatusMeta>) -> Vec<String> {
    let mut keys = match message {
        UiMessage::Raw(raw) => raw.account_keys.clone(),
        UiMessage::Parsed(parsed) => parsed
            .account_keys
            .iter()
            .map(|account| account.pubkey.clone())
            .collect(),
    };

    if let Some(meta) = meta
        && let OptionSerializer::Some(loaded) = &meta.loaded_addresses {
            keys.extend(loaded.writable.iter().cloned());
            keys.extend(loaded.readonly.iter().cloned());
        }

    keys
}

fn compiled_ui_to_raw(
    slot: u64,
    signature: &str,
    path: IxPath,
    keys: &[String],
    ix: &UiCompiledInstruction,
) -> Option<RawInstruction> {
    let program_id = keys.get(ix.program_id_index as usize)?.clone();
    let accounts = resolve_accounts(keys, &ix.accounts, slot, signature)?;
    let data = bs58::decode(&ix.data).into_vec().ok()?;
    Some(RawInstruction {
        slot,
        signature: signature.to_string(),
        path,
        program_id,
        accounts,
        data,
        failed: false,
    })
}

fn ui_instruction_to_raw(
    slot: u64,
    signature: &str,
    path: IxPath,
    keys: &[String],
    ix: &UiInstruction,
) -> Option<RawInstruction> {
    match ix {
        UiInstruction::Compiled(compiled) => {
            compiled_ui_to_raw(slot, signature, path, keys, compiled)
        }
        UiInstruction::Parsed(UiParsedInstruction::PartiallyDecoded(partial)) => {
            let data = bs58::decode(&partial.data).into_vec().ok()?;
            Some(RawInstruction {
                slot,
                signature: signature.to_string(),
                path,
                program_id: partial.program_id.clone(),
                accounts: partial.accounts.clone(),
                data,
                failed: false,
            })
        }
        UiInstruction::Parsed(UiParsedInstruction::Parsed(_)) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::testutil::TempDb;
    use solana_client::rpc_request::RpcResponseErrorData;

    fn rpc_error(code: i64) -> ClientError {
        ClientErrorKind::RpcError(RpcError::RpcResponseError {
            code,
            message: String::new(),
            data: RpcResponseErrorData::Empty,
        })
        .into()
    }

    fn empty_block() -> UiConfirmedBlock {
        UiConfirmedBlock {
            previous_blockhash: String::new(),
            blockhash: String::new(),
            parent_slot: 0,
            transactions: Some(Vec::new()),
            signatures: None,
            rewards: None,
            num_reward_partitions: None,
            block_time: None,
            block_height: None,
        }
    }

    async fn temp_store() -> (Arc<EventStore>, TempDb) {
        let db = TempDb::new();
        let store = EventStore::open(&db.url).await.unwrap();
        (Arc::new(store), db)
    }

    async fn open_gaps(store: &EventStore) -> Vec<(u64, u64)> {
        let gaps = store.list_gaps(true).await.unwrap();
        gaps.into_iter().map(|g| (g.from_slot, g.to_slot)).collect()
    }

    #[tokio::test]
    async fn partial_failure_leaves_only_failed_ranges_open() {
        use solana_client::rpc_custom_error::JSON_RPC_SERVER_ERROR_BLOCK_NOT_AVAILABLE;

        let (store, _db) = temp_store().await;
        let mut worker =
            BackfillWorker::new(String::new(), store.clone(), IdlRouter::default(), false);
        worker.max_attempts = 1;
        worker.chunk_slots = 4;
        let fetch = |slot: u64| async move {
            match slot {
                2 | 3 => Err(rpc_error(JSON_RPC_SERVER_ERROR_BLOCK_NOT_AVAILABLE)),
                4 => Err(rpc_error(JSON_RPC_SERVER_ERROR_BLOCK_CLEANED_UP)),
                6 => Err(rpc_error(JSON_RPC_SERVER_ERROR_SLOT_SKIPPED)),
                _ => Ok(empty_block()),
            }
        };
        let mut end_skips = HashMap::new();
        store.record_gap(1, 6).await.unwrap();

        // First chunk 1..=4: 2-3 can still succeed, 4 is never served, 5-6 are not done yet.
        let rest = worker.repair_chunk(1, 6, &fetch, &mut end_skips).await;
        assert_eq!(rest, Some((5, 6)));
        assert_eq!(open_gaps(&store).await, [(2, 3), (5, 6)]);
        // Slot 6 ends the gap, so "skipped" there is retried, not taken as done.
        assert_eq!(
            worker.repair_chunk(5, 6, &fetch, &mut end_skips).await,
            None
        );
        assert_eq!(open_gaps(&store).await, [(2, 3), (6, 6)]);
    }

    #[tokio::test]
    async fn unavailable_gap_is_closed_and_not_requeued() {
        use std::sync::atomic::{AtomicU32, Ordering};

        let (store, _db) = temp_store().await;
        let worker = BackfillWorker::new(String::new(), store.clone(), IdlRouter::default(), false);
        let calls = AtomicU32::new(0);
        let fetch = |_slot: u64| {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err(rpc_error(JSON_RPC_SERVER_ERROR_BLOCK_CLEANED_UP)) }
        };
        store.record_gap(10, 12).await.unwrap();
        let (gap_tx, gap_rx) = mpsc::channel(4);
        let check = async {
            while calls.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            // A late channel copy of the gap in progress must not repeat the repair.
            gap_tx.send((10, 12)).await.unwrap();
            while !open_gaps(&store).await.is_empty() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            // Several scans run here; a closed gap must not come back.
            tokio::time::sleep(Duration::from_millis(400)).await;
        };
        let result = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::select! {
                _ = worker.repair(gap_rx, Duration::from_millis(50), fetch) => panic!("repair stopped"),
                _ = check => {}
            }
        })
        .await;
        result.expect("unavailable gap was not closed");
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn open_gaps_are_retried_while_running() {
        use solana_client::rpc_custom_error::JSON_RPC_SERVER_ERROR_BLOCK_NOT_AVAILABLE;
        use std::sync::atomic::{AtomicU32, Ordering};

        let (store, _db) = temp_store().await;
        let mut worker =
            BackfillWorker::new(String::new(), store.clone(), IdlRouter::default(), false);
        worker.max_attempts = 1;
        let slot3_calls = AtomicU32::new(0);
        // The first fetch of slot 3 fails, so its gap stays open after the first pass.
        let fetch = |slot: u64| {
            let first = slot == 3 && slot3_calls.fetch_add(1, Ordering::SeqCst) == 0;
            async move {
                if first {
                    Err(rpc_error(JSON_RPC_SERVER_ERROR_BLOCK_NOT_AVAILABLE))
                } else {
                    Ok(empty_block())
                }
            }
        };
        let (gap_tx, gap_rx) = mpsc::channel(4);
        let check = async {
            // Let the first scan pass, so only a later scan can find these gaps.
            tokio::time::sleep(Duration::from_millis(100)).await;
            store.record_gap(3, 3).await.unwrap();
            gap_tx.send((3, 3)).await.unwrap();
            // As when run.rs drops a gap from a full queue: stored, never sent.
            store.record_gap(5, 5).await.unwrap();
            while !open_gaps(&store).await.is_empty() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        };
        let result = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::select! {
                _ = worker.repair(gap_rx, Duration::from_millis(50), fetch) => panic!("repair stopped"),
                _ = check => {}
            }
        })
        .await;
        drop(gap_tx);
        result.expect("open gaps were not retried");
        assert!(slot3_calls.load(Ordering::SeqCst) >= 2);
    }

    #[tokio::test]
    async fn redact_drops_rpc_url() {
        let Err(err) =
            solana_client::client_error::reqwest::get("http://127.0.0.1:1/?api-key=SECRET").await
        else {
            panic!("port 1 must refuse the connection");
        };
        let err = ClientError::from(ClientErrorKind::Reqwest(err));
        assert!(err.to_string().contains("SECRET"));
        assert!(!redact(err).contains("SECRET"));
    }
}
