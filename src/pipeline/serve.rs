use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json};
use axum::Router;
use hyper::server::conn::http1;
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::service::TowerToHyperService;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, RwLock, Semaphore};
use tracing::{error, info, warn};

use super::auth::{require_api_key, ApiKeyState, WS_KEY_PROTOCOL};
use super::event::IndexEvent;
use super::metrics as prom;
use super::store::{EventFilter, EventStore, StoreBusy, MAX_EVENT_OFFSET};

const MAX_HTTP_CONNECTIONS: usize = 512;
/// Also limits a silent new connection and an idle keep-alive connection.
#[cfg(not(test))]
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(test)]
const HEADER_READ_TIMEOUT: Duration = Duration::from_millis(300);
const MAX_WS_CLIENTS: usize = 256;
const WS_BACKLOG: i64 = 50;
const WS_SEND_TIMEOUT: Duration = Duration::from_secs(10);
/// Clients send only control frames, so a small cap is enough.
const WS_MAX_MESSAGE: usize = 4 << 10;
#[cfg(not(test))]
const WS_PING_INTERVAL: Duration = Duration::from_secs(30);
#[cfg(test)]
const WS_PING_INTERVAL: Duration = Duration::from_millis(300);

#[derive(Clone)]
pub struct AppState {
    pub events: broadcast::Sender<IndexEvent>,
    pub stats: Arc<RwLock<LiveStats>>,
    pub store: Arc<EventStore>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct LiveStats {
    pub slot: u64,
    pub finalized_slot: u64,
    pub events_total: u64,
    pub token_transfers: u64,
    pub program_ix: u64,
    pub open_gaps: i64,
    pub lag_slots: u64,
}

const LIVE_HTML: &str = include_str!("../../public/live.html");

pub async fn serve(bind: SocketAddr, state: AppState, auth: Arc<ApiKeyState>) -> Result<()> {
    prom::init();
    let app = router(state, auth);

    info!(%bind, "http/ws listening");
    let listener = TcpListener::bind(bind).await?;
    accept_loop(listener, app, MAX_HTTP_CONNECTIONS).await;
    Ok(())
}

/// HTTP/1 only: the auto (h1/h2c) version sniff has no timer, so silent sockets could fill the
/// cap. axum::serve has no header read timeout and no connection cap, and both apply before auth.
async fn accept_loop(listener: TcpListener, app: Router, max_connections: usize) {
    let slots = Arc::new(Semaphore::new(max_connections));
    loop {
        let stream = match listener.accept().await {
            Ok((stream, _)) => stream,
            Err(err) => {
                // For example EMFILE: wait a little so the loop does not spin.
                warn!(error = %err, "http accept failed");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        // When all slots are in use, dropping the stream closes the connection (no 503).
        let Ok(permit) = slots.clone().try_acquire_owned() else {
            continue;
        };
        let service = TowerToHyperService::new(app.clone());
        tokio::spawn(async move {
            let mut builder = http1::Builder::new();
            // hyper starts this timer on the first read and again each time the connection goes idle.
            builder
                .timer(TokioTimer::new())
                .header_read_timeout(HEADER_READ_TIMEOUT);
            let _ = builder
                .serve_connection(TokioIo::new(stream), service)
                .with_upgrades()
                .await;
            drop(permit);
        });
    }
}

fn router(state: AppState, auth: Arc<ApiKeyState>) -> Router {
    let open = auth.keys.is_empty();
    let api = Router::new()
        .route("/stream", get(ws_handler))
        .route("/events", get(events))
        .route("/cursor", get(cursor))
        .route("/gaps", get(gaps))
        .route("/stats", get(stats));

    let protected = Router::new()
        .merge(api.clone())
        .nest("/v1", api)
        .layer(middleware::from_fn_with_state(auth, require_api_key))
        .layer(Extension(Arc::new(Semaphore::new(MAX_WS_CLIENTS))))
        .with_state(state.clone());

    let app = Router::new()
        .route("/", get(home))
        .route("/health", get(health))
        .route("/v1/health", get(health))
        .route("/metrics", get(prometheus_metrics))
        .merge(protected)
        .with_state(state);
    // Any bind: a container on 0.0.0.0 can still be published on host loopback.
    if open {
        app.layer(middleware::from_fn(rebinding_guard))
    } else {
        app
    }
}

/// DNS rebinding: a page can point its own DNS name at a local address and then read an
/// open API. So an open server accepts only an IP literal or a localhost name as Host.
/// A request with no Host is refused too; an absolute URI can carry the authority.
async fn rebinding_guard(request: axum::extract::Request, next: Next) -> Response {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .or_else(|| request.uri().authority().map(|a| a.as_str()));
    if !host.is_some_and(is_ip_or_localhost) {
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(request).await
}

/// A rebinding page always has a DNS name, never an IP literal. `*.localhost` cannot be
/// registered, so it resolves only to loopback.
fn is_ip_or_localhost(host: &str) -> bool {
    let Ok(authority) = host.parse::<axum::http::uri::Authority>() else {
        return false;
    };
    let name = authority.host().trim_end_matches('.').to_ascii_lowercase();
    name == "localhost"
        || name.ends_with(".localhost")
        || name
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .is_ok()
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
        "finalized_slot": stats.finalized_slot,
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

/// The store binds slots as i64, so a larger u64 would wrap to a negative value.
pub(crate) fn slot_fits_i64(slot: Option<u64>) -> bool {
    slot.is_none_or(|s| i64::try_from(s).is_ok())
}

fn events_filter(params: EventsQuery) -> Result<EventFilter, String> {
    let offset = params.offset.unwrap_or(0);
    // The store rejects a deeper offset; answer 400 here, not 500.
    if offset > MAX_EVENT_OFFSET {
        return Err(format!("offset must not exceed {MAX_EVENT_OFFSET}"));
    }
    if !slot_fits_i64(params.since_slot) || !slot_fits_i64(params.until_slot) {
        return Err(format!("since_slot and until_slot must not exceed {}", i64::MAX));
    }
    Ok(EventFilter {
        limit: params.limit.unwrap_or(100),
        offset,
        since_slot: params.since_slot,
        until_slot: params.until_slot,
        kind: params.kind,
        program: params.program,
        signature: params.signature,
    })
}

async fn events(State(state): State<AppState>, Query(params): Query<EventsQuery>) -> Response {
    let filter = match events_filter(params) {
        Ok(filter) => filter,
        Err(msg) => {
            return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": msg })))
                .into_response();
        }
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

/// DB errors can name tables or hold row data, so clients get a generic message.
fn store_error(err: anyhow::Error) -> Response {
    if err.downcast_ref::<StoreBusy>().is_some() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "busy" })),
        )
            .into_response();
    }
    error!(error = %format_args!("{err:#}"), "store query failed");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "error": "internal error" })),
    )
        .into_response()
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    Extension(slots): Extension<Arc<Semaphore>>,
    State(state): State<AppState>,
) -> Response {
    if !same_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok(permit) = slots.try_acquire_owned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    // Browsers drop the socket unless the 101 echoes one offered subprotocol; never echo the key.
    ws.protocols([WS_KEY_PROTOCOL])
        .max_message_size(WS_MAX_MESSAGE)
        .max_frame_size(WS_MAX_MESSAGE)
        .on_upgrade(move |socket| async move {
            client(socket, state).await;
            drop(permit);
        })
}

/// CORS does not cover WebSocket upgrades, so a cross-site page could read the stream.
/// Clients that send no Origin are not browsers and pass.
fn same_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    let Some((_, origin_host)) = origin.to_str().ok().and_then(|o| o.split_once("://")) else {
        return false;
    };
    // A reverse proxy can rewrite Host; browsers cannot set X-Forwarded-Host.
    let forwarded = headers
        .get("x-forwarded-host")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next());
    [headers.get(header::HOST).and_then(|v| v.to_str().ok()), forwarded]
        .into_iter()
        .flatten()
        .any(|host| host.trim().eq_ignore_ascii_case(origin_host))
}

fn lagged_notice(skipped: u64) -> String {
    serde_json::json!({ "type": "lagged", "skipped": skipped }).to_string()
}

async fn client(mut socket: WebSocket, state: AppState) {
    // Subscribe before the backlog query, so no event falls between the two.
    let mut rx = state.events.subscribe();
    let mut backlog = HashSet::new();
    if let Ok(mut recent) = state.store.recent_events(WS_BACKLOG).await {
        recent.reverse();
        for event in recent {
            let Ok(json) = serde_json::to_string(&event) else {
                continue;
            };
            backlog.insert(json.clone());
            if !send_frame(&mut socket, Message::Text(json.into())).await {
                return;
            }
        }
    }

    let start = tokio::time::Instant::now() + WS_PING_INTERVAL;
    let mut ping = tokio::time::interval_at(start, WS_PING_INTERVAL);
    let mut alive = true;
    loop {
        tokio::select! {
            msg = rx.recv() => {
                let text = match msg {
                    Ok(event) => match serde_json::to_string(&event) {
                        // Events stored after subscribe are in the backlog and the live feed.
                        Ok(json) if backlog.remove(&json) => continue,
                        Ok(json) => json,
                        Err(_) => continue,
                    },
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        prom::record_stream_lagged(skipped);
                        warn!(skipped, "websocket client lagged");
                        lagged_notice(skipped)
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                };
                if !send_frame(&mut socket, Message::Text(text.into())).await {
                    return;
                }
            }
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return,
                Some(Ok(_)) => alive = true,
            },
            _ = ping.tick() => {
                // No frame, not even a Pong, for a full interval: the peer is gone.
                if !alive {
                    return;
                }
                alive = false;
                if !send_frame(&mut socket, Message::Ping(Default::default())).await {
                    return;
                }
            }
        }
    }
}

/// A stalled reader must not hold its connection slot forever.
async fn send_frame(socket: &mut WebSocket, frame: Message) -> bool {
    matches!(
        tokio::time::timeout(WS_SEND_TIMEOUT, socket.send(frame)).await,
        Ok(Ok(()))
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use futures::StreamExt;
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    async fn temp_store(name: &str) -> Arc<EventStore> {
        let path = std::env::temp_dir().join(format!(
            "index-serve-{name}-{}-{}.db",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let url = format!("sqlite:{}", path.display());
        Arc::new(EventStore::open(&url).await.unwrap())
    }

    async fn spawn_app(
        store: Arc<EventStore>,
        bus: broadcast::Sender<IndexEvent>,
        keys: Vec<String>,
    ) -> SocketAddr {
        let state = AppState {
            events: bus,
            stats: Arc::new(RwLock::new(LiveStats::default())),
            store,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = router(state, Arc::new(ApiKeyState::from_keys(keys)));
        tokio::spawn(accept_loop(listener, app, MAX_HTTP_CONNECTIONS));
        addr
    }

    async fn next_json(
        ws: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) -> serde_json::Value {
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            match msg {
                WsMessage::Text(text) => return serde_json::from_str(&text).unwrap(),
                WsMessage::Ping(_) => {}
                other => panic!("unexpected frame {other:?}"),
            }
        }
    }

    fn query(offset: Option<i64>, since_slot: Option<u64>) -> EventsQuery {
        EventsQuery {
            limit: None,
            offset,
            since_slot,
            until_slot: None,
            kind: None,
            program: None,
            signature: None,
        }
    }

    #[test]
    fn events_filter_rejects_deep_offset_and_wrapping_slot() {
        assert!(events_filter(query(Some(MAX_EVENT_OFFSET), None)).is_ok());
        assert!(events_filter(query(Some(MAX_EVENT_OFFSET + 1), None)).is_err());
        assert!(events_filter(query(None, Some(i64::MAX as u64))).is_ok());
        assert!(events_filter(query(None, Some(i64::MAX as u64 + 1))).is_err());
    }

    #[tokio::test]
    async fn store_error_hides_internal_text() {
        let resp = store_error(anyhow::anyhow!("no such table: events_secret"));
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(!body.contains("events_secret"), "{body}");

        let resp = store_error(anyhow::Error::new(StoreBusy));
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], br#"{"error":"busy"}"#);
    }

    #[test]
    fn same_origin_rejects_cross_site_upgrade() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("127.0.0.1:8080"));
        assert!(same_origin(&headers));

        headers.insert(header::ORIGIN, HeaderValue::from_static("http://127.0.0.1:8080"));
        assert!(same_origin(&headers));

        headers.insert(header::ORIGIN, HeaderValue::from_static("https://evil.example"));
        assert!(!same_origin(&headers));

        headers.insert(header::ORIGIN, HeaderValue::from_static("null"));
        assert!(!same_origin(&headers));

        headers.insert(header::ORIGIN, HeaderValue::from_static("https://index.example"));
        headers.insert("x-forwarded-host", HeaderValue::from_static("index.example"));
        assert!(same_origin(&headers));
    }

    #[tokio::test]
    async fn ws_sends_backlog_oldest_first() {
        let store = temp_store("backlog").await;
        for slot in 1..=3 {
            store
                .write_event(&IndexEvent::Slot { slot, parent: None })
                .await
                .unwrap();
        }
        let (bus, _) = broadcast::channel(16);
        let addr = spawn_app(store, bus, vec![]).await;

        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/stream"))
            .await
            .unwrap();
        let slots: Vec<u64> = [
            next_json(&mut ws).await,
            next_json(&mut ws).await,
            next_json(&mut ws).await,
        ]
        .iter()
        .map(|v| v["slot"].as_u64().unwrap())
        .collect();
        assert_eq!(slots, vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn ws_rejects_cross_site_origin() {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        use tokio_tungstenite::tungstenite::{http, Error};

        let store = temp_store("origin").await;
        let (bus, _) = broadcast::channel(4);
        let addr = spawn_app(store, bus, vec![]).await;

        let mut req = format!("ws://{addr}/stream").into_client_request().unwrap();
        req.headers_mut().insert(
            "origin",
            http::HeaderValue::from_static("https://evil.example"),
        );
        match tokio_tungstenite::connect_async(req).await {
            Err(Error::Http(resp)) => assert_eq!(resp.status().as_u16(), 403),
            other => panic!("expected 403, got {:?}", other.map(|(_, resp)| resp.status())),
        }
    }

    #[tokio::test]
    async fn ws_accepts_subprotocol_key_and_echoes_marker() {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        use tokio_tungstenite::tungstenite::http;

        let store = temp_store("wskey").await;
        let (bus, _) = broadcast::channel(4);
        let addr = spawn_app(store, bus, vec!["s3cret".into()]).await;

        let mut req = format!("ws://{addr}/v1/stream").into_client_request().unwrap();
        req.headers_mut().insert(
            "sec-websocket-protocol",
            http::HeaderValue::from_static("index-key, s3cret"),
        );
        let (_, resp) = tokio_tungstenite::connect_async(req).await.unwrap();
        assert_eq!(resp.status().as_u16(), 101);
        assert_eq!(resp.headers()["sec-websocket-protocol"], "index-key");
    }

    #[tokio::test]
    async fn subprotocol_key_only_on_stream_and_query_key_rejected() {
        use tokio_tungstenite::tungstenite::Error;

        let store = temp_store("wsreject").await;
        let (bus, _) = broadcast::channel(4);
        let addr = spawn_app(store, bus, vec!["s3cret".into()]).await;

        let status = reqwest::Client::new()
            .get(format!("http://{addr}/events"))
            .header("sec-websocket-protocol", "index-key, s3cret")
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status.as_u16(), 401);

        match tokio_tungstenite::connect_async(format!("ws://{addr}/stream?api_key=s3cret")).await {
            Err(Error::Http(resp)) => assert_eq!(resp.status().as_u16(), 401),
            other => panic!("expected 401, got {:?}", other.map(|(_, resp)| resp.status())),
        }
    }

    #[tokio::test]
    async fn ws_reports_lag_to_client() {
        let store = temp_store("lag").await;
        let (bus, _) = broadcast::channel(2);
        let addr = spawn_app(store, bus.clone(), vec![]).await;

        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/stream"))
            .await
            .unwrap();
        while bus.receiver_count() == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        // The current-thread runtime does not run the server task during these sends.
        for slot in 1..=5 {
            bus.send(IndexEvent::Slot { slot, parent: None }).unwrap();
        }

        let first = next_json(&mut ws).await;
        assert_eq!(first["type"], "lagged");
        assert_eq!(first["skipped"], 3);
        assert_eq!(next_json(&mut ws).await["slot"], 4);
    }

    #[tokio::test]
    async fn open_server_accepts_only_ip_or_localhost_host() {
        let store = temp_store("host").await;
        let (bus, _) = broadcast::channel(4);
        let addr = spawn_app(store.clone(), bus.clone(), vec![]).await;
        let client = reqwest::Client::new();
        let status = |host: &'static str| {
            let req = client.get(format!("http://{addr}/stats")).header("host", host);
            async move { req.send().await.unwrap().status().as_u16() }
        };
        // router() no longer depends on the bind, so this covers a 0.0.0.0 container bind too.
        assert_eq!(status("rebind.evil.example").await, 403);
        assert_eq!(status("evil.example").await, 403);
        assert_eq!(status("localhost:1234").await, 200);
        assert_eq!(status("app.localhost").await, 200);
        assert_eq!(status("[::1]").await, 200);
        assert_eq!(status("127.0.0.1:8080").await, 200);
        assert_eq!(status("10.0.0.5:8080").await, 200);

        // HTTP/1.0 allows no Host header; an absolute URI carries the authority, as in HTTP/2.
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let raw_status = |request: &'static str| async move {
            let mut raw = tokio::net::TcpStream::connect(addr).await.unwrap();
            raw.write_all(request.as_bytes()).await.unwrap();
            let mut reply = Vec::new();
            raw.read_to_end(&mut reply).await.unwrap();
            String::from_utf8_lossy(&reply).lines().next().unwrap_or_default().to_string()
        };
        assert_eq!(raw_status("GET /stats HTTP/1.0\r\n\r\n").await, "HTTP/1.0 403 Forbidden");
        assert_eq!(
            raw_status("GET http://127.0.0.1:9/stats HTTP/1.0\r\n\r\n").await,
            "HTTP/1.0 200 OK"
        );
        assert_eq!(
            raw_status("GET http://rebind.evil.example/stats HTTP/1.0\r\n\r\n").await,
            "HTTP/1.0 403 Forbidden"
        );

        // With keys set the key check covers rebinding, so Host is not checked.
        let keyed = spawn_app(store, bus, vec!["k".into()]).await;
        let status = client
            .get(format!("http://{keyed}/stats"))
            .header("host", "rebind.evil.example")
            .header("x-api-key", "k")
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status.as_u16(), 200);
    }

    #[tokio::test]
    async fn ws_closes_peer_that_never_answers_ping() {
        let store = temp_store("ping").await;
        let (bus, _) = broadcast::channel(4);
        let addr = spawn_app(store, bus, vec![]).await;
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/stream"))
            .await
            .unwrap();

        // The client sends Pong only while it reads, so it stays silent here.
        tokio::time::sleep(WS_PING_INTERVAL * 3).await;
        let closed = tokio::time::timeout(Duration::from_secs(2), async {
            let mut saw_ping = false;
            while let Some(frame) = ws.next().await {
                match frame {
                    Ok(WsMessage::Ping(_)) => saw_ping = true,
                    Ok(WsMessage::Close(_)) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            saw_ping
        })
        .await;
        assert!(matches!(closed, Ok(true)), "server kept the silent connection open");
    }

    #[tokio::test]
    async fn accept_loop_drops_slow_headers_and_extra_connections() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let store = temp_store("conn").await;
        let (bus, _) = broadcast::channel(4);
        let state = AppState {
            events: bus,
            stats: Arc::new(RwLock::new(LiveStats::default())),
            store,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = router(state, Arc::new(ApiKeyState::from_keys(vec![])));
        tokio::spawn(accept_loop(listener, app, 1));

        // Reads 0 bytes once the server closes the socket.
        async fn closed_within(stream: &mut tokio::net::TcpStream, wait: Duration) -> bool {
            let mut buf = [0u8; 64];
            matches!(tokio::time::timeout(wait, stream.read(&mut buf)).await, Ok(Ok(0) | Err(_)))
        }

        let mut slow = tokio::net::TcpStream::connect(addr).await.unwrap();
        slow.write_all(b"GET /health HTTP/1.1\r\n").await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut extra = tokio::net::TcpStream::connect(addr).await.unwrap();
        assert!(closed_within(&mut extra, Duration::from_millis(200)).await);
        assert!(closed_within(&mut slow, HEADER_READ_TIMEOUT * 4).await);
    }

    #[tokio::test]
    async fn ws_closes_on_oversized_frame() {
        use futures::SinkExt;

        let store = temp_store("big").await;
        let (bus, _) = broadcast::channel(4);
        let addr = spawn_app(store, bus, vec![]).await;
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/stream"))
            .await
            .unwrap();
        ws.send(WsMessage::Text("x".repeat(WS_MAX_MESSAGE + 1))).await.unwrap();

        let closed = tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(frame) = ws.next().await {
                if matches!(frame, Ok(WsMessage::Close(_)) | Err(_)) {
                    break;
                }
            }
        })
        .await;
        assert!(closed.is_ok(), "server kept the connection open");
    }

    #[tokio::test]
    async fn accept_loop_closes_silent_and_idle_connections() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let store = temp_store("idle").await;
        let (bus, _) = broadcast::channel(4);
        let addr = spawn_app(store, bus, vec![]).await;

        let started = tokio::time::Instant::now();
        let mut silent = tokio::net::TcpStream::connect(addr).await.unwrap();
        let mut buf = [0u8; 64];
        let read = tokio::time::timeout(HEADER_READ_TIMEOUT * 4, silent.read(&mut buf)).await;
        assert!(matches!(read, Ok(Ok(0) | Err(_))), "silent socket stayed open");
        assert!(started.elapsed() >= HEADER_READ_TIMEOUT / 2);

        let mut idle = tokio::net::TcpStream::connect(addr).await.unwrap();
        idle.write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .await
            .unwrap();
        let mut reply = Vec::new();
        let read = tokio::time::timeout(HEADER_READ_TIMEOUT * 4, idle.read_to_end(&mut reply)).await;
        assert!(read.is_ok(), "idle keep-alive socket stayed open");
        assert!(reply.starts_with(b"HTTP/1.1 200"), "{}", String::from_utf8_lossy(&reply));
    }
}
