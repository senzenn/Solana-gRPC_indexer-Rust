use anyhow::{Context, Result};
use chrono::Utc;
use serde::Serialize;
use sqlx::postgres::PgPoolOptions;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::{postgres::PgPool, QueryBuilder, Row, Sqlite, SqlitePool};

use super::event::IndexEvent;

enum DbBackend {
    Sqlite(SqlitePool),
    Postgres(PgPool),
}

#[derive(Debug, Clone)]
pub struct Cursor {
    pub slot: u64,
}

#[derive(Debug, Clone, Default)]
pub struct EventFilter {
    pub limit: i64,
    pub offset: i64,
    pub since_slot: Option<u64>,
    pub until_slot: Option<u64>,
    pub kind: Option<String>,
    pub program: Option<String>,
    pub signature: Option<String>,
}

impl EventFilter {
    pub fn normalize(mut self) -> Self {
        if self.limit <= 0 {
            self.limit = 100;
        } else if self.limit > 500 {
            self.limit = 500;
        }
        if self.offset < 0 {
            self.offset = 0;
        }
        self
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CursorInfo {
    pub slot: u64,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct GapRecord {
    pub from_slot: u64,
    pub to_slot: u64,
    pub filled: bool,
}

pub struct EventStore {
    backend: DbBackend,
}

impl EventStore {
    pub async fn open(database_url: &str) -> Result<Self> {
        if database_url.starts_with("postgres://") || database_url.starts_with("postgresql://") {
            let pool = PgPoolOptions::new()
                .max_connections(5)
                .connect(database_url)
                .await
                .with_context(|| format!("connect postgres {database_url}"))?;
            let store = Self {
                backend: DbBackend::Postgres(pool),
            };
            store.migrate_postgres().await?;
            return Ok(store);
        }

        let url = if database_url.is_empty() {
            "sqlite:./index.db?mode=rwc".to_string()
        } else if database_url.starts_with("sqlite:") {
            if database_url.contains('?') {
                database_url.to_string()
            } else {
                format!("{database_url}?mode=rwc")
            }
        } else {
            format!("sqlite:{database_url}?mode=rwc")
        };

        if let Some(path) = url
            .strip_prefix("sqlite:")
            .and_then(|rest| rest.split('?').next())
            && let Some(parent) = std::path::Path::new(path).parent()
                && !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent).ok();
                }

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&url)
            .await
            .with_context(|| format!("connect {url}"))?;

        let store = Self {
            backend: DbBackend::Sqlite(pool),
        };
        store.migrate_sqlite().await?;
        Ok(store)
    }

    async fn migrate_sqlite(&self) -> Result<()> {
        let DbBackend::Sqlite(pool) = &self.backend else {
            anyhow::bail!("expected sqlite backend");
        };
        sqlx::migrate!("./migrations/sqlite")
            .run(pool)
            .await?;
        Ok(())
    }

    async fn migrate_postgres(&self) -> Result<()> {
        let DbBackend::Postgres(pool) = &self.backend else {
            anyhow::bail!("expected postgres backend");
        };
        sqlx::migrate!("./migrations/postgres")
            .run(pool)
            .await?;
        Ok(())
    }

    pub async fn load_cursor(&self) -> Result<Option<Cursor>> {
        match &self.backend {
            DbBackend::Sqlite(pool) => {
                let row = sqlx::query("SELECT slot FROM cursors WHERE id = 1")
                    .fetch_optional(pool)
                    .await?;
                Ok(row.map(|r| Cursor {
                    slot: r.get::<i64, _>("slot") as u64,
                }))
            }
            DbBackend::Postgres(pool) => {
                let row = sqlx::query("SELECT slot FROM cursors WHERE id = 1")
                    .fetch_optional(pool)
                    .await?;
                Ok(row.map(|r| Cursor {
                    slot: r.get::<i64, _>("slot") as u64,
                }))
            }
        }
    }

    pub async fn commit_cursor(&self, slot: u64) -> Result<()> {
        let updated = Utc::now().to_rfc3339();
        match &self.backend {
            DbBackend::Sqlite(pool) => {
                sqlx::query(
                    "INSERT INTO cursors (id, slot, updated_at) VALUES (1, ?, ?)
                     ON CONFLICT(id) DO UPDATE SET slot = excluded.slot, updated_at = excluded.updated_at",
                )
                .bind(slot as i64)
                .bind(&updated)
                .execute(pool)
                .await?;
            }
            DbBackend::Postgres(pool) => {
                sqlx::query(
                    "INSERT INTO cursors (id, slot, updated_at) VALUES (1, $1, $2)
                     ON CONFLICT(id) DO UPDATE SET slot = EXCLUDED.slot, updated_at = EXCLUDED.updated_at",
                )
                .bind(slot as i64)
                .bind(&updated)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    pub async fn record_gap(&self, from_slot: u64, to_slot: u64) -> Result<()> {
        match &self.backend {
            DbBackend::Sqlite(pool) => {
                sqlx::query(
                    "INSERT OR IGNORE INTO gaps (from_slot, to_slot, filled) VALUES (?, ?, 0)",
                )
                .bind(from_slot as i64)
                .bind(to_slot as i64)
                .execute(pool)
                .await?;
            }
            DbBackend::Postgres(pool) => {
                sqlx::query(
                    "INSERT INTO gaps (from_slot, to_slot, filled) VALUES ($1, $2, FALSE)
                     ON CONFLICT DO NOTHING",
                )
                .bind(from_slot as i64)
                .bind(to_slot as i64)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    pub async fn open_gap_count(&self) -> Result<i64> {
        match &self.backend {
            DbBackend::Sqlite(pool) => {
                let row = sqlx::query("SELECT COUNT(*) as n FROM gaps WHERE filled = 0")
                    .fetch_one(pool)
                    .await?;
                Ok(row.get("n"))
            }
            DbBackend::Postgres(pool) => {
                let row = sqlx::query("SELECT COUNT(*) as n FROM gaps WHERE filled = FALSE")
                    .fetch_one(pool)
                    .await?;
                Ok(row.get("n"))
            }
        }
    }

    pub async fn write_event(&self, event: &IndexEvent) -> Result<bool> {
        let signature = event.signature().unwrap_or("");
        let ix_index = event.ix_index().unwrap_or(0);
        let slot = event.slot().unwrap_or(0);
        let payload = serde_json::to_string(event)?;
        match &self.backend {
            DbBackend::Sqlite(pool) => {
                let result = sqlx::query(
                    "INSERT OR IGNORE INTO events (slot, signature, ix_index, program, kind, payload)
                     VALUES (?, ?, ?, ?, ?, ?)",
                )
                .bind(slot as i64)
                .bind(signature)
                .bind(ix_index as i64)
                .bind(event.program())
                .bind(event.kind())
                .bind(&payload)
                .execute(pool)
                .await?;
                Ok(result.rows_affected() > 0)
            }
            DbBackend::Postgres(pool) => {
                let result = sqlx::query(
                    "INSERT INTO events (slot, signature, ix_index, program, kind, payload)
                     VALUES ($1, $2, $3, $4, $5, $6::jsonb)
                     ON CONFLICT (signature, ix_index, kind) DO NOTHING",
                )
                .bind(slot as i64)
                .bind(signature)
                .bind(ix_index as i64)
                .bind(event.program())
                .bind(event.kind())
                .bind(payload)
                .execute(pool)
                .await?;
                Ok(result.rows_affected() > 0)
            }
        }
    }

    pub async fn recent_events(&self, limit: i64) -> Result<Vec<IndexEvent>> {
        match &self.backend {
            DbBackend::Sqlite(pool) => {
                let rows = sqlx::query("SELECT payload FROM events ORDER BY id DESC LIMIT ?")
                    .bind(limit)
                    .fetch_all(pool)
                    .await?;
                rows_to_events_sqlite(rows)
            }
            DbBackend::Postgres(pool) => {
                let rows = sqlx::query("SELECT payload::text as payload FROM events ORDER BY id DESC LIMIT $1")
                    .bind(limit)
                    .fetch_all(pool)
                    .await?;
                rows_to_events_postgres(rows)
            }
        }
    }

    pub async fn query_events(&self, filter: EventFilter) -> Result<Vec<IndexEvent>> {
        let filter = filter.normalize();
        match &self.backend {
            DbBackend::Sqlite(pool) => {
                let mut qb = QueryBuilder::<Sqlite>::new("SELECT payload FROM events WHERE 1=1");
                apply_event_filter(&mut qb, &filter);
                qb.push(" ORDER BY id ASC LIMIT ");
                qb.push_bind(filter.limit);
                qb.push(" OFFSET ");
                qb.push_bind(filter.offset);
                let rows = qb.build().fetch_all(pool).await?;
                rows_to_events_sqlite(rows)
            }
            DbBackend::Postgres(pool) => {
                let mut qb =
                    QueryBuilder::<sqlx::Postgres>::new("SELECT payload::text as payload FROM events WHERE 1=1");
                apply_event_filter_pg(&mut qb, &filter);
                qb.push(" ORDER BY id ASC LIMIT ");
                qb.push_bind(filter.limit);
                qb.push(" OFFSET ");
                qb.push_bind(filter.offset);
                let rows = qb.build().fetch_all(pool).await?;
                rows_to_events_postgres(rows)
            }
        }
    }

    pub async fn cursor_info(&self) -> Result<Option<CursorInfo>> {
        match &self.backend {
            DbBackend::Sqlite(pool) => {
                let row = sqlx::query("SELECT slot, updated_at FROM cursors WHERE id = 1")
                    .fetch_optional(pool)
                    .await?;
                Ok(row.map(|r| CursorInfo {
                    slot: r.get::<i64, _>("slot") as u64,
                    updated_at: r.get("updated_at"),
                }))
            }
            DbBackend::Postgres(pool) => {
                let row = sqlx::query(
                    "SELECT slot, updated_at::text as updated_at FROM cursors WHERE id = 1",
                )
                .fetch_optional(pool)
                .await?;
                Ok(row.map(|r| CursorInfo {
                    slot: r.get::<i64, _>("slot") as u64,
                    updated_at: r.get("updated_at"),
                }))
            }
        }
    }

    pub async fn list_gaps(&self, open_only: bool) -> Result<Vec<GapRecord>> {
        match &self.backend {
            DbBackend::Sqlite(pool) => {
                let rows = if open_only {
                    sqlx::query(
                        "SELECT from_slot, to_slot, filled FROM gaps WHERE filled = 0 ORDER BY from_slot ASC",
                    )
                    .fetch_all(pool)
                    .await?
                } else {
                    sqlx::query(
                        "SELECT from_slot, to_slot, filled FROM gaps ORDER BY from_slot ASC",
                    )
                    .fetch_all(pool)
                    .await?
                };
                Ok(rows.into_iter().map(row_to_gap_sqlite).collect())
            }
            DbBackend::Postgres(pool) => {
                let rows = if open_only {
                    sqlx::query(
                        "SELECT from_slot, to_slot, filled FROM gaps WHERE filled = FALSE ORDER BY from_slot ASC",
                    )
                    .fetch_all(pool)
                    .await?
                } else {
                    sqlx::query(
                        "SELECT from_slot, to_slot, filled FROM gaps ORDER BY from_slot ASC",
                    )
                    .fetch_all(pool)
                    .await?
                };
                Ok(rows.into_iter().map(row_to_gap_postgres).collect())
            }
        }
    }

    pub async fn mark_gap_filled(&self, from_slot: u64, to_slot: u64) -> Result<()> {
        match &self.backend {
            DbBackend::Sqlite(pool) => {
                sqlx::query("UPDATE gaps SET filled = 1 WHERE from_slot = ? AND to_slot = ?")
                    .bind(from_slot as i64)
                    .bind(to_slot as i64)
                    .execute(pool)
                    .await?;
            }
            DbBackend::Postgres(pool) => {
                sqlx::query(
                    "UPDATE gaps SET filled = TRUE WHERE from_slot = $1 AND to_slot = $2",
                )
                .bind(from_slot as i64)
                .bind(to_slot as i64)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }
}

fn apply_event_filter<'a>(qb: &mut QueryBuilder<'a, Sqlite>, filter: &EventFilter) {
    if let Some(since_slot) = filter.since_slot {
        qb.push(" AND slot >= ");
        qb.push_bind(since_slot as i64);
    }
    if let Some(until_slot) = filter.until_slot {
        qb.push(" AND slot <= ");
        qb.push_bind(until_slot as i64);
    }
    if let Some(kind) = &filter.kind {
        qb.push(" AND kind = ");
        qb.push_bind(kind.clone());
    }
    if let Some(program) = &filter.program {
        qb.push(" AND program = ");
        qb.push_bind(program.clone());
    }
    if let Some(signature) = &filter.signature {
        qb.push(" AND signature = ");
        qb.push_bind(signature.clone());
    }
}

fn apply_event_filter_pg<'a>(qb: &mut QueryBuilder<'a, sqlx::Postgres>, filter: &EventFilter) {
    if let Some(since_slot) = filter.since_slot {
        qb.push(" AND slot >= ");
        qb.push_bind(since_slot as i64);
    }
    if let Some(until_slot) = filter.until_slot {
        qb.push(" AND slot <= ");
        qb.push_bind(until_slot as i64);
    }
    if let Some(kind) = &filter.kind {
        qb.push(" AND kind = ");
        qb.push_bind(kind.clone());
    }
    if let Some(program) = &filter.program {
        qb.push(" AND program = ");
        qb.push_bind(program.clone());
    }
    if let Some(signature) = &filter.signature {
        qb.push(" AND signature = ");
        qb.push_bind(signature.clone());
    }
}

fn rows_to_events_sqlite(rows: Vec<sqlx::sqlite::SqliteRow>) -> Result<Vec<IndexEvent>> {
    let mut out = Vec::new();
    for row in rows {
        let payload: String = row.get("payload");
        let event: IndexEvent = serde_json::from_str(&payload)
            .with_context(|| format!("deserialize event payload: {payload}"))?;
        out.push(event);
    }
    Ok(out)
}

fn rows_to_events_postgres(rows: Vec<sqlx::postgres::PgRow>) -> Result<Vec<IndexEvent>> {
    let mut out = Vec::new();
    for row in rows {
        let payload: String = row.get("payload");
        let event: IndexEvent = serde_json::from_str(&payload)
            .with_context(|| format!("deserialize event payload: {payload}"))?;
        out.push(event);
    }
    Ok(out)
}

fn row_to_gap_sqlite(row: sqlx::sqlite::SqliteRow) -> GapRecord {
    GapRecord {
        from_slot: row.get::<i64, _>("from_slot") as u64,
        to_slot: row.get::<i64, _>("to_slot") as u64,
        filled: row.get::<i64, _>("filled") != 0,
    }
}

fn row_to_gap_postgres(row: sqlx::postgres::PgRow) -> GapRecord {
    GapRecord {
        from_slot: row.get::<i64, _>("from_slot") as u64,
        to_slot: row.get::<i64, _>("to_slot") as u64,
        filled: row.get::<bool, _>("filled"),
    }
}

/// If `next` skipped slots after `last`, return the missing exclusive range.
pub fn detect_gap(last: Option<u64>, next: u64) -> Option<(u64, u64)> {
    let last = last?;
    if next > last + 1 {
        Some((last + 1, next - 1))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::event::{IndexEvent, TOKEN_PROGRAM};

    #[test]
    fn gap_detected_when_slots_skipped() {
        assert_eq!(detect_gap(Some(10), 14), Some((11, 13)));
        assert_eq!(detect_gap(Some(10), 11), None);
        assert_eq!(detect_gap(None, 5), None);
    }

    #[tokio::test]
    async fn cursor_survives_reopen() {
        let path = std::env::temp_dir().join(format!("index-cursor-{}.db", std::process::id()));
        let url = format!("sqlite:{}", path.display());
        {
            let store = EventStore::open(&url).await.unwrap();
            store.commit_cursor(99).await.unwrap();
        }
        {
            let store = EventStore::open(&url).await.unwrap();
            let cursor = store.load_cursor().await.unwrap().unwrap();
            assert_eq!(cursor.slot, 99);
        }
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn duplicate_event_is_ignored() {
        let path = std::env::temp_dir().join(format!("index-dup-{}.db", std::process::id()));
        let url = format!("sqlite:{}", path.display());
        let store = EventStore::open(&url).await.unwrap();
        let event = IndexEvent::TokenTransfer {
            slot: 1,
            signature: "sig".into(),
            ix_index: 0,
            program: TOKEN_PROGRAM.into(),
            source: "a".into(),
            destination: "b".into(),
            authority: None,
            mint: None,
            amount: 1,
            decimals: None,
        };
        assert!(store.write_event(&event).await.unwrap());
        assert!(!store.write_event(&event).await.unwrap());
        let _ = std::fs::remove_file(&path);
    }

    fn token_transfer(slot: u64, signature: &str) -> IndexEvent {
        IndexEvent::TokenTransfer {
            slot,
            signature: signature.into(),
            ix_index: 0,
            program: TOKEN_PROGRAM.into(),
            source: "a".into(),
            destination: "b".into(),
            authority: None,
            mint: None,
            amount: slot,
            decimals: None,
        }
    }

    #[test]
    fn event_filter_normalizes_limit_and_offset() {
        let filter = EventFilter {
            limit: 0,
            offset: -5,
            ..Default::default()
        }
        .normalize();
        assert_eq!(filter.limit, 100);
        assert_eq!(filter.offset, 0);

        let filter = EventFilter {
            limit: 1000,
            offset: 10,
            ..Default::default()
        }
        .normalize();
        assert_eq!(filter.limit, 500);
        assert_eq!(filter.offset, 10);
    }

    #[tokio::test]
    async fn query_events_applies_filters() {
        let path = std::env::temp_dir().join(format!("index-query-{}.db", std::process::id()));
        let url = format!("sqlite:{}", path.display());
        let store = EventStore::open(&url).await.unwrap();

        store.write_event(&token_transfer(10, "sig-a")).await.unwrap();
        store.write_event(&token_transfer(20, "sig-b")).await.unwrap();
        store
            .write_event(&IndexEvent::ProgramIx {
                slot: 15,
                signature: "sig-c".into(),
                ix_index: 0,
                program_id: "prog".into(),
                program_name: "pump".into(),
                instruction: "swap".into(),
                accounts: vec![],
                args: None,
                named_accounts: None,
            })
            .await
            .unwrap();

        let all = store
            .query_events(EventFilter {
                limit: 100,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(all.len(), 3);

        let since = store
            .query_events(EventFilter {
                since_slot: Some(15),
                limit: 100,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(since.len(), 2);
        assert!(since.iter().all(|e| e.slot().unwrap_or(0) >= 15));

        let until = store
            .query_events(EventFilter {
                until_slot: Some(15),
                limit: 100,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(until.len(), 2);

        let by_kind = store
            .query_events(EventFilter {
                kind: Some("program_ix".into()),
                limit: 100,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(by_kind.len(), 1);
        assert_eq!(by_kind[0].kind(), "program_ix");

        let by_sig = store
            .query_events(EventFilter {
                signature: Some("sig-a".into()),
                limit: 100,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(by_sig.len(), 1);
        assert_eq!(by_sig[0].signature(), Some("sig-a"));

        let paged = store
            .query_events(EventFilter {
                limit: 1,
                offset: 1,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(paged.len(), 1);

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn cursor_info_and_gap_queries() {
        let path = std::env::temp_dir().join(format!("index-gaps-{}.db", std::process::id()));
        let url = format!("sqlite:{}", path.display());
        let store = EventStore::open(&url).await.unwrap();

        assert!(store.cursor_info().await.unwrap().is_none());
        store.commit_cursor(42).await.unwrap();
        let info = store.cursor_info().await.unwrap().unwrap();
        assert_eq!(info.slot, 42);
        assert!(!info.updated_at.is_empty());

        store.record_gap(10, 12).await.unwrap();
        store.record_gap(20, 22).await.unwrap();

        let open = store.list_gaps(true).await.unwrap();
        assert_eq!(open.len(), 2);
        assert!(!open[0].filled);

        store.mark_gap_filled(10, 12).await.unwrap();
        let open = store.list_gaps(true).await.unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].from_slot, 20);

        let all = store.list_gaps(false).await.unwrap();
        assert_eq!(all.len(), 2);
        assert!(all.iter().any(|g| g.from_slot == 10 && g.filled));

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn replay_roundtrip() {
        let path = std::env::temp_dir().join(format!("index-replay-{}.db", std::process::id()));
        let url = format!("sqlite:{}", path.display());
        let store = EventStore::open(&url).await.unwrap();

        let program_ix = IndexEvent::ProgramIx {
            slot: 105,
            signature: "replay-sig-2".into(),
            ix_index: 1,
            program_id: "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P".into(),
            program_name: "pump".into(),
            instruction: "buy".into(),
            accounts: vec!["acct-a".into(), "acct-b".into()],
            args: Some(serde_json::json!({ "amount": 1_000_000 })),
            named_accounts: None,
        };
        let transfer = token_transfer(100, "replay-sig-1");

        assert!(store.write_event(&transfer).await.unwrap());
        assert!(store.write_event(&program_ix).await.unwrap());
        store.commit_cursor(105).await.unwrap();

        let cursor = store.load_cursor().await.unwrap().unwrap();
        assert_eq!(cursor.slot, 105);

        let queried = store
            .query_events(EventFilter {
                limit: 100,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(queried.len(), 2);

        let transfer_back = queried
            .iter()
            .find(|e| e.signature() == Some("replay-sig-1"))
            .expect("transfer event");
        assert_eq!(transfer_back.slot(), Some(100));
        assert_eq!(transfer_back.kind(), "token_transfer");
        assert_eq!(transfer_back.program(), TOKEN_PROGRAM);

        let ix_back = queried
            .iter()
            .find(|e| e.signature() == Some("replay-sig-2"))
            .expect("program ix event");
        assert_eq!(ix_back.slot(), Some(105));
        assert_eq!(ix_back.kind(), "program_ix");
        assert_eq!(ix_back.program(), "pump");

        let since = store
            .query_events(EventFilter {
                since_slot: Some(101),
                limit: 100,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(since.len(), 1);
        assert_eq!(since[0].signature(), Some("replay-sig-2"));

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn replay_detects_and_persists_gaps() {
        let path = std::env::temp_dir().join(format!("index-replay-gap-{}.db", std::process::id()));
        let url = format!("sqlite:{}", path.display());
        let store = EventStore::open(&url).await.unwrap();

        store.commit_cursor(10).await.unwrap();
        assert_eq!(detect_gap(Some(10), 14), Some((11, 13)));
        store.record_gap(11, 13).await.unwrap();

        let gaps = store.list_gaps(true).await.unwrap();
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].from_slot, 11);
        assert_eq!(gaps[0].to_slot, 13);

        store.write_event(&IndexEvent::Gap {
            from_slot: 11,
            to_slot: 13,
        })
        .await
        .unwrap();

        let gap_events = store
            .query_events(EventFilter {
                kind: Some("gap".into()),
                limit: 10,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(gap_events.len(), 1);

        let _ = std::fs::remove_file(&path);
    }
}
