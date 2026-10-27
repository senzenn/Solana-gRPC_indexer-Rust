//! Copies finalized events from the ch_outbox table to ClickHouse over its HTTP
//! interface. A row leaves the outbox only after ClickHouse accepts it.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use tracing::{info, warn};

use super::config_file::ClickHouseConfig;
use super::geyser::endpoint_label;
use super::store::{EventStore, OutboxRow};

const BATCH_ROWS: i64 = 1000;
const IDLE_WAIT: Duration = Duration::from_secs(1);
const RETRY_MIN: Duration = Duration::from_millis(500);
const RETRY_MAX: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub struct ClickHouseWorker {
    cfg: ClickHouseConfig,
    password: Option<String>,
    store: Arc<EventStore>,
    http: reqwest::Client,
}

#[derive(Serialize)]
struct ChRow<'a> {
    signature: &'a str,
    ix_path: &'a str,
    kind: &'a str,
    slot: u64,
    program: &'a str,
    payload: &'a str,
    version: u64,
}

impl ClickHouseWorker {
    pub fn new(
        cfg: ClickHouseConfig,
        password: Option<String>,
        store: Arc<EventStore>,
    ) -> Result<Self> {
        cfg.validate()?;
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()?;
        Ok(Self {
            cfg,
            password,
            store,
            http,
        })
    }

    /// Runs until the task stops. On an error the rows stay in the outbox and
    /// the next try waits longer, up to RETRY_MAX.
    pub async fn run(self) -> Result<()> {
        info!(url = %endpoint_label(&self.cfg.url), table = %self.cfg.table, "clickhouse sink on");
        let mut table_ready = false;
        let mut wait = RETRY_MIN;
        loop {
            let result = if table_ready {
                self.flush().await
            } else {
                self.create_table().await.map(|()| 0)
            };
            match result {
                Ok(sent) => {
                    table_ready = true;
                    wait = RETRY_MIN;
                    if sent == 0 {
                        tokio::time::sleep(IDLE_WAIT).await;
                    }
                }
                Err(err) => {
                    warn!(
                        error = %format!("{err:#}"),
                        retry_in = ?wait,
                        "clickhouse sink failed; rows stay in the outbox"
                    );
                    tokio::time::sleep(wait).await;
                    wait = (wait * 2).min(RETRY_MAX);
                }
            }
        }
    }

    /// ReplacingMergeTree keeps the row with the highest version per key, so a
    /// resent or updated event ends as one row after merges (or with FINAL).
    async fn create_table(&self) -> Result<()> {
        let ddl = format!(
            "CREATE TABLE IF NOT EXISTS {} (
                signature String,
                ix_path String,
                kind LowCardinality(String),
                slot UInt64,
                program LowCardinality(String),
                payload String,
                version UInt64
            ) ENGINE = ReplacingMergeTree(version)
            ORDER BY (kind, signature, ix_path)",
            self.cfg.table
        );
        self.post(&ddl, String::new()).await
    }

    /// Sends one batch. Returns how many outbox rows it removed.
    async fn flush(&self) -> Result<usize> {
        let finalized = self
            .store
            .load_cursor()
            .await?
            .map(|c| c.finalized_slot)
            .unwrap_or(0);
        let batch = self.store.outbox_batch(finalized, BATCH_ROWS).await?;
        if batch.is_empty() {
            return Ok(0);
        }
        let body = encode_rows(&batch)?;
        if !body.is_empty() {
            let insert = format!("INSERT INTO {} FORMAT JSONEachRow", self.cfg.table);
            self.post(&insert, body).await?;
        }
        let ids: Vec<i64> = batch.iter().map(|row| row.id).collect();
        self.store.outbox_delete(&ids).await?;
        Ok(ids.len())
    }

    async fn post(&self, query: &str, body: String) -> Result<()> {
        let mut request = self
            .http
            .post(&self.cfg.url)
            .query(&[("query", query), ("database", self.cfg.database.as_str())])
            .body(body);
        if let Some(user) = &self.cfg.user {
            request = request.header("X-ClickHouse-User", user);
        }
        if let Some(password) = &self.password {
            request = request.header("X-ClickHouse-Key", password);
        }
        // without_url: the URL can hold credentials.
        let response = request.send().await.map_err(|err| {
            anyhow!(
                "send to {}: {}",
                endpoint_label(&self.cfg.url),
                err.without_url()
            )
        })?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            let text: String = text.chars().take(300).collect();
            bail!("clickhouse returned {status}: {}", text.trim());
        }
        Ok(())
    }
}

/// One JSON object per line. Rows of deleted (fork) events are left out.
fn encode_rows(batch: &[OutboxRow]) -> Result<String> {
    let mut body = String::new();
    for row in batch {
        let Some(event) = &row.event else {
            continue;
        };
        let line = serde_json::to_string(&ChRow {
            signature: &event.signature,
            ix_path: &event.ix_path,
            kind: &event.kind,
            slot: event.slot,
            program: &event.program,
            payload: &event.payload,
            version: u64::try_from(row.id).context("negative outbox id")?,
        })?;
        body.push_str(&line);
        body.push('\n');
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU16, Ordering};

    use axum::Router;
    use axum::extract::{Query, State};
    use axum::http::StatusCode;
    use axum::routing::post;

    use crate::pipeline::testutil::{TempDb, transfer};

    #[derive(Default)]
    struct FakeClickHouse {
        status: AtomicU16,
        inserts: Mutex<Vec<String>>,
    }

    async fn fake_post(
        State(fake): State<Arc<FakeClickHouse>>,
        Query(params): Query<std::collections::HashMap<String, String>>,
        body: String,
    ) -> StatusCode {
        let status = StatusCode::from_u16(fake.status.load(Ordering::SeqCst))
            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        if status.is_success() && params.get("query").is_some_and(|q| q.starts_with("INSERT")) {
            fake.inserts.lock().unwrap().push(body);
        }
        status
    }

    async fn start_fake() -> (Arc<FakeClickHouse>, String) {
        let fake = Arc::new(FakeClickHouse::default());
        let app = Router::new()
            .route("/", post(fake_post))
            .with_state(fake.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });
        (fake, url)
    }

    #[tokio::test]
    async fn sends_finalized_rows_and_keeps_them_on_failure() {
        let db = TempDb::new();
        let mut store = EventStore::open(&db.url).await.unwrap();
        store.enable_outbox();
        let store = Arc::new(store);
        store.write_event(&transfer(10, "s10")).await.unwrap();
        store.write_event(&transfer(20, "s20")).await.unwrap();
        store.commit_cursors(15, Some(15)).await.unwrap();

        let (fake, url) = start_fake().await;
        let cfg = ClickHouseConfig {
            url,
            database: "default".into(),
            table: "index_events".into(),
            user: None,
        };
        let worker = ClickHouseWorker::new(cfg, None, store.clone()).unwrap();

        fake.status.store(500, Ordering::SeqCst);
        assert!(worker.flush().await.is_err());
        assert_eq!(
            store.outbox_batch(u64::MAX >> 1, 10).await.unwrap().len(),
            2
        );

        fake.status.store(200, Ordering::SeqCst);
        assert_eq!(
            worker.flush().await.unwrap(),
            1,
            "slot 20 is above the finalized slot"
        );
        let inserts = fake.inserts.lock().unwrap().clone();
        assert_eq!(inserts.len(), 1);
        let row: serde_json::Value = serde_json::from_str(inserts[0].trim()).unwrap();
        assert_eq!(row["signature"], "s10");
        assert_eq!(row["kind"], "token_transfer");
        assert_eq!(row["slot"], 10);

        // A replay of the same event adds no outbox row.
        store.write_event(&transfer(20, "s20")).await.unwrap();
        let left = store.outbox_batch(u64::MAX >> 1, 10).await.unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].event.as_ref().unwrap().slot, 20);
    }

    #[tokio::test]
    async fn new_rejects_a_table_name_that_is_not_an_identifier() {
        let store = Arc::new(EventStore::open("sqlite::memory:").await.unwrap());
        let cfg = ClickHouseConfig {
            url: "http://127.0.0.1:8123".into(),
            database: "default".into(),
            table: "x; DROP TABLE y".into(),
            user: None,
        };
        assert!(ClickHouseWorker::new(cfg, None, store).is_err());
    }
}
