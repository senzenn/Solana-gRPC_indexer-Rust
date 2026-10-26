use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use tokio::sync::{broadcast, mpsc, RwLock};
use tokio::task::{JoinError, JoinSet};
use tracing::{info, warn};

use super::auth::ApiKeyState;
use super::config_file::SinkConfig;
use super::event::IndexEvent;
use super::idl::IdlRouter;
use super::ingest::{decode_instructions, run_ingest, IngestConfig, IngestMsg, SlotCommitment};
use super::serve::{serve, AppState, LiveStats};
use super::sink::{build_sinks, SinkFanout};
use super::metrics as prom;
use super::store::{detect_gap, EventStore};

/// Sinks are best-effort: when this queue is full, events are dropped, not waited on.
const SINK_QUEUE: usize = 1024;
const GAP_QUEUE: usize = 64;
/// Limit on cached slot parents if confirmed updates stop.
const MAX_SLOT_PARENTS: usize = 4096;
/// A slot this far above the confirmed baseline is treated as corrupt (about 4.6 days).
const MAX_SLOT_JUMP: u64 = 1_000_000;
/// After this many far confirmed slots in a row, accept the jump: a long outage
/// moves the real tip that far too.
const JUMP_ACCEPT_STREAK: u32 = 32;

type TaskResult = (&'static str, Result<()>);

pub struct RunArgs {
    pub endpoint: String,
    pub auth_token: String,
    pub db: String,
    pub idl_dir: PathBuf,
    pub ws_bind: SocketAddr,
    pub wallets: Vec<String>,
    pub tui: bool,
    pub rpc_url: Option<String>,
    pub sinks: Vec<SinkConfig>,
    pub api_keys: Vec<String>,
    pub grpc_bind: Option<SocketAddr>,
    pub track_failed: bool,
    pub track_tokens: bool,
    pub accounts_include: Vec<String>,
    pub replay: super::config_file::ReplayMode,
    pub ping_secs: u64,
    pub compression: super::config_file::CompressionKind,
}

pub async fn run(args: RunArgs) -> Result<()> {
    let store = Arc::new(EventStore::open(&args.db).await?);
    let cursor = store.load_cursor().await?;
    // Ingest resumes at the cursor slot itself and replays it; writes are idempotent.
    let from_slot = cursor.as_ref().map(|c| c.slot).filter(|slot| *slot > 0);
    info!(resume_slot = ?from_slot, "loaded cursor");

    let router = IdlRouter::from_dir(&args.idl_dir)?;
    let extra_programs = router.program_ids();
    info!(programs = extra_programs.len(), "loaded IDL parsers");

    let sink_configs = sinks_for_mode(args.sinks, args.tui);
    let sink_tx = if sink_configs.is_empty() {
        None
    } else {
        let sinks = build_sinks(&sink_configs).context("invalid sinks config")?;
        let (tx, rx) = mpsc::channel::<IndexEvent>(SINK_QUEUE);
        tokio::spawn(deliver_sinks(sinks, rx));
        Some(tx)
    };
    let api_state = Arc::new(ApiKeyState::from_keys(args.api_keys));

    let (ingest_tx, mut ingest_rx) = mpsc::channel::<IngestMsg>(4096);
    let (bus, _) = broadcast::channel::<IndexEvent>(1024);
    let stats = Arc::new(RwLock::new(LiveStats {
        slot: cursor.as_ref().map(|c| c.slot).unwrap_or(0),
        finalized_slot: cursor.as_ref().map(|c| c.finalized_slot).unwrap_or(0),
        lag_slots: cursor
            .as_ref()
            .map(|c| c.slot.saturating_sub(c.finalized_slot))
            .unwrap_or(0),
        ..Default::default()
    }));
    prom::init();

    // Every task here is critical: when one ends, `run` ends with its result.
    let mut tasks: JoinSet<TaskResult> = JoinSet::new();

    let last_slot_atomic = Arc::new(AtomicU64::new(cursor.as_ref().map(|c| c.slot).unwrap_or(0)));
    let track_tokens = args.track_tokens;
    let ingest_cfg = IngestConfig {
        endpoint: args.endpoint,
        auth_token: args.auth_token,
        from_slot,
        wallets: args.wallets,
        extra_programs,
        accounts_include: args.accounts_include,
        track_tokens,
        track_failed: args.track_failed,
        replay: args.replay,
        ping_secs: args.ping_secs,
        compression: args.compression,
    };
    let ingest_slots = last_slot_atomic.clone();
    tasks.spawn(async move { ("ingest", run_ingest(ingest_cfg, ingest_tx, ingest_slots).await) });

    // Without a backfill worker nothing reads gaps, so do not queue them.
    let gap_tx = if let Some(rpc_url) = args.rpc_url.clone() {
        let (gap_tx, gap_rx) = mpsc::channel::<(u64, u64)>(GAP_QUEUE);
        let worker = super::backfill::BackfillWorker::new(
            rpc_url,
            store.clone(),
            router.clone(),
            track_tokens,
        )
        .with_track_failed(args.track_failed)
        .with_publish(bus.clone(), sink_tx.clone());
        tasks.spawn(async move { ("backfill", worker.run(gap_rx).await) });
        Some(gap_tx)
    } else {
        None
    };

    let http_state = AppState {
        events: bus.clone(),
        stats: stats.clone(),
        store: store.clone(),
    };
    let bind = args.ws_bind;
    warn_if_exposed("HTTP server", bind, &api_state);
    let keys = api_state.clone();
    tasks.spawn(async move { ("http server", serve(bind, http_state, keys).await) });

    if let Some(grpc_bind) = args.grpc_bind {
        warn_if_exposed("gRPC export", grpc_bind, &api_state);
        let export = super::grpc_export::ExportService {
            events: bus.clone(),
            store: store.clone(),
        };
        let keys = api_state.clone();
        tasks.spawn(async move {
            ("grpc export", super::grpc_export::serve(grpc_bind, export, keys).await)
        });
        info!(%grpc_bind, "gRPC export listening");
    }

    if args.tui {
        let rx = bus.subscribe();
        let stats_tui = stats.clone();
        tasks.spawn(async move { ("tui", super::tui::run_tui(rx, stats_tui).await) });
    } else {
        info!(bind = %args.ws_bind, "open http://{}/ for the live view", args.ws_bind);
    }

    let mut pipeline = Pipeline {
        store,
        router,
        track_tokens,
        bus,
        stats,
        gap_tx,
        sink_tx,
        resume_slot: last_slot_atomic,
        last_slot: cursor.map(|c| c.slot),
        slot_parents: BTreeMap::new(),
        sink_drops: 0,
        jump_streak: 0,
    };
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);

    loop {
        let msg = tokio::select! {
            res = &mut shutdown => {
                res?;
                info!("shutdown signal received; stopping");
                return Ok(());
            }
            Some(joined) = tasks.join_next() => return task_exit(joined),
            msg = ingest_rx.recv() => match msg {
                Some(msg) => msg,
                // The sender lives in the ingest task, so its result tells why the channel closed.
                None => match tasks.join_next().await {
                    Some(joined) => return task_exit(joined),
                    None => bail!("ingest channel closed"),
                },
            },
        };
        pipeline.handle(msg).await?;
    }
}

/// Per-message state of the ingest loop, apart from `run` so tests can drive it.
struct Pipeline {
    store: Arc<EventStore>,
    router: IdlRouter,
    track_tokens: bool,
    bus: broadcast::Sender<IndexEvent>,
    stats: Arc<RwLock<LiveStats>>,
    gap_tx: Option<mpsc::Sender<(u64, u64)>>,
    sink_tx: Option<mpsc::Sender<IndexEvent>>,
    resume_slot: Arc<AtomicU64>,
    last_slot: Option<u64>,
    slot_parents: BTreeMap<u64, u64>,
    sink_drops: u64,
    jump_streak: u32,
}

impl Pipeline {
    async fn handle(&mut self, msg: IngestMsg) -> Result<()> {
        if let Some(slot) = slot_out_of_range(&msg) {
            warn!(slot, "slot above i64::MAX; update skipped");
            return Ok(());
        }
        let jump_gap = match self.check_jump(&msg) {
            Jump::Near => None,
            Jump::Skip(slot) => {
                warn!(slot, last_slot = ?self.last_slot, "slot too far above the confirmed slot; update skipped");
                return Ok(());
            }
            Jump::Accept { from, to } => Some((from, to)),
        };

        match msg {
            IngestMsg::Reconnect => {
                prom::record_reconnect();
            }
            IngestMsg::GeyserError => {
                prom::record_geyser_error();
            }
            IngestMsg::Slot {
                slot,
                parent,
                status,
            } => {
                let store = &self.store;
                store.record_slot(slot, parent, status).await?;
                if let Some(parent) = parent {
                    self.slot_parents.insert(slot, parent);
                    if self.slot_parents.len() > MAX_SLOT_PARENTS {
                        self.slot_parents.pop_first();
                    }
                }
                match status {
                    SlotCommitment::Confirmed => {
                        // Agave sends confirmed updates with no parent; use the one from processed.
                        let parent = parent.or_else(|| self.slot_parents.get(&slot).copied());
                        let gap = jump_gap.or_else(|| confirmed_gap(self.last_slot, slot, parent));
                        if let Some((from, to)) = gap {
                            emit_gap(store, &self.bus, self.gap_tx.as_ref(), &self.stats, from, to)
                                .await?;
                        }
                        let s = slot.max(self.last_slot.unwrap_or(0));
                        self.last_slot = Some(s);
                        // Yellowstone sends all content of a slot before its confirmed status, so
                        // `s` is complete. Ingest resumes at `s`; the replay is idempotent.
                        self.resume_slot.store(s, Ordering::Relaxed);
                        store.commit_cursor(s).await?;
                        self.slot_parents.retain(|cached, _| *cached > s);
                    }
                    SlotCommitment::Finalized => {
                        let orphans = store.apply_finalized(slot).await?;
                        if !orphans.is_empty() {
                            prom::record_orphans(orphans.len() as u64);
                            warn!(slot, orphans = orphans.len(), "dropped orphan slots");
                            if let Some(s) = self.last_slot
                                && orphans.contains(&s)
                            {
                                self.last_slot = Some(slot);
                                self.resume_slot.store(slot, Ordering::Relaxed);
                            }
                        }
                    }
                    SlotCommitment::Dead => {
                        // The stream is at confirmed commitment and a dead slot never confirms,
                        // so it has no stored events. Finalization drops the row as an orphan.
                        warn!(slot, "dead slot");
                    }
                    SlotCommitment::Processed => {}
                }
                {
                    let mut st = self.stats.write().await;
                    st.slot = self.last_slot.unwrap_or(st.slot);
                    if let Ok(Some(cursor)) = store.load_cursor().await {
                        st.finalized_slot = cursor.finalized_slot;
                        st.lag_slots = st.slot.saturating_sub(cursor.finalized_slot);
                    }
                    st.open_gaps = store.open_gap_count().await.unwrap_or(st.open_gaps);
                    prom::update_live_stats(st.slot, st.finalized_slot, st.lag_slots, st.open_gaps);
                }
            }
            IngestMsg::Instructions(ixs) => {
                // Event slots do not move the gap baseline or the resume slot: a slot is
                // complete only at its confirmed status.
                let events = decode_instructions(&self.router, &ixs, self.track_tokens);
                for event in events {
                    let inserted = self.store.write_event(&event).await?;
                    if !inserted {
                        continue;
                    }
                    {
                        let mut s = self.stats.write().await;
                        s.events_total += 1;
                        s.slot = self.last_slot.unwrap_or(s.slot);
                        match &event {
                            IndexEvent::TokenTransfer { .. } => s.token_transfers += 1,
                            IndexEvent::ProgramIx { .. } => s.program_ix += 1,
                            _ => {}
                        }
                        s.lag_slots = s.slot.saturating_sub(s.finalized_slot);
                        prom::record_event_stored();
                        prom::update_live_stats(s.slot, s.finalized_slot, s.lag_slots, s.open_gaps);
                    }
                    let _ = self.bus.send(event.clone());
                    if let Some(tx) = &self.sink_tx
                        && let Err(err) = tx.try_send(event)
                    {
                        self.sink_drops += 1;
                        if self.sink_drops % 1000 == 1 {
                            warn!(dropped = self.sink_drops, error = %err, "sink queue unavailable; event not delivered to sinks");
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Checks for a slot more than MAX_SLOT_JUMP above the baseline. Only far confirmed
    /// slots count toward accepting the jump.
    fn check_jump(&mut self, msg: &IngestMsg) -> Jump {
        let Some(last) = self.last_slot else {
            return Jump::Near;
        };
        let far = |slot: u64| slot.saturating_sub(last) > MAX_SLOT_JUMP;
        match msg {
            IngestMsg::Slot { slot, status, .. } => {
                let confirmed = *status == SlotCommitment::Confirmed;
                if !far(*slot) {
                    if confirmed {
                        self.jump_streak = 0;
                    }
                    return Jump::Near;
                }
                if !confirmed {
                    return Jump::Skip(*slot);
                }
                self.jump_streak += 1;
                if self.jump_streak < JUMP_ACCEPT_STREAK {
                    return Jump::Skip(*slot);
                }
                self.jump_streak = 0;
                warn!(slot, last_slot = last, "far confirmed slots kept coming; accepting the jump");
                // Events of this slot and the skipped ones were dropped, so the gap includes `slot`.
                Jump::Accept {
                    from: last + 1,
                    to: *slot,
                }
            }
            IngestMsg::Instructions(ixs) => match ixs.iter().map(|ix| ix.slot).find(|s| far(*s)) {
                Some(slot) => Jump::Skip(slot),
                None => Jump::Near,
            },
            IngestMsg::Reconnect | IngestMsg::GeyserError => Jump::Near,
        }
    }
}

enum Jump {
    Near,
    /// Drop the update.
    Skip(u64),
    /// Far confirmed slots kept coming: take the slot and record this gap.
    Accept { from: u64, to: u64 },
}

/// Missing range before confirmed `slot`. With a known parent, the slots between
/// the parent and `slot` were skipped by their leaders, so they are not a gap.
fn confirmed_gap(last: Option<u64>, slot: u64, parent: Option<u64>) -> Option<(u64, u64)> {
    let last = last?;
    match parent {
        Some(parent) if parent < slot => (parent > last).then(|| (last + 1, parent)),
        _ => detect_gap(Some(last), slot),
    }
}

/// The store keeps slots as BIGINT and rejects larger values. Such a slot must
/// not reach the store, `last_slot` or the cursor.
fn slot_out_of_range(msg: &IngestMsg) -> Option<u64> {
    let fits = |slot: &u64| i64::try_from(*slot).is_ok();
    match msg {
        IngestMsg::Slot { slot, parent, .. } => {
            std::iter::once(*slot).chain(*parent).find(|s| !fits(s))
        }
        IngestMsg::Instructions(ixs) => ixs.iter().map(|ix| ix.slot).find(|s| !fits(s)),
        IngestMsg::Reconnect | IngestMsg::GeyserError => None,
    }
}

/// Map a finished background task to the result of `run`. Only a TUI quit is a clean stop.
fn task_exit(joined: Result<TaskResult, JoinError>) -> Result<()> {
    match joined {
        Ok(("tui", Ok(()))) => {
            info!("tui closed; stopping");
            Ok(())
        }
        Ok((name, Ok(()))) => bail!("{name} task stopped"),
        Ok((name, Err(err))) => Err(err.context(format!("{name} task failed"))),
        Err(err) => bail!("background task failed: {err}"),
    }
}

/// Under the TUI, stdout sink lines would draw over the screen.
fn sinks_for_mode(sinks: Vec<SinkConfig>, tui: bool) -> Vec<SinkConfig> {
    if !tui {
        return sinks;
    }
    let (stdout, rest): (Vec<_>, Vec<_>) = sinks.into_iter().partition(|s| s.kind == "stdout");
    if !stdout.is_empty() {
        warn!(skipped = stdout.len(), "stdout sinks are off under --tui");
    }
    rest
}

/// Runs sinks off the ingest loop, so a slow webhook cannot stall indexing.
async fn deliver_sinks(sinks: SinkFanout, mut rx: mpsc::Receiver<IndexEvent>) {
    while let Some(event) = rx.recv().await {
        let _ = sinks.send(&event).await;
    }
}

/// Auth is off when no key is set, so a non-loopback bind serves all data to the network.
pub(super) fn warn_if_exposed(service: &str, bind: SocketAddr, auth: &ApiKeyState) {
    if auth.keys.is_empty() && !bind.ip().is_loopback() {
        warn!(%bind, "{service} binds a non-loopback address with no API keys; data is open to the network");
    }
}

/// Resolves on Ctrl-C or SIGTERM. As container PID 1, SIGTERM has no default action.
pub(super) async fn shutdown_signal() -> Result<()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).context("install SIGTERM handler")?;
        tokio::select! {
            res = tokio::signal::ctrl_c() => res.context("wait for Ctrl-C")?,
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await.context("wait for Ctrl-C")?;
    Ok(())
}

async fn emit_gap(
    store: &EventStore,
    bus: &broadcast::Sender<IndexEvent>,
    gap_tx: Option<&mpsc::Sender<(u64, u64)>>,
    stats: &Arc<RwLock<LiveStats>>,
    from: u64,
    to: u64,
) -> Result<()> {
    let event = IndexEvent::Gap {
        from_slot: from,
        to_slot: to,
    };
    store.record_gap(from, to).await?;
    let _ = store.write_event(&event).await;
    let _ = bus.send(event);
    // Do not wait on the backfill queue: the gap row is stored and ingest must not stall.
    if let Some(gap_tx) = gap_tx
        && let Err(err) = gap_tx.try_send((from, to))
    {
        warn!(from, to, error = %err, "backfill queue unavailable; gap stays open in the store");
    }
    {
        let mut st = stats.write().await;
        st.open_gaps = store.open_gap_count().await.unwrap_or(st.open_gaps);
        prom::update_live_stats(st.slot, st.finalized_slot, st.lag_slots, st.open_gaps);
    }
    warn!(from, to, "slot gap");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn skipped_slots_are_not_gaps() {
        // 101 was skipped by its leader: 102 builds on 100.
        assert_eq!(confirmed_gap(Some(100), 102, Some(100)), None);
        // Fork switch: the parent is below the last confirmed slot.
        assert_eq!(confirmed_gap(Some(105), 106, Some(103)), None);
        assert_eq!(confirmed_gap(Some(100), 105, Some(103)), Some((101, 103)));
        assert_eq!(confirmed_gap(Some(100), 102, None), Some((101, 101)));
        assert_eq!(confirmed_gap(Some(100), 101, Some(100)), None);
        assert_eq!(confirmed_gap(None, 102, Some(100)), None);
    }

    #[test]
    fn slots_above_i64_max_are_skipped() {
        use crate::pipeline::event::RawInstruction;
        use crate::pipeline::ix_path::IxPath;

        let big = i64::MAX as u64 + 1;
        let slot = |slot, parent| IngestMsg::Slot {
            slot,
            parent,
            status: SlotCommitment::Confirmed,
        };
        let ixs = |slot| {
            IngestMsg::Instructions(vec![RawInstruction {
                slot,
                signature: "sig".into(),
                path: IxPath::root(0),
                program_id: "p".into(),
                accounts: vec![],
                data: vec![],
                failed: false,
            }])
        };
        assert_eq!(slot_out_of_range(&slot(i64::MAX as u64, Some(1))), None);
        assert_eq!(slot_out_of_range(&slot(u64::MAX, None)), Some(u64::MAX));
        assert_eq!(slot_out_of_range(&slot(5, Some(big))), Some(big));
        assert_eq!(slot_out_of_range(&ixs(7)), None);
        assert_eq!(slot_out_of_range(&ixs(big)), Some(big));
        assert_eq!(slot_out_of_range(&IngestMsg::Reconnect), None);
    }

    #[tokio::test]
    async fn emit_gap_does_not_block_when_gap_queue_is_full_or_absent() {
        let path = std::env::temp_dir().join(format!("index-run-gap-{}.db", std::process::id()));
        let store = EventStore::open(&format!("sqlite:{}", path.display()))
            .await
            .expect("open store");
        let (bus, _) = broadcast::channel(16);
        let stats = Arc::new(RwLock::new(LiveStats::default()));
        // Nothing reads this queue, as when the backfill worker is slow.
        let (gap_tx, _gap_rx) = mpsc::channel(1);
        let emit = async {
            for i in 0..4u64 {
                let base = i * 10;
                emit_gap(&store, &bus, None, &stats, base + 1, base + 2).await?;
                emit_gap(&store, &bus, Some(&gap_tx), &stats, base + 3, base + 4).await?;
            }
            anyhow::Ok(())
        };
        let result = tokio::time::timeout(Duration::from_secs(10), emit).await;
        let _ = std::fs::remove_file(&path);
        result
            .expect("emit_gap blocked on the gap queue")
            .expect("emit_gap");
    }

    fn temp_db(name: &str) -> (std::path::PathBuf, String) {
        let path = std::env::temp_dir().join(format!("index-run-{name}-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let url = format!("sqlite:{}", path.display());
        (path, url)
    }

    async fn pipeline(url: &str, last_slot: Option<u64>) -> Pipeline {
        let store = Arc::new(EventStore::open(url).await.expect("open store"));
        if let Some(slot) = last_slot {
            store.commit_cursor(slot).await.expect("cursor");
        }
        Pipeline {
            store,
            router: IdlRouter::default(),
            track_tokens: true,
            bus: broadcast::channel(16).0,
            stats: Arc::new(RwLock::new(LiveStats::default())),
            gap_tx: None,
            sink_tx: None,
            resume_slot: Arc::new(AtomicU64::new(last_slot.unwrap_or(0))),
            last_slot,
            slot_parents: BTreeMap::new(),
            sink_drops: 0,
            jump_streak: 0,
        }
    }

    fn transfer_at(slot: u64) -> IngestMsg {
        use crate::pipeline::event::{RawInstruction, TOKEN_PROGRAM};
        use crate::pipeline::ix_path::IxPath;
        let mut data = vec![3u8]; // SPL Token transfer
        data.extend_from_slice(&1_000u64.to_le_bytes());
        IngestMsg::Instructions(vec![RawInstruction {
            slot,
            signature: format!("sig-{slot}"),
            path: IxPath::root(0),
            program_id: TOKEN_PROGRAM.into(),
            accounts: vec!["src".into(), "dst".into(), "auth".into()],
            data,
            failed: false,
        }])
    }

    fn slot_msg(slot: u64, parent: Option<u64>, status: SlotCommitment) -> IngestMsg {
        IngestMsg::Slot {
            slot,
            parent,
            status,
        }
    }

    #[tokio::test]
    async fn event_slots_do_not_move_cursor_or_open_gaps() {
        let (path, url) = temp_db("events");
        let s = 1_000;
        let mut p = pipeline(&url, Some(s)).await;
        p.handle(transfer_at(s + 5)).await.expect("events");
        p.handle(slot_msg(s + 1, Some(s), SlotCommitment::Confirmed))
            .await
            .expect("slot");
        let cursor = p.store.load_cursor().await.expect("cursor").expect("some");
        let gaps = p.store.list_gaps(false).await.expect("gaps");
        let events = p.store.recent_events(10).await.expect("events");
        let _ = std::fs::remove_file(&path);
        assert_eq!(cursor.slot, s + 1);
        assert_eq!(p.resume_slot.load(Ordering::Relaxed), s + 1);
        assert!(gaps.is_empty(), "unexpected gaps: {}", gaps.len());
        assert_eq!(events.len(), 1);
    }

    #[tokio::test]
    async fn dead_slot_keeps_its_row() {
        let (path, url) = temp_db("dead");
        let mut p = pipeline(&url, Some(100)).await;
        p.handle(slot_msg(101, Some(100), SlotCommitment::Dead))
            .await
            .expect("dead");
        let slots = p.store.slots_between(100, 101).await.expect("slots");
        let _ = std::fs::remove_file(&path);
        assert_eq!(slots, vec![101]);
    }

    #[tokio::test]
    async fn far_slots_are_skipped_until_they_persist() {
        let (path, url) = temp_db("jump");
        let s = 1_000;
        let far = s + MAX_SLOT_JUMP + 10;
        let mut p = pipeline(&url, Some(s)).await;
        p.handle(transfer_at(far)).await.expect("events");
        p.handle(slot_msg(far, Some(far - 1), SlotCommitment::Finalized))
            .await
            .expect("finalized");
        p.handle(slot_msg(far, Some(far - 1), SlotCommitment::Confirmed))
            .await
            .expect("confirmed");
        let cursor = p.store.load_cursor().await.expect("cursor").expect("some");
        let events = p.store.recent_events(10).await.expect("events");
        assert_eq!(cursor.slot, s);
        assert_eq!(cursor.finalized_slot, 0);
        assert_eq!(p.last_slot, Some(s));
        assert!(events.is_empty());

        // Far updates of any status stay out of the store and the parent cache.
        p.handle(slot_msg(far + 1, Some(far), SlotCommitment::Processed))
            .await
            .expect("processed");
        assert!(p.slot_parents.is_empty());
        assert!(p.store.slots_between(s, far + 1).await.expect("slots").is_empty());

        // A plausible slot resets the run; a long run of far slots is accepted.
        p.handle(slot_msg(s + 1, Some(s), SlotCommitment::Confirmed))
            .await
            .expect("near");
        let last_far = far + u64::from(JUMP_ACCEPT_STREAK) - 1;
        for slot in far..=last_far {
            assert_eq!(p.last_slot, Some(s + 1));
            // Events come before their confirmed status, so the guard drops them.
            p.handle(transfer_at(slot)).await.expect("events");
            p.handle(slot_msg(slot, Some(slot - 1), SlotCommitment::Confirmed))
                .await
                .expect("far");
        }
        let gaps = p.store.list_gaps(false).await.expect("gaps");
        let _ = std::fs::remove_file(&path);
        assert_eq!(p.last_slot, Some(last_far));
        assert_eq!(gaps.len(), 1);
        assert_eq!((gaps[0].from_slot, gaps[0].to_slot), (s + 2, last_far));
    }

    #[test]
    fn tui_drops_stdout_sinks_only() {
        let sink = |kind: &str| SinkConfig {
            kind: kind.into(),
            url: Some("http://hook".into()),
            types: None,
            programs: None,
            instructions: None,
        };
        let sinks = vec![sink("stdout"), sink("webhook")];
        assert_eq!(sinks_for_mode(sinks.clone(), false).len(), 2);
        let kept = sinks_for_mode(sinks, true);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].kind, "webhook");
    }
}
