use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::{broadcast, mpsc, RwLock};
use tracing::{info, warn};

use super::auth::ApiKeyState;
use super::config_file::SinkConfig;
use super::event::IndexEvent;
use super::idl::IdlRouter;
use super::ingest::{decode_instructions, run_ingest, IngestConfig, IngestMsg};
use super::serve::{serve, AppState, LiveStats};
use super::sink::{build_sinks, SinkFanout};
use super::metrics as prom;
use super::store::{detect_gap, EventStore};

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
}

pub async fn run(args: RunArgs) -> Result<()> {
    let store = Arc::new(EventStore::open(&args.db).await?);
    let cursor = store.load_cursor().await?;
    let from_slot = cursor.as_ref().map(|c| c.slot.saturating_add(1));
    info!(from_slot = ?from_slot, "loaded cursor");

    let router = IdlRouter::from_dir(&args.idl_dir)?;
    let extra_programs = router.program_ids();
    info!(programs = extra_programs.len(), "loaded IDL parsers");

    let sinks = Arc::new(
        build_sinks(&args.sinks).unwrap_or_else(|_| SinkFanout::new(Vec::new())),
    );
    let api_state = Arc::new(ApiKeyState::from_keys(args.api_keys));

    let (ingest_tx, mut ingest_rx) = mpsc::channel::<IngestMsg>(4096);
    let (gap_tx, gap_rx) = mpsc::channel::<(u64, u64)>(64);
    let (bus, _) = broadcast::channel::<IndexEvent>(1024);
    let stats = Arc::new(RwLock::new(LiveStats::default()));
    prom::init();

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
    };
    let ingest_slots = last_slot_atomic.clone();
    tokio::spawn(async move {
        if let Err(err) = run_ingest(ingest_cfg, ingest_tx, ingest_slots).await {
            warn!(error = %err, "ingest ended");
        }
    });

    if let Some(rpc_url) = args.rpc_url.clone() {
        let worker = super::backfill::BackfillWorker::new(
            rpc_url,
            store.clone(),
            router.clone(),
            track_tokens,
        );
        tokio::spawn(async move {
            if let Err(err) = worker.run(gap_rx).await {
                warn!(error = %err, "backfill ended");
            }
        });
    }

    let http_state = AppState {
        events: bus.clone(),
        stats: stats.clone(),
        store: store.clone(),
    };
    let bind = args.ws_bind;
    let keys = api_state.clone();
    tokio::spawn(async move {
        if let Err(err) = serve(bind, http_state, keys).await {
            warn!(error = %err, "http server ended");
        }
    });

    if let Some(grpc_bind) = args.grpc_bind {
        let export = super::grpc_export::ExportService {
            events: bus.clone(),
            store: store.clone(),
        };
        tokio::spawn(async move {
            if let Err(err) = super::grpc_export::serve(grpc_bind, export).await {
                warn!(error = %err, "grpc export ended");
            }
        });
        info!(%grpc_bind, "gRPC export listening");
    }

    if args.tui {
        let rx = bus.subscribe();
        let stats_tui = stats.clone();
        tokio::spawn(async move {
            if let Err(err) = super::tui::run_tui(rx, stats_tui).await {
                warn!(error = %err, "tui ended");
            }
        });
    } else {
        info!(bind = %args.ws_bind, "open http://{}/ for the live view", args.ws_bind);
    }

    let mut last_slot: Option<u64> = cursor.map(|c| c.slot);

    loop {
        let Some(msg) = ingest_rx.recv().await else {
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        };

        match msg {
            IngestMsg::Reconnect => {
                prom::record_reconnect();
            }
            IngestMsg::GeyserError => {
                prom::record_geyser_error();
            }
            IngestMsg::Slot { slot, parent: _ } => {
                if let Some((from, to)) = detect_gap(last_slot, slot) {
                    emit_gap(&store, &bus, &gap_tx, &stats, from, to).await?;
                }
                last_slot = Some(slot.max(last_slot.unwrap_or(0)));
                if let Some(s) = last_slot {
                    last_slot_atomic.store(s, Ordering::Relaxed);
                    store.commit_cursor(s).await?;
                }
                {
                    let mut st = stats.write().await;
                    st.slot = last_slot.unwrap_or(slot);
                    st.open_gaps = store.open_gap_count().await.unwrap_or(st.open_gaps);
                    prom::update_live_stats(st.slot, st.lag_slots, st.open_gaps);
                }
            }
            IngestMsg::Instructions(ixs) => {
                let events = decode_instructions(&router, &ixs, track_tokens);
                for event in events {
                    if let Some(slot) = event.slot() {
                        if let Some((from, to)) = detect_gap(last_slot, slot) {
                            emit_gap(&store, &bus, &gap_tx, &stats, from, to).await?;
                        }
                        last_slot = Some(slot.max(last_slot.unwrap_or(0)));
                        last_slot_atomic.store(last_slot.unwrap_or(slot), Ordering::Relaxed);
                    }
                    let inserted = store.write_event(&event).await?;
                    if !inserted {
                        continue;
                    }
                    {
                        let mut s = stats.write().await;
                        s.events_total += 1;
                        s.slot = last_slot.unwrap_or(s.slot);
                        match &event {
                            IndexEvent::TokenTransfer { .. } => s.token_transfers += 1,
                            IndexEvent::ProgramIx { .. } => s.program_ix += 1,
                            _ => {}
                        }
                        s.open_gaps = store.open_gap_count().await.unwrap_or(s.open_gaps);
                        prom::record_event_stored();
                        prom::update_live_stats(s.slot, s.lag_slots, s.open_gaps);
                    }
                    let _ = bus.send(event.clone());
                    let _ = sinks.send(&event).await;
                }
            }
        }
    }
}

async fn emit_gap(
    store: &EventStore,
    bus: &broadcast::Sender<IndexEvent>,
    gap_tx: &mpsc::Sender<(u64, u64)>,
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
    let _ = gap_tx.send((from, to)).await;
    {
        let mut st = stats.write().await;
        st.open_gaps = store.open_gap_count().await.unwrap_or(st.open_gaps);
        prom::update_live_stats(st.slot, st.lag_slots, st.open_gaps);
    }
    warn!(from, to, "slot gap");
    Ok(())
}
