use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::{broadcast, RwLock};
use tracing::info;

use super::auth::ApiKeyState;
use super::metrics as prom;
use super::serve::{serve, AppState, LiveStats};
use super::store::EventStore;

/// Serve the live UI + REST/WS from an existing SQLite/Postgres DB without geyser ingest.
pub async fn run_demo(db: String, bind: SocketAddr, api_keys: Vec<String>) -> Result<()> {
    let store = Arc::new(EventStore::open(&db).await?);
    let cursor = store.load_cursor().await?;
    let (bus, _) = broadcast::channel::<super::event::IndexEvent>(256);
    let stats = Arc::new(RwLock::new(LiveStats {
        slot: cursor.as_ref().map(|c| c.slot).unwrap_or(0),
        ..Default::default()
    }));

    let open_gaps = store.open_gap_count().await.unwrap_or(0);
    if let Ok(recent) = store.recent_events(200).await {
        let mut st = stats.write().await;
        st.events_total = recent.len() as u64;
        st.open_gaps = open_gaps;
        for e in &recent {
            match e {
                super::event::IndexEvent::TokenTransfer { .. } => st.token_transfers += 1,
                super::event::IndexEvent::ProgramIx { .. } => st.program_ix += 1,
                _ => {}
            }
        }
        prom::update_live_stats(st.slot, st.lag_slots, st.open_gaps);
    } else {
        let mut st = stats.write().await;
        st.open_gaps = open_gaps;
        prom::update_live_stats(st.slot, st.lag_slots, st.open_gaps);
    }

    let state = AppState {
        events: bus,
        stats,
        store,
    };
    let auth = Arc::new(ApiKeyState::from_keys(api_keys));

    info!(
        %bind,
        "demo mode: serving stored events only (no geyser). open http://{bind}/"
    );
    serve(bind, state, auth).await
}
