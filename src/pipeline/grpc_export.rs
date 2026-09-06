use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result;
use futures::Stream;
use tokio::sync::broadcast;
use tokio_stream::{wrappers::BroadcastStream, StreamExt};
use tonic::{Request, Response, Status};
use tracing::info;

use super::event::IndexEvent;
use super::store::{EventFilter, EventStore};

pub mod proto {
    tonic::include_proto!("index");
}

use proto::index_export_server::{IndexExport, IndexExportServer};
use proto::{GetEventsRequest, GetEventsResponse, IndexEventMsg, StreamEventsRequest};

pub struct ExportService {
    pub events: broadcast::Sender<IndexEvent>,
    pub store: Arc<EventStore>,
}

#[tonic::async_trait]
impl IndexExport for ExportService {
    type StreamEventsStream =
        Pin<Box<dyn Stream<Item = Result<IndexEventMsg, Status>> + Send + 'static>>;

    async fn stream_events(
        &self,
        request: Request<StreamEventsRequest>,
    ) -> Result<Response<Self::StreamEventsStream>, Status> {
        let req = request.into_inner();
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

        let rx = self.events.subscribe();
        let stream = BroadcastStream::new(rx).filter_map(move |msg| {
            let event = msg.ok()?;
            if let Some(ref k) = kind_filter
                && k != event.kind() {
                    return None;
                }
            if let Some(ref p) = program_filter
                && p != event.program() {
                    return None;
                }
            Some(Ok(to_msg(&event)))
        });

        Ok(Response::new(Box::pin(stream)))
    }

    async fn get_events(
        &self,
        request: Request<GetEventsRequest>,
    ) -> Result<Response<GetEventsResponse>, Status> {
        let req = request.into_inner();
        let limit = if req.limit == 0 { 100 } else { req.limit.min(500) };
        let filter = EventFilter {
            limit: limit as i64,
            since_slot: if req.since_slot == 0 {
                None
            } else {
                Some(req.since_slot)
            },
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
            ..Default::default()
        };
        let events = self
            .store
            .query_events(filter)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        Ok(Response::new(GetEventsResponse {
            events: events.iter().map(to_msg).collect(),
        }))
    }
}

fn to_msg(event: &IndexEvent) -> IndexEventMsg {
    IndexEventMsg {
        slot: event.slot().unwrap_or(0),
        kind: event.kind().to_string(),
        json: serde_json::to_string(event).unwrap_or_default(),
    }
}

pub async fn serve(bind: SocketAddr, service: ExportService) -> Result<()> {
    info!(%bind, "gRPC export listening");
    tonic::transport::Server::builder()
        .add_service(IndexExportServer::new(service))
        .serve(bind)
        .await?;
    Ok(())
}
