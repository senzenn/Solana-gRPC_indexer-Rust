use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use futures::Stream;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{broadcast, mpsc, Semaphore};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::{error, info, warn};

use super::auth::{extract_api_key, ApiKeyState};
use super::event::IndexEvent;
use super::metrics as prom;
use super::serve::slot_fits_i64;
use super::store::{EventFilter, EventStore, StoreBusy, MAX_EVENT_OFFSET};

pub mod proto {
    tonic::include_proto!("index");
}

use proto::index_export_server::{IndexExport, IndexExportServer};
use proto::{GetEventsRequest, GetEventsResponse, IndexEventMsg, StreamEventsRequest};

const GRPC_CONCURRENCY_PER_CONNECTION: usize = 64;
const GRPC_MAX_STREAMS_PER_CONNECTION: u32 = 64;
const GRPC_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const GRPC_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);
const MAX_GRPC_STREAMS: usize = 256;
const STREAM_BUFFER: usize = 256;
#[cfg(not(test))]
const STREAM_SEND_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(test)]
const STREAM_SEND_TIMEOUT: Duration = Duration::from_millis(200);

/// Process-wide cap on StreamEvents subscribers, across all connections.
static STREAM_SLOTS: Semaphore = Semaphore::const_new(MAX_GRPC_STREAMS);

type EventStream = Pin<Box<dyn Stream<Item = Result<IndexEventMsg, Status>> + Send + 'static>>;

pub struct ExportService {
    pub events: broadcast::Sender<IndexEvent>,
    pub store: Arc<EventStore>,
}

impl ExportService {
    fn open_stream(
        &self,
        req: StreamEventsRequest,
        slots: &'static Semaphore,
    ) -> Result<Response<EventStream>, Status> {
        let permit = slots
            .try_acquire()
            .map_err(|_| Status::resource_exhausted("too many StreamEvents clients"))?;
        let kind_filter = if req.kind.is_empty() {
            None
        } else {
            Some(req.kind)
        };
        let program_filter = if req.program.is_empty() {
            None
        } else {
            Some(req.program)
        };

        let mut rx = self.events.subscribe();
        let (tx, out) = mpsc::channel(STREAM_BUFFER);
        // The task owns the permit. hyper stops polling a client that does not read, so the
        // send timeout, not the response body, must free the slot.
        tokio::spawn(async move {
            let _slot = permit;
            loop {
                let item = tokio::select! {
                    _ = tx.closed() => return,
                    msg = rx.recv() => match msg {
                        Ok(event) => {
                            if kind_filter.as_ref().is_some_and(|k| k != event.kind())
                                || program_filter.as_ref().is_some_and(|p| p != event.program())
                            {
                                continue;
                            }
                            Ok(to_msg(&event))
                        }
                        // The status ends the stream, so the client knows to resync with GetEvents.
                        Err(RecvError::Lagged(skipped)) => {
                            prom::record_stream_lagged(skipped);
                            warn!(skipped, "grpc stream client lagged");
                            Err(Status::data_loss(format!(
                                "stream lagged, {skipped} events skipped; resync with GetEvents"
                            )))
                        }
                        Err(RecvError::Closed) => return,
                    },
                };
                let last = item.is_err();
                if tx.send_timeout(item, STREAM_SEND_TIMEOUT).await.is_err() || last {
                    return;
                }
            }
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(out))))
    }
}

#[tonic::async_trait]
impl IndexExport for ExportService {
    type StreamEventsStream = EventStream;

    async fn stream_events(
        &self,
        request: Request<StreamEventsRequest>,
    ) -> Result<Response<Self::StreamEventsStream>, Status> {
        self.open_stream(request.into_inner(), &STREAM_SLOTS)
    }

    async fn get_events(
        &self,
        request: Request<GetEventsRequest>,
    ) -> Result<Response<GetEventsResponse>, Status> {
        let req = request.into_inner();
        let limit = if req.limit == 0 { 100 } else { req.limit.min(500) };
        let offset = i64::try_from(req.offset)
            .ok()
            .filter(|o| *o <= MAX_EVENT_OFFSET)
            .ok_or_else(|| {
                Status::invalid_argument(format!("offset must not exceed {MAX_EVENT_OFFSET}"))
            })?;
        let since_slot = (req.since_slot != 0).then_some(req.since_slot);
        let until_slot = (req.until_slot != 0).then_some(req.until_slot);
        if !slot_fits_i64(since_slot) || !slot_fits_i64(until_slot) {
            return Err(Status::invalid_argument(format!(
                "since_slot and until_slot must not exceed {}",
                i64::MAX
            )));
        }
        let filter = EventFilter {
            limit: limit as i64,
            offset,
            since_slot,
            until_slot,
            kind: if req.kind.is_empty() {
                None
            } else {
                Some(req.kind)
            },
            program: if req.program.is_empty() {
                None
            } else {
                Some(req.program)
            },
            signature: if req.signature.is_empty() {
                None
            } else {
                Some(req.signature)
            },
        };
        let events = self.store.query_events(filter).await.map_err(store_status)?;
        Ok(Response::new(GetEventsResponse {
            events: events.iter().map(to_msg).collect(),
        }))
    }
}

fn store_status(err: anyhow::Error) -> Status {
    if err.downcast_ref::<StoreBusy>().is_some() {
        return Status::unavailable("busy");
    }
    error!(error = %format_args!("{err:#}"), "grpc get_events failed");
    Status::internal("internal error")
}

fn to_msg(event: &IndexEvent) -> IndexEventMsg {
    IndexEventMsg {
        slot: event.slot().unwrap_or(0),
        kind: event.kind().to_string(),
        json: serde_json::to_string(event).unwrap_or_default(),
    }
}

/// Same keys and headers as the HTTP API: `x-api-key` or `authorization: Bearer`.
fn check_api_key(auth: &ApiKeyState, request: Request<()>) -> Result<Request<()>, Status> {
    if auth.allows(extract_api_key(request.metadata().as_ref())) {
        Ok(request)
    } else {
        Err(Status::unauthenticated("missing or invalid api key"))
    }
}

pub async fn serve(bind: SocketAddr, service: ExportService, auth: Arc<ApiKeyState>) -> Result<()> {
    info!(%bind, "gRPC export listening");
    let check = move |request: Request<()>| check_api_key(&auth, request);
    // The timeout ends only the wait for response headers, so StreamEvents streams stay open.
    tonic::transport::Server::builder()
        .concurrency_limit_per_connection(GRPC_CONCURRENCY_PER_CONNECTION)
        .max_concurrent_streams(GRPC_MAX_STREAMS_PER_CONNECTION)
        .http2_keepalive_interval(Some(GRPC_KEEPALIVE_INTERVAL))
        .timeout(GRPC_REQUEST_TIMEOUT)
        .add_service(IndexExportServer::with_interceptor(service, check))
        .serve(bind)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_stream::StreamExt;
    use tonic::metadata::MetadataValue;

    async fn service(name: &str, capacity: usize) -> ExportService {
        let path = std::env::temp_dir().join(format!(
            "index-grpc-{name}-{}-{}.db",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let store = EventStore::open(&format!("sqlite:{}", path.display()))
            .await
            .unwrap();
        let (events, _) = broadcast::channel(capacity);
        ExportService {
            events,
            store: Arc::new(store),
        }
    }

    fn transfer(slot: u64, signature: &str) -> IndexEvent {
        IndexEvent::TokenTransfer {
            slot,
            signature: signature.into(),
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

    #[test]
    fn interceptor_enforces_configured_keys() {
        let auth = ApiKeyState::from_keys(vec!["secret".into()]);
        assert_eq!(
            check_api_key(&auth, Request::new(())).unwrap_err().code(),
            tonic::Code::Unauthenticated
        );

        let mut req = Request::new(());
        req.metadata_mut()
            .insert("x-api-key", MetadataValue::from_static("wrong"));
        assert!(check_api_key(&auth, req).is_err());

        let mut req = Request::new(());
        req.metadata_mut()
            .insert("x-api-key", MetadataValue::from_static("secret"));
        assert!(check_api_key(&auth, req).is_ok());

        let mut req = Request::new(());
        req.metadata_mut()
            .insert("authorization", MetadataValue::from_static("Bearer secret"));
        assert!(check_api_key(&auth, req).is_ok());

        let open = ApiKeyState::from_keys(vec![]);
        assert!(check_api_key(&open, Request::new(())).is_ok());
    }

    #[tokio::test]
    async fn stream_events_reports_lag() {
        let svc = service("lag", 2).await;
        let mut stream = svc
            .stream_events(Request::new(StreamEventsRequest::default()))
            .await
            .unwrap()
            .into_inner();
        for slot in 1..=5 {
            svc.events.send(transfer(slot, &format!("s{slot}"))).unwrap();
        }
        let first = stream.next().await.unwrap();
        assert_eq!(first.unwrap_err().code(), tonic::Code::DataLoss);
    }

    #[tokio::test]
    async fn get_events_pages_within_one_slot() {
        let svc = service("page", 4).await;
        for i in 0..3 {
            svc.store
                .write_event(&transfer(7, &format!("sig{i}")))
                .await
                .unwrap();
        }
        let page = |offset| GetEventsRequest {
            limit: 2,
            since_slot: 7,
            offset,
            ..Default::default()
        };
        let first = svc.get_events(Request::new(page(0))).await.unwrap().into_inner();
        let second = svc.get_events(Request::new(page(2))).await.unwrap().into_inner();
        assert_eq!(first.events.len(), 2);
        assert_eq!(second.events.len(), 1);
        assert!(second.events[0].json.contains("sig2"));

        let too_deep = svc
            .get_events(Request::new(page(MAX_EVENT_OFFSET as u64 + 1)))
            .await
            .unwrap_err();
        assert_eq!(too_deep.code(), tonic::Code::InvalidArgument);
    }

    #[test]
    fn store_busy_maps_to_unavailable() {
        assert_eq!(store_status(anyhow::Error::new(StoreBusy)).code(), tonic::Code::Unavailable);
        let other = store_status(anyhow::anyhow!("no such table: secret_tbl"));
        assert_eq!(other.code(), tonic::Code::Internal);
        assert!(!other.message().contains("secret_tbl"));
    }

    #[tokio::test]
    async fn stream_slot_frees_when_stream_drops() {
        static SLOTS: Semaphore = Semaphore::const_new(1);
        let svc = service("slots", 4).await;

        let first = svc.open_stream(StreamEventsRequest::default(), &SLOTS);
        assert!(first.is_ok());
        match svc.open_stream(StreamEventsRequest::default(), &SLOTS) {
            Err(status) => assert_eq!(status.code(), tonic::Code::ResourceExhausted),
            Ok(_) => panic!("second stream must be refused"),
        }
        drop(first);
        // The forwarding task sees the closed channel and drops the permit.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(svc.open_stream(StreamEventsRequest::default(), &SLOTS).is_ok());
    }

    #[tokio::test]
    async fn stalled_stream_frees_slot_after_send_timeout() {
        static SLOTS: Semaphore = Semaphore::const_new(1);
        let svc = service("stall", 1024).await;

        // The client holds the stream but never reads it.
        let stalled = svc.open_stream(StreamEventsRequest::default(), &SLOTS);
        assert!(stalled.is_ok());
        for slot in 0..(STREAM_BUFFER as u64 + 10) {
            svc.events.send(transfer(slot, &format!("s{slot}"))).unwrap();
        }
        tokio::time::sleep(STREAM_SEND_TIMEOUT * 3).await;
        assert!(svc.open_stream(StreamEventsRequest::default(), &SLOTS).is_ok());
        drop(stalled);
    }
}
