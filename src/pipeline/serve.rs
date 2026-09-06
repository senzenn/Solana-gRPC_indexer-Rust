use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::middleware;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::Json;
use axum::Router;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, RwLock};
use tracing::info;

use super::auth::{require_api_key, ApiKeyState};
use super::event::IndexEvent;
use super::metrics as prom;
use super::store::{EventFilter, EventStore};

#[derive(Clone)]
pub struct AppState {
    pub events: broadcast::Sender<IndexEvent>,
    pub stats: Arc<RwLock<LiveStats>>,
    pub store: Arc<EventStore>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct LiveStats {
    pub slot: u64,
    pub events_total: u64,
    pub token_transfers: u64,
    pub program_ix: u64,
    pub open_gaps: i64,
    pub lag_slots: u64,
}

const LIVE_HTML: &str = include_str!("../../public/live.html");

pub async fn serve(bind: SocketAddr, state: AppState, auth: Arc<ApiKeyState>) -> Result<()> {
    let protected = Router::new()
        .route("/stream", get(ws_handler))
        .route("/events", get(events))
        .route("/cursor", get(cursor))
        .route("/gaps", get(gaps))
        .route("/stats", get(stats))
        .layer(middleware::from_fn_with_state(auth.clone(), require_api_key))
        .with_state(state.clone());

    prom::init();

    let app = Router::new()
        .route("/", get(home))
        .route("/health", get(health))
        .route("/metrics", get(prometheus_metrics))
        .merge(protected)
        .with_state(state);

    info!(%bind, "http/ws listening");
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn home() -> Html<&'static str> {
    Html(LIVE_HTML)
}

async fn prometheus_metrics() -> impl IntoResponse {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        prom::render(),
    )
}

async fn health(State(state): State<AppState>) -> impl IntoResponse {
    let stats = state.stats.read().await;
    Json(serde_json::json!({
        "ok": true,
        "slot": stats.slot,
        "events_total": stats.events_total,
        "open_gaps": stats.open_gaps,
    }))
}

async fn stats(State(state): State<AppState>) -> impl IntoResponse {
    let stats = state.stats.read().await.clone();
    Json(stats)
}

#[derive(Debug, Deserialize)]
struct EventsQuery {
    limit: Option<i64>,
    offset: Option<i64>,
    since_slot: Option<u64>,
    until_slot: Option<u64>,
    #[serde(rename = "type")]
    kind: Option<String>,
    program: Option<String>,
    signature: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GapsQuery {
    open_only: Option<bool>,
}

async fn events(State(state): State<AppState>, Query(params): Query<EventsQuery>) -> Response {
    let filter = EventFilter {
        limit: params.limit.unwrap_or(100),
        offset: params.offset.unwrap_or(0),
        since_slot: params.since_slot,
        until_slot: params.until_slot,
        kind: params.kind,
        program: params.program,
        signature: params.signature,
    };
    match state.store.query_events(filter).await {
        Ok(events) => Json(events).into_response(),
        Err(err) => store_error(err),
    }
}

async fn cursor(State(state): State<AppState>) -> Response {
    match state.store.cursor_info().await {
        Ok(info) => Json(info).into_response(),
        Err(err) => store_error(err),
    }
}

async fn gaps(State(state): State<AppState>, Query(params): Query<GapsQuery>) -> Response {
    let open_only = params.open_only.unwrap_or(true);
    match state.store.list_gaps(open_only).await {
        Ok(gaps) => Json(gaps).into_response(),
        Err(err) => store_error(err),
    }
}

fn store_error(err: anyhow::Error) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "error": err.to_string() })),
    )
        .into_response()
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| client(socket, state))
}

async fn client(mut socket: WebSocket, state: AppState) {
    if let Ok(recent) = state.store.recent_events(50).await {
        for event in recent {
            if let Ok(json) = serde_json::to_string(&event)
                && socket.send(Message::Text(json.into())).await.is_err() {
                    return;
                }
        }
    }

    let mut rx = state.events.subscribe();
    loop {
        match rx.recv().await {
            Ok(event) => {
                if let Ok(json) = serde_json::to_string(&event)
                    && socket.send(Message::Text(json.into())).await.is_err() {
                        return;
                    }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}
