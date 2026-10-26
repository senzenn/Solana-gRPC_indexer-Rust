use std::borrow::Cow;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::Utc;
use serde::Serialize;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool, postgres::PgPool};
use tokio::sync::{Semaphore, SemaphorePermit};

use super::event::IndexEvent;
use super::reorg::{SlotCommitment, orphans_if_chain_complete};

pub const MAX_EVENT_LIMIT: i64 = 500;
/// A large OFFSET makes the database walk that many rows. Deeper pages must use `since_slot`.
pub const MAX_EVENT_OFFSET: i64 = 10_000;
pub const MAX_GAPS_LISTED: i64 = 1_000;

const POOL_CONNECTIONS: u32 = 5;
/// API reads share the pool with ingest. Keep 2 connections free for ingest writes.
const API_READ_PERMITS: usize = POOL_CONNECTIONS as usize - 2;
const API_READ_WAIT: Duration = Duration::from_secs(5);
/// Same limit as the SQLite busy wait. A runaway query must not hold a pool connection.
const PG_STATEMENT_TIMEOUT: &str = "30s";
/// Postgres btree index rows have a size limit. IDL names are not trusted.
const MAX_PROGRAM_BYTES: usize = 128;

/// An API read did not get a read permit in time. The HTTP and gRPC layers return 503.
#[derive(Debug)]
pub struct StoreBusy;

impl std::fmt::Display for StoreBusy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("store busy")
    }
}

impl std::error::Error for StoreBusy {}

enum DbBackend {
    Sqlite(SqlitePool),
    Postgres(PgPool),
}

#[derive(Debug, Clone)]
pub struct Cursor {
    pub slot: u64,
    pub finalized_slot: u64,
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
        } else if self.limit > MAX_EVENT_LIMIT {
            self.limit = MAX_EVENT_LIMIT;
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
    pub finalized_slot: u64,
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
    api_reads: Semaphore,
    api_read_wait: Duration,
}

impl EventStore {
    fn new(backend: DbBackend) -> Self {
        Self {
            backend,
            api_reads: Semaphore::new(API_READ_PERMITS),
            api_read_wait: API_READ_WAIT,
        }
    }

    /// Only methods that serve API clients take this permit. Ingest paths must not.
    async fn api_read_permit(&self) -> Result<SemaphorePermit<'_>> {
        match tokio::time::timeout(self.api_read_wait, self.api_reads.acquire()).await {
            Ok(Ok(permit)) => Ok(permit),
            _ => Err(anyhow::Error::new(StoreBusy)),
        }
    }

    pub async fn open(database_url: &str) -> Result<Self> {
        if database_url.starts_with("postgres://") || database_url.starts_with("postgresql://") {
            let pool = PgPoolOptions::new()
                .max_connections(POOL_CONNECTIONS)
                .connect_with(pg_options(database_url)?)
                .await
                .with_context(|| format!("connect postgres {}", redact_url(database_url)))?;
            let store = Self::new(DbBackend::Postgres(pool));
            store.migrate_postgres().await?;
            return Ok(store);
        }

        // Without this check a typo or mysql:// URL silently becomes a local SQLite file.
        if !database_url.starts_with("sqlite:") && database_url.contains("://") {
            let scheme = url::Url::parse(database_url)
                .map(|u| u.scheme().to_string())
                .unwrap_or_else(|_| "unknown".to_string());
            anyhow::bail!(
                "unsupported database url scheme {scheme:?}: use sqlite:<path>, a file path, or postgres://"
            );
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
            .strip_prefix("sqlite://")
            .or_else(|| url.strip_prefix("sqlite:"))
            .and_then(|rest| rest.split('?').next())
            && let Some(parent) = std::path::Path::new(path).parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).ok();
        }

        // WAL lets API reads run during ingest writes. The default 5s busy wait made
        // a slow read fail the ingest write.
        let options = SqliteConnectOptions::from_str(&url)
            .with_context(|| format!("parse {url}"))?
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(30));
        let pool = SqlitePoolOptions::new()
            .max_connections(POOL_CONNECTIONS)
            .connect_with(options)
            .await
            .with_context(|| format!("connect {url}"))?;

        let store = Self::new(DbBackend::Sqlite(pool));
        store.migrate_sqlite().await?;
        Ok(store)
    }

    async fn migrate_sqlite(&self) -> Result<()> {
        let DbBackend::Sqlite(pool) = &self.backend else {
            anyhow::bail!("expected sqlite backend");
        };
        sqlx::migrate!("./migrations/sqlite").run(pool).await?;
        Ok(())
    }

    async fn migrate_postgres(&self) -> Result<()> {
        let DbBackend::Postgres(pool) = &self.backend else {
            anyhow::bail!("expected postgres backend");
        };
        sqlx::migrate!("./migrations/postgres").run(pool).await?;
        Ok(())
    }

    pub async fn load_cursor(&self) -> Result<Option<Cursor>> {
        match &self.backend {
            DbBackend::Sqlite(pool) => {
                let row = sqlx::query("SELECT slot, finalized_slot FROM cursors WHERE id = 1")
                    .fetch_optional(pool)
                    .await?;
                Ok(row.map(|r| Cursor {
                    slot: r.get::<i64, _>("slot") as u64,
                    finalized_slot: r.get::<i64, _>("finalized_slot") as u64,
                }))
            }
            DbBackend::Postgres(pool) => {
                let row = sqlx::query("SELECT slot, finalized_slot FROM cursors WHERE id = 1")
                    .fetch_optional(pool)
                    .await?;
                Ok(row.map(|r| Cursor {
                    slot: r.get::<i64, _>("slot") as u64,
                    finalized_slot: r.get::<i64, _>("finalized_slot") as u64,
                }))
            }
        }
    }

    pub async fn commit_cursor(&self, slot: u64) -> Result<()> {
        self.commit_cursors(slot, None).await
    }

    pub async fn commit_cursors(&self, slot: u64, finalized_slot: Option<u64>) -> Result<()> {
        let updated = Utc::now().to_rfc3339();
        let slot = slot_i64(slot)?;
        let finalized_slot = finalized_slot.map(slot_i64).transpose()?;
        match &self.backend {
            DbBackend::Sqlite(pool) => {
                sqlx::query(
                    "INSERT INTO cursors (id, slot, finalized_slot, updated_at) VALUES (1, ?, ?, ?)
                     ON CONFLICT(id) DO UPDATE SET
                        slot = excluded.slot,
                        finalized_slot = COALESCE(?, cursors.finalized_slot),
                        updated_at = excluded.updated_at",
                )
                .bind(slot)
                .bind(finalized_slot.unwrap_or(0))
                .bind(&updated)
                .bind(finalized_slot)
                .execute(pool)
                .await?;
            }
            DbBackend::Postgres(pool) => {
                sqlx::query(
                    "INSERT INTO cursors (id, slot, finalized_slot, updated_at) VALUES (1, $1, $2, $3::timestamptz)
                     ON CONFLICT(id) DO UPDATE SET
                        slot = EXCLUDED.slot,
                        finalized_slot = COALESCE($4, cursors.finalized_slot),
                        updated_at = EXCLUDED.updated_at",
                )
                .bind(slot)
                .bind(finalized_slot.unwrap_or(0))
                .bind(&updated)
                .bind(finalized_slot)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    pub async fn record_gap(&self, from_slot: u64, to_slot: u64) -> Result<()> {
        let (from_slot, to_slot) = (slot_i64(from_slot)?, slot_i64(to_slot)?);
        match &self.backend {
            DbBackend::Sqlite(pool) => {
                sqlx::query(
                    "INSERT OR IGNORE INTO gaps (from_slot, to_slot, filled) VALUES (?, ?, 0)",
                )
                .bind(from_slot)
                .bind(to_slot)
                .execute(pool)
                .await?;
            }
            DbBackend::Postgres(pool) => {
                sqlx::query(
                    "INSERT INTO gaps (from_slot, to_slot, filled) VALUES ($1, $2, FALSE)
                     ON CONFLICT DO NOTHING",
                )
                .bind(from_slot)
                .bind(to_slot)
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
        let mut event = event.clone();
        event.fill_ix_path();
        let (signature, ix_path) = event.store_identity();
        let ix_index = event.ix_index().unwrap_or(0);
        let slot = slot_i64(event.slot().unwrap_or(0))?;
        let program = without_nul(event.program());
        let program = truncate_utf8(&program, MAX_PROGRAM_BYTES);
        let payload = event_payload(&event)?;
        match &self.backend {
            DbBackend::Sqlite(pool) => {
                let result = sqlx::query(
                    "INSERT INTO events (slot, signature, ix_index, ix_path, program, kind, payload)
                     VALUES (?, ?, ?, ?, ?, ?, ?)
                     ON CONFLICT(signature, ix_path, kind) DO UPDATE SET
                        slot = excluded.slot,
                        program = excluded.program,
                        payload = excluded.payload
                     WHERE events.payload != excluded.payload OR events.slot != excluded.slot",
                )
                .bind(slot)
                .bind(&signature)
                .bind(ix_index as i64)
                .bind(&ix_path)
                .bind(program)
                .bind(event.kind())
                .bind(&payload)
                .execute(pool)
                .await?;
                Ok(result.rows_affected() > 0)
            }
            DbBackend::Postgres(pool) => {
                let result = sqlx::query(
                    "INSERT INTO events (slot, signature, ix_index, ix_path, program, kind, payload)
                     VALUES ($1, $2, $3, $4, $5, $6, $7::jsonb)
                     ON CONFLICT (signature, ix_path, kind) DO UPDATE SET
                        slot = EXCLUDED.slot,
                        program = EXCLUDED.program,
                        payload = EXCLUDED.payload
                     WHERE events.payload IS DISTINCT FROM EXCLUDED.payload
                        OR events.slot IS DISTINCT FROM EXCLUDED.slot",
                )
                .bind(slot)
                .bind(&signature)
                .bind(ix_index as i64)
                .bind(&ix_path)
                .bind(program)
                .bind(event.kind())
                .bind(payload)
                .execute(pool)
                .await?;
                Ok(result.rows_affected() > 0)
            }
        }
    }

    pub async fn recent_events(&self, limit: i64) -> Result<Vec<IndexEvent>> {
        let limit = limit.clamp(1, MAX_EVENT_LIMIT);
        let _permit = self.api_read_permit().await?;
        match &self.backend {
            DbBackend::Sqlite(pool) => {
                let rows = sqlx::query("SELECT payload FROM events ORDER BY id DESC LIMIT ?")
                    .bind(limit)
                    .fetch_all(pool)
                    .await?;
                rows_to_events_sqlite(rows)
            }
            DbBackend::Postgres(pool) => {
                let rows = sqlx::query(
                    "SELECT payload::text as payload FROM events ORDER BY id DESC LIMIT $1",
                )
                .bind(limit)
                .fetch_all(pool)
                .await?;
                rows_to_events_postgres(rows)
            }
        }
    }

    pub async fn query_events(&self, filter: EventFilter) -> Result<Vec<IndexEvent>> {
        let filter = filter.normalize();
        anyhow::ensure!(
            filter.offset <= MAX_EVENT_OFFSET,
            "offset {} is above the maximum {MAX_EVENT_OFFSET}; page with since_slot",
            filter.offset
        );
        let _permit = self.api_read_permit().await?;
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
                let mut qb = QueryBuilder::<sqlx::Postgres>::new(
                    "SELECT payload::text as payload FROM events WHERE 1=1",
                );
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
        let _permit = self.api_read_permit().await?;
        match &self.backend {
            DbBackend::Sqlite(pool) => {
                let row = sqlx::query(
                    "SELECT slot, finalized_slot, updated_at FROM cursors WHERE id = 1",
                )
                .fetch_optional(pool)
                .await?;
                Ok(row.map(|r| CursorInfo {
                    slot: r.get::<i64, _>("slot") as u64,
                    finalized_slot: r.get::<i64, _>("finalized_slot") as u64,
                    updated_at: r.get("updated_at"),
                }))
            }
            DbBackend::Postgres(pool) => {
                let row = sqlx::query(
                    "SELECT slot, finalized_slot, updated_at::text as updated_at FROM cursors WHERE id = 1",
                )
                .fetch_optional(pool)
                .await?;
                Ok(row.map(|r| CursorInfo {
                    slot: r.get::<i64, _>("slot") as u64,
                    finalized_slot: r.get::<i64, _>("finalized_slot") as u64,
                    updated_at: r.get("updated_at"),
                }))
            }
        }
    }

    pub async fn record_slot(
        &self,
        slot: u64,
        parent: Option<u64>,
        status: SlotCommitment,
    ) -> Result<()> {
        let status = status.as_str();
        let slot = slot_i64(slot)?;
        let parent = parent.map(slot_i64).transpose()?;
        match &self.backend {
            DbBackend::Sqlite(pool) => {
                sqlx::query(
                    "INSERT INTO slots (slot, parent, status) VALUES (?, ?, ?)
                     ON CONFLICT(slot) DO UPDATE SET
                        parent = COALESCE(excluded.parent, slots.parent),
                        status = CASE
                            WHEN excluded.status = 'dead' THEN 'dead'
                            WHEN excluded.status = 'finalized' THEN 'finalized'
                            WHEN excluded.status = 'confirmed' AND slots.status != 'finalized'
                                THEN 'confirmed'
                            ELSE slots.status
                        END",
                )
                .bind(slot)
                .bind(parent)
                .bind(status)
                .execute(pool)
                .await?;
            }
            DbBackend::Postgres(pool) => {
                sqlx::query(
                    "INSERT INTO slots (slot, parent, status) VALUES ($1, $2, $3)
                     ON CONFLICT(slot) DO UPDATE SET
                        parent = COALESCE(EXCLUDED.parent, slots.parent),
                        status = CASE
                            WHEN EXCLUDED.status = 'dead' THEN 'dead'
                            WHEN EXCLUDED.status = 'finalized' THEN 'finalized'
                            WHEN EXCLUDED.status = 'confirmed' AND slots.status != 'finalized'
                                THEN 'confirmed'
                            ELSE slots.status
                        END",
                )
                .bind(slot)
                .bind(parent)
                .bind(status)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    pub async fn slot_parents_between(
        &self,
        from_slot: u64,
        to_slot: u64,
    ) -> Result<std::collections::HashMap<u64, u64>> {
        let mut map = std::collections::HashMap::new();
        match &self.backend {
            DbBackend::Sqlite(pool) => {
                let rows = sqlx::query(
                    "SELECT slot, parent FROM slots WHERE slot > ? AND slot <= ? AND parent IS NOT NULL",
                )
                .bind(slot_bound(from_slot))
                .bind(slot_bound(to_slot))
                .fetch_all(pool)
                .await?;
                for row in rows {
                    map.insert(
                        row.get::<i64, _>("slot") as u64,
                        row.get::<i64, _>("parent") as u64,
                    );
                }
            }
            DbBackend::Postgres(pool) => {
                let rows = sqlx::query(
                    "SELECT slot, parent FROM slots WHERE slot > $1 AND slot <= $2 AND parent IS NOT NULL",
                )
                .bind(slot_bound(from_slot))
                .bind(slot_bound(to_slot))
                .fetch_all(pool)
                .await?;
                for row in rows {
                    map.insert(
                        row.get::<i64, _>("slot") as u64,
                        row.get::<i64, _>("parent") as u64,
                    );
                }
            }
        }
        Ok(map)
    }

    pub async fn slots_between(&self, from_exclusive: u64, to_inclusive: u64) -> Result<Vec<u64>> {
        let mut slots = Vec::new();
        match &self.backend {
            DbBackend::Sqlite(pool) => {
                let rows = sqlx::query(
                    "SELECT slot FROM slots WHERE slot > ? AND slot <= ?
                     UNION
                     SELECT DISTINCT slot FROM events WHERE slot > ? AND slot <= ? AND kind NOT IN ('gap', 'idl_drift')",
                )
                .bind(slot_bound(from_exclusive))
                .bind(slot_bound(to_inclusive))
                .bind(slot_bound(from_exclusive))
                .bind(slot_bound(to_inclusive))
                .fetch_all(pool)
                .await?;
                for row in rows {
                    slots.push(row.get::<i64, _>("slot") as u64);
                }
            }
            DbBackend::Postgres(pool) => {
                let rows = sqlx::query(
                    "SELECT slot FROM slots WHERE slot > $1 AND slot <= $2
                     UNION
                     SELECT DISTINCT slot FROM events WHERE slot > $1 AND slot <= $2 AND kind NOT IN ('gap', 'idl_drift')",
                )
                .bind(slot_bound(from_exclusive))
                .bind(slot_bound(to_inclusive))
                .fetch_all(pool)
                .await?;
                for row in rows {
                    slots.push(row.get::<i64, _>("slot") as u64);
                }
            }
        }
        Ok(slots)
    }

    /// Gap and IDL drift rows are not slot data: a gap row's slot is its `to_slot`.
    pub async fn delete_events_for_slots(&self, slots: &[u64]) -> Result<u64> {
        let slots = stored_slots(slots);
        if slots.is_empty() {
            return Ok(0);
        }
        match &self.backend {
            DbBackend::Sqlite(pool) => {
                let mut qb = QueryBuilder::<Sqlite>::new(
                    "DELETE FROM events WHERE kind NOT IN ('gap', 'idl_drift') AND slot IN (",
                );
                let mut sep = qb.separated(", ");
                for slot in slots {
                    sep.push_bind(slot);
                }
                sep.push_unseparated(")");
                let result = qb.build().execute(pool).await?;
                Ok(result.rows_affected())
            }
            DbBackend::Postgres(pool) => {
                let mut qb = QueryBuilder::<sqlx::Postgres>::new(
                    "DELETE FROM events WHERE kind NOT IN ('gap', 'idl_drift') AND slot IN (",
                );
                let mut sep = qb.separated(", ");
                for slot in slots {
                    sep.push_bind(slot);
                }
                sep.push_unseparated(")");
                let result = qb.build().execute(pool).await?;
                Ok(result.rows_affected())
            }
        }
    }

    pub async fn delete_slots(&self, slots: &[u64]) -> Result<()> {
        let slots = stored_slots(slots);
        if slots.is_empty() {
            return Ok(());
        }
        match &self.backend {
            DbBackend::Sqlite(pool) => {
                let mut qb = QueryBuilder::<Sqlite>::new("DELETE FROM slots WHERE slot IN (");
                let mut sep = qb.separated(", ");
                for slot in slots {
                    sep.push_bind(slot);
                }
                sep.push_unseparated(")");
                qb.build().execute(pool).await?;
            }
            DbBackend::Postgres(pool) => {
                let mut qb =
                    QueryBuilder::<sqlx::Postgres>::new("DELETE FROM slots WHERE slot IN (");
                let mut sep = qb.separated(", ");
                for slot in slots {
                    sep.push_bind(slot);
                }
                sep.push_unseparated(")");
                qb.build().execute(pool).await?;
            }
        }
        Ok(())
    }

    /// Rows below the finalized slot are never read again: the next parent walk
    /// stops at the finalized slot.
    async fn prune_slots_below(&self, finalized: u64) -> Result<()> {
        let finalized = slot_bound(finalized);
        match &self.backend {
            DbBackend::Sqlite(pool) => {
                sqlx::query("DELETE FROM slots WHERE slot < ?")
                    .bind(finalized)
                    .execute(pool)
                    .await?;
            }
            DbBackend::Postgres(pool) => {
                sqlx::query("DELETE FROM slots WHERE slot < $1")
                    .bind(finalized)
                    .execute(pool)
                    .await?;
            }
        }
        Ok(())
    }

    /// Mark `finalized` as finalized and delete events from orphaned slots.
    ///
    /// First finalized update only records the cursor. Later updates walk the
    /// parent chain; if the chain is incomplete, nothing is deleted.
    ///
    /// The resume slot does not move up to `finalized`: slots above the confirmed
    /// cursor may not be indexed yet.
    pub async fn apply_finalized(&self, finalized: u64) -> Result<Vec<u64>> {
        let cursor = self.load_cursor().await?;
        let old_finalized = cursor.as_ref().map(|c| c.finalized_slot).unwrap_or(0);
        let confirmed = cursor.as_ref().map(|c| c.slot).unwrap_or(finalized);

        if old_finalized == 0 || finalized <= old_finalized {
            // Yellowstone can send Finalized for an ancestor after its descendant.
            let finalized = finalized.max(old_finalized);
            self.commit_cursors(confirmed, Some(finalized)).await?;
            self.prune_slots_below(finalized).await?;
            return Ok(Vec::new());
        }

        let parents = self.slot_parents_between(old_finalized, finalized).await?;
        let known = self.slots_between(old_finalized, finalized).await?;
        match orphans_if_chain_complete(finalized, old_finalized, &parents, &known) {
            None => {
                tracing::warn!(
                    finalized,
                    old_finalized,
                    "parent chain incomplete; skip orphan delete"
                );
                self.commit_cursors(confirmed, Some(finalized)).await?;
                self.prune_slots_below(finalized).await?;
                Ok(Vec::new())
            }
            Some(orphans) => {
                if !orphans.is_empty() {
                    self.delete_events_for_slots(&orphans).await?;
                    self.delete_slots(&orphans).await?;
                }
                // An orphan resume slot is on a dead fork; restart from the finalized tip.
                let confirmed = if orphans.contains(&confirmed) {
                    finalized
                } else {
                    confirmed
                };
                self.commit_cursors(confirmed, Some(finalized)).await?;
                self.prune_slots_below(finalized).await?;
                Ok(orphans)
            }
        }
    }

    /// API read. Returns at most `MAX_GAPS_LISTED` rows, oldest first.
    pub async fn list_gaps(&self, open_only: bool) -> Result<Vec<GapRecord>> {
        let _permit = self.api_read_permit().await?;
        self.select_gaps(open_only).await
    }

    /// Open gaps for the backfill worker. API traffic must not block it.
    pub async fn backfill_gaps(&self) -> Result<Vec<GapRecord>> {
        self.select_gaps(true).await
    }

    async fn select_gaps(&self, open_only: bool) -> Result<Vec<GapRecord>> {
        match &self.backend {
            DbBackend::Sqlite(pool) => {
                let rows = if open_only {
                    sqlx::query(
                        "SELECT from_slot, to_slot, filled FROM gaps WHERE filled = 0 ORDER BY from_slot ASC LIMIT ?",
                    )
                    .bind(MAX_GAPS_LISTED)
                    .fetch_all(pool)
                    .await?
                } else {
                    sqlx::query(
                        "SELECT from_slot, to_slot, filled FROM gaps ORDER BY from_slot ASC LIMIT ?",
                    )
                    .bind(MAX_GAPS_LISTED)
                    .fetch_all(pool)
                    .await?
                };
                Ok(rows.into_iter().map(row_to_gap_sqlite).collect())
            }
            DbBackend::Postgres(pool) => {
                let rows = if open_only {
                    sqlx::query(
                        "SELECT from_slot, to_slot, filled FROM gaps WHERE filled = FALSE ORDER BY from_slot ASC LIMIT $1",
                    )
                    .bind(MAX_GAPS_LISTED)
                    .fetch_all(pool)
                    .await?
                } else {
                    sqlx::query(
                        "SELECT from_slot, to_slot, filled FROM gaps ORDER BY from_slot ASC LIMIT $1",
                    )
                    .bind(MAX_GAPS_LISTED)
                    .fetch_all(pool)
                    .await?
                };
                Ok(rows.into_iter().map(row_to_gap_postgres).collect())
            }
        }
    }

    pub async fn mark_gap_filled(&self, from_slot: u64, to_slot: u64) -> Result<()> {
        let (from_slot, to_slot) = (slot_i64(from_slot)?, slot_i64(to_slot)?);
        match &self.backend {
            DbBackend::Sqlite(pool) => {
                sqlx::query("UPDATE gaps SET filled = 1 WHERE from_slot = ? AND to_slot = ?")
                    .bind(from_slot)
                    .bind(to_slot)
                    .execute(pool)
                    .await?;
            }
            DbBackend::Postgres(pool) => {
                sqlx::query("UPDATE gaps SET filled = TRUE WHERE from_slot = $1 AND to_slot = $2")
                    .bind(from_slot)
                    .bind(to_slot)
                    .execute(pool)
                    .await?;
            }
        }
        Ok(())
    }
}

/// Slot as the store's BIGINT. Rejects values above `i64::MAX` so a bad upstream
/// slot is never written as a negative number.
fn slot_i64(slot: u64) -> Result<i64> {
    i64::try_from(slot).with_context(|| format!("slot {slot} is above i64::MAX"))
}

/// Slot as a range bound. No stored slot is above `i64::MAX`, so the clamp keeps
/// the predicate result the same.
fn slot_bound(slot: u64) -> i64 {
    i64::try_from(slot).unwrap_or(i64::MAX)
}

/// Slots that can be in the store. Larger values cannot match a row.
fn stored_slots(slots: &[u64]) -> Vec<i64> {
    slots
        .iter()
        .filter_map(|slot| i64::try_from(*slot).ok())
        .collect()
}

/// Postgres `jsonb` and `text` reject U+0000, so one on-chain string could stop
/// ingest. Both backends store U+FFFD in its place.
fn event_payload(event: &IndexEvent) -> Result<String> {
    let payload = serde_json::to_string(event)?;
    if !payload.contains("\\u0000") {
        return Ok(payload);
    }
    let mut value = serde_json::to_value(event)?;
    strip_nul(&mut value);
    Ok(serde_json::to_string(&value)?)
}

fn strip_nul(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(s) => {
            if s.contains('\0') {
                *s = s.replace('\0', "\u{FFFD}");
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(strip_nul),
        serde_json::Value::Object(map) => {
            if map.keys().any(|key| key.contains('\0')) {
                *map = std::mem::take(map)
                    .into_iter()
                    .map(|(key, v)| (without_nul(&key).into_owned(), v))
                    .collect();
            }
            map.values_mut().for_each(strip_nul);
        }
        _ => {}
    }
}

fn without_nul(s: &str) -> Cow<'_, str> {
    if s.contains('\0') {
        Cow::Owned(s.replace('\0', "\u{FFFD}"))
    } else {
        Cow::Borrowed(s)
    }
}

fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
    let mut end = s.len().min(max_bytes);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s.get(..end).unwrap_or_default()
}

/// Database URL without password or query string, for errors and logs.
fn redact_url(database_url: &str) -> String {
    url::Url::parse(database_url)
        .ok()
        .and_then(|mut url| {
            url.set_password(None).ok()?;
            url.set_query(None);
            Some(url.to_string())
        })
        .unwrap_or_else(|| "<unparsable url>".to_string())
}

fn pg_options(database_url: &str) -> Result<PgConnectOptions> {
    // The parse error can quote the URL, so it is not chained.
    let options = PgConnectOptions::from_str(database_url)
        .map_err(|_| anyhow::anyhow!("parse postgres url {}", redact_url(database_url)))?;
    // Postgres takes the last -c value, so a timeout set in the URL must not get a second one.
    if options
        .get_options()
        .is_some_and(|set| set.contains("statement_timeout"))
    {
        return Ok(options);
    }
    Ok(options.options([("statement_timeout", PG_STATEMENT_TIMEOUT)]))
}

fn apply_event_filter<'a>(qb: &mut QueryBuilder<'a, Sqlite>, filter: &EventFilter) {
    if let Some(since_slot) = filter.since_slot {
        qb.push(" AND slot >= ");
        qb.push_bind(slot_bound(since_slot));
    }
    if let Some(until_slot) = filter.until_slot {
        qb.push(" AND slot <= ");
        qb.push_bind(slot_bound(until_slot));
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
        qb.push_bind(slot_bound(since_slot));
    }
    if let Some(until_slot) = filter.until_slot {
        qb.push(" AND slot <= ");
        qb.push_bind(slot_bound(until_slot));
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
        out.push(normalize_event(event));
    }
    Ok(out)
}

fn rows_to_events_postgres(rows: Vec<sqlx::postgres::PgRow>) -> Result<Vec<IndexEvent>> {
    let mut out = Vec::new();
    for row in rows {
        let payload: String = row.get("payload");
        let event: IndexEvent = serde_json::from_str(&payload)
            .with_context(|| format!("deserialize event payload: {payload}"))?;
        out.push(normalize_event(event));
    }
    Ok(out)
}

fn normalize_event(mut event: IndexEvent) -> IndexEvent {
    event.fill_ix_path();
    event
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
    let first = last?.checked_add(1)?;
    if next > first {
        Some((first, next - 1))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::event::{IndexEvent, TOKEN_PROGRAM};
    use crate::pipeline::testutil::TempDb;

    #[test]
    fn gap_detected_when_slots_skipped() {
        assert_eq!(detect_gap(Some(10), 14), Some((11, 13)));
        assert_eq!(detect_gap(Some(10), 11), None);
        assert_eq!(detect_gap(None, 5), None);
        assert_eq!(detect_gap(Some(u64::MAX), 5), None);
    }

    #[tokio::test]
    async fn cursor_survives_reopen() {
        let db = TempDb::new();
        let url = db.url.clone();
        {
            let store = EventStore::open(&url).await.unwrap();
            store.commit_cursor(99).await.unwrap();
        }
        {
            let store = EventStore::open(&url).await.unwrap();
            let cursor = store.load_cursor().await.unwrap().unwrap();
            assert_eq!(cursor.slot, 99);
        }
    }

    #[tokio::test]
    async fn duplicate_event_is_ignored() {
        let db = TempDb::new();
        let url = db.url.clone();
        let store = EventStore::open(&url).await.unwrap();
        let event = IndexEvent::TokenTransfer {
            slot: 1,
            signature: "sig".into(),
            ix_index: 0,
            ix_path: "0".into(),
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
    }

    fn token_transfer(slot: u64, signature: &str) -> IndexEvent {
        token_transfer_at(slot, signature, "0")
    }

    fn token_transfer_at(slot: u64, signature: &str, path: &str) -> IndexEvent {
        let ix_index = path
            .split('.')
            .next()
            .and_then(|part| part.parse().ok())
            .unwrap_or(0);
        IndexEvent::TokenTransfer {
            slot,
            signature: signature.into(),
            ix_index,
            ix_path: path.into(),
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
        let db = TempDb::new();
        let url = db.url.clone();
        let store = EventStore::open(&url).await.unwrap();

        store
            .write_event(&token_transfer(10, "sig-a"))
            .await
            .unwrap();
        store
            .write_event(&token_transfer(20, "sig-b"))
            .await
            .unwrap();
        store
            .write_event(&IndexEvent::ProgramIx {
                slot: 15,
                signature: "sig-c".into(),
                ix_index: 0,
                ix_path: "0".into(),
                program_id: "prog".into(),
                program_name: "pump".into(),
                instruction: "swap".into(),
                accounts: vec![],
                args: None,
                named_accounts: None,
                failed: false,
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
    }

    #[tokio::test]
    async fn cursor_info_and_gap_queries() {
        let db = TempDb::new();
        let url = db.url.clone();
        let store = EventStore::open(&url).await.unwrap();

        assert!(store.cursor_info().await.unwrap().is_none());
        store.commit_cursor(42).await.unwrap();
        let info = store.cursor_info().await.unwrap().unwrap();
        assert_eq!(info.slot, 42);
        assert_eq!(info.finalized_slot, 0);
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
    }

    #[tokio::test]
    async fn replay_roundtrip() {
        let db = TempDb::new();
        let url = db.url.clone();
        let store = EventStore::open(&url).await.unwrap();

        let program_ix = IndexEvent::ProgramIx {
            slot: 105,
            signature: "replay-sig-2".into(),
            ix_index: 1,
            ix_path: "1.0".into(),
            program_id: "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P".into(),
            program_name: "pump".into(),
            instruction: "buy".into(),
            accounts: vec!["acct-a".into(), "acct-b".into()],
            args: Some(serde_json::json!({ "amount": 1_000_000 })),
            named_accounts: None,
            failed: false,
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
    }

    #[tokio::test]
    async fn replay_detects_and_persists_gaps() {
        let db = TempDb::new();
        let url = db.url.clone();
        let store = EventStore::open(&url).await.unwrap();

        store.commit_cursor(10).await.unwrap();
        assert_eq!(detect_gap(Some(10), 14), Some((11, 13)));
        store.record_gap(11, 13).await.unwrap();

        let gaps = store.list_gaps(true).await.unwrap();
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].from_slot, 11);
        assert_eq!(gaps[0].to_slot, 13);

        store
            .write_event(&IndexEvent::Gap {
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
    }

    #[tokio::test]
    async fn path_identity_keeps_outer_and_inner_distinct() {
        let db = TempDb::new();
        let url = db.url.clone();
        let store = EventStore::open(&url).await.unwrap();

        let outer = token_transfer_at(10, "sig", "0");
        let inner = token_transfer_at(10, "sig", "0.0");
        assert!(store.write_event(&outer).await.unwrap());
        assert!(store.write_event(&inner).await.unwrap());
        assert!(!store.write_event(&inner).await.unwrap());

        let all = store
            .query_events(EventFilter {
                limit: 100,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn upsert_replaces_payload_for_same_path() {
        let db = TempDb::new();
        let url = db.url.clone();
        let store = EventStore::open(&url).await.unwrap();

        let first = token_transfer_at(10, "sig", "0.1");
        assert!(store.write_event(&first).await.unwrap());

        let mut updated = token_transfer_at(10, "sig", "0.1");
        if let IndexEvent::TokenTransfer { amount, .. } = &mut updated {
            *amount = 99;
        }
        assert!(store.write_event(&updated).await.unwrap());

        let all = store
            .query_events(EventFilter {
                limit: 10,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(all.len(), 1);
        match &all[0] {
            IndexEvent::TokenTransfer {
                amount, ix_path, ..
            } => {
                assert_eq!(*amount, 99);
                assert_eq!(ix_path, "0.1");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn apply_finalized_deletes_orphan_slot_events() {
        let db = TempDb::new();
        let url = db.url.clone();
        let store = EventStore::open(&url).await.unwrap();

        store
            .record_slot(100, None, SlotCommitment::Finalized)
            .await
            .unwrap();
        store
            .record_slot(101, Some(100), SlotCommitment::Confirmed)
            .await
            .unwrap();
        store
            .record_slot(102, Some(101), SlotCommitment::Confirmed)
            .await
            .unwrap();
        store
            .record_slot(103, Some(100), SlotCommitment::Finalized)
            .await
            .unwrap();

        store.write_event(&token_transfer(100, "a")).await.unwrap();
        store.write_event(&token_transfer(101, "b")).await.unwrap();
        store.write_event(&token_transfer(102, "c")).await.unwrap();
        store.write_event(&token_transfer(103, "d")).await.unwrap();
        store.commit_cursors(102, Some(100)).await.unwrap();

        let orphans = store.apply_finalized(103).await.unwrap();
        assert_eq!(orphans, vec![101, 102]);

        let remaining = store
            .query_events(EventFilter {
                limit: 100,
                ..Default::default()
            })
            .await
            .unwrap();
        let slots: Vec<u64> = remaining.iter().filter_map(|e| e.slot()).collect();
        assert_eq!(slots, vec![100, 103]);

        let cursor = store.load_cursor().await.unwrap().unwrap();
        assert_eq!(cursor.finalized_slot, 103);
        assert_eq!(cursor.slot, 103);
    }

    #[tokio::test]
    async fn apply_finalized_skips_delete_when_parents_missing() {
        let db = TempDb::new();
        let url = db.url.clone();
        let store = EventStore::open(&url).await.unwrap();

        store.write_event(&token_transfer(101, "b")).await.unwrap();
        store.commit_cursors(101, Some(100)).await.unwrap();

        let orphans = store.apply_finalized(103).await.unwrap();
        assert!(orphans.is_empty());
        let remaining = store
            .query_events(EventFilter {
                limit: 10,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(remaining.len(), 1);
    }

    async fn memory_store() -> EventStore {
        EventStore::open("sqlite::memory:").await.unwrap()
    }

    fn all_events() -> EventFilter {
        EventFilter {
            limit: 100,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn nul_in_strings_is_replaced_before_store() {
        let store = memory_store().await;
        let event = IndexEvent::ProgramIx {
            slot: 1,
            signature: "sig".into(),
            ix_index: 0,
            ix_path: "0".into(),
            program_id: "prog".into(),
            program_name: "pu\0mp".into(),
            instruction: "create".into(),
            accounts: vec![],
            args: Some(serde_json::json!({ "name": "a\u{0}b", "k\u{0}": ["\u{0}"] })),
            named_accounts: None,
            failed: false,
        };
        assert!(store.write_event(&event).await.unwrap());

        let back = store
            .query_events(EventFilter {
                program: Some("pu\u{FFFD}mp".into()),
                ..all_events()
            })
            .await
            .unwrap();
        assert_eq!(back.len(), 1);
        let IndexEvent::ProgramIx { args, .. } = &back[0] else {
            panic!("unexpected {:?}", back[0]);
        };
        assert_eq!(
            args,
            &Some(serde_json::json!({ "name": "a\u{FFFD}b", "k\u{FFFD}": ["\u{FFFD}"] }))
        );
    }

    #[tokio::test]
    async fn query_events_rejects_offset_past_max() {
        let store = memory_store().await;
        let deep = EventFilter {
            offset: MAX_EVENT_OFFSET + 1,
            ..all_events()
        };
        assert!(store.query_events(deep).await.is_err());
        let last = EventFilter {
            offset: MAX_EVENT_OFFSET,
            ..all_events()
        };
        assert!(store.query_events(last).await.unwrap().is_empty());
    }

    #[test]
    fn redact_url_drops_password_and_query() {
        let shown = redact_url("postgresql://indexer:S3cret@db:5432/indexer?password=S3cret");
        assert!(!shown.contains("S3cret"));
        assert!(shown.contains("indexer@db:5432/indexer"));
    }

    #[tokio::test]
    async fn open_rejects_unknown_url_scheme() {
        let Err(err) = EventStore::open("mysql://indexer:S3cret@db/indexer").await else {
            panic!("mysql url must not open a sqlite file");
        };
        let msg = format!("{err:#}");
        assert!(msg.contains("\"mysql\""));
        assert!(!msg.contains("S3cret"));
    }

    #[test]
    fn pg_options_add_statement_timeout_unless_url_sets_it() {
        let opts = pg_options("postgres://u:S3cret@db/x?options[search_path]=idx").unwrap();
        assert_eq!(
            opts.get_options(),
            Some("-c search_path=idx -c statement_timeout=30s")
        );
        let opts = pg_options("postgres://u:S3cret@db/x?options[statement_timeout]=5s").unwrap();
        assert_eq!(opts.get_options(), Some("-c statement_timeout=5s"));
    }

    #[tokio::test]
    async fn sqlite_store_uses_wal() {
        let db = TempDb::new();
        let store = EventStore::open(&db.url).await.unwrap();
        let DbBackend::Sqlite(pool) = &store.backend else {
            panic!("expected sqlite");
        };
        let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(pool)
            .await
            .unwrap();
        assert_eq!(mode, "wal");
    }

    #[tokio::test]
    async fn list_gaps_is_capped() {
        let store = memory_store().await;
        for from in (0..=MAX_GAPS_LISTED as u64).map(|i| i * 10) {
            store.record_gap(from, from + 1).await.unwrap();
        }
        assert_eq!(store.open_gap_count().await.unwrap(), MAX_GAPS_LISTED + 1);
        let listed = store.list_gaps(false).await.unwrap();
        assert_eq!(listed.len(), MAX_GAPS_LISTED as usize);
        assert_eq!(listed[0].from_slot, 0);
    }

    #[tokio::test]
    async fn slots_above_i64_max_do_not_wrap() {
        let store = memory_store().await;
        store.write_event(&token_transfer(10, "a")).await.unwrap();

        let since = EventFilter {
            since_slot: Some(u64::MAX),
            ..all_events()
        };
        assert!(store.query_events(since).await.unwrap().is_empty());
        let until = EventFilter {
            until_slot: Some(u64::MAX),
            ..all_events()
        };
        assert_eq!(store.query_events(until).await.unwrap().len(), 1);

        assert!(store.commit_cursor(u64::MAX).await.is_err());
        assert!(
            store
                .write_event(&token_transfer(u64::MAX, "b"))
                .await
                .is_err()
        );
        assert!(store.load_cursor().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn apply_finalized_never_moves_finalized_back() {
        let store = memory_store().await;
        store.commit_cursors(200, Some(150)).await.unwrap();
        assert!(store.apply_finalized(100).await.unwrap().is_empty());
        let cursor = store.load_cursor().await.unwrap().unwrap();
        assert_eq!(cursor.finalized_slot, 150);
        assert_eq!(cursor.slot, 200);
    }

    #[tokio::test]
    async fn slot_delete_keeps_gap_events() {
        let store = memory_store().await;
        store
            .write_event(&IndexEvent::Gap {
                from_slot: 101,
                to_slot: 101,
            })
            .await
            .unwrap();
        store.write_event(&token_transfer(101, "a")).await.unwrap();

        assert_eq!(store.delete_events_for_slots(&[101]).await.unwrap(), 1);
        let left = store.query_events(all_events()).await.unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].kind(), "gap");
    }

    #[tokio::test]
    async fn apply_finalized_prunes_slots_below_finalized() {
        let store = memory_store().await;
        store
            .record_slot(100, None, SlotCommitment::Finalized)
            .await
            .unwrap();
        store
            .record_slot(101, Some(100), SlotCommitment::Confirmed)
            .await
            .unwrap();
        store
            .record_slot(102, Some(101), SlotCommitment::Confirmed)
            .await
            .unwrap();
        store.commit_cursors(102, Some(100)).await.unwrap();

        assert!(store.apply_finalized(101).await.unwrap().is_empty());
        let DbBackend::Sqlite(pool) = &store.backend else {
            panic!("expected sqlite");
        };
        let slots: Vec<i64> = sqlx::query_scalar("SELECT slot FROM slots ORDER BY slot")
            .fetch_all(pool)
            .await
            .unwrap();
        assert_eq!(slots, vec![101, 102]);
    }

    #[tokio::test]
    async fn migration_marks_legacy_flat_ix_path() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let migrations = dir.join("migrations");
        std::fs::create_dir_all(&migrations).unwrap();
        for (name, sql) in [
            (
                "001_events_pipeline.sql",
                include_str!("../../migrations/sqlite/001_events_pipeline.sql"),
            ),
            (
                "002_ix_path_and_finalized.sql",
                include_str!("../../migrations/sqlite/002_ix_path_and_finalized.sql"),
            ),
            (
                "003_events_filter_indexes.sql",
                include_str!("../../migrations/sqlite/003_events_filter_indexes.sql"),
            ),
            (
                "004_gaps_open_index.sql",
                include_str!("../../migrations/sqlite/004_gaps_open_index.sql"),
            ),
        ] {
            std::fs::write(migrations.join(name), sql).unwrap();
        }
        let db = dir.join("legacy.db");
        {
            let pool = SqlitePool::connect(&format!("sqlite:{}?mode=rwc", db.display()))
                .await
                .unwrap();
            sqlx::migrate::Migrator::new(migrations.as_path())
                .await
                .unwrap()
                .run(&pool)
                .await
                .unwrap();
            // Pre-002 inner ix at flat index 2: 002 copied "2" into ix_path and the payload has no ix_path.
            let legacy = r#"{"type":"token_transfer","slot":5,"signature":"old","ix_index":2,"program":"p","source":"a","destination":"b","authority":null,"mint":null,"amount":1,"decimals":null}"#;
            let current = serde_json::to_string(&token_transfer_at(5, "new", "0.1")).unwrap();
            for (signature, ix_index, ix_path, payload) in
                [("old", 2, "2", legacy), ("new", 0, "0.1", current.as_str())]
            {
                sqlx::query(
                    "INSERT INTO events (slot, signature, ix_index, ix_path, program, kind, payload)
                     VALUES (5, ?, ?, ?, 'p', 'token_transfer', ?)",
                )
                .bind(signature)
                .bind(ix_index)
                .bind(ix_path)
                .bind(payload)
                .execute(&pool)
                .await
                .unwrap();
            }
            pool.close().await;
        }

        let store = EventStore::open(&format!("sqlite:{}", db.display()))
            .await
            .unwrap();
        let events = store.query_events(all_events()).await.unwrap();
        let paths: Vec<_> = events.iter().filter_map(|e| e.ix_path()).collect();
        assert_eq!(paths, vec!["legacy:2", "0.1"]);
        let DbBackend::Sqlite(pool) = &store.backend else {
            panic!("expected sqlite");
        };
        let columns: Vec<String> = sqlx::query_scalar("SELECT ix_path FROM events ORDER BY id")
            .fetch_all(pool)
            .await
            .unwrap();
        assert_eq!(columns, vec!["legacy:2", "0.1"]);
    }

    #[tokio::test]
    async fn api_reads_return_busy_while_writes_proceed() {
        let mut store = memory_store().await;
        store.api_read_wait = Duration::from_millis(20);
        let _held = store
            .api_reads
            .acquire_many(API_READ_PERMITS as u32)
            .await
            .unwrap();

        let busy = |err: anyhow::Error| err.downcast_ref::<StoreBusy>().is_some();
        assert!(busy(store.query_events(all_events()).await.unwrap_err()));
        assert!(busy(store.recent_events(10).await.unwrap_err()));
        assert!(busy(store.cursor_info().await.unwrap_err()));
        assert!(busy(store.list_gaps(true).await.unwrap_err()));

        assert!(store.write_event(&token_transfer(1, "a")).await.unwrap());
        store.commit_cursor(1).await.unwrap();
        store.record_gap(5, 6).await.unwrap();
        assert_eq!(store.backfill_gaps().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn apply_finalized_keeps_resume_slot_on_restart() {
        let store = memory_store().await;
        store.commit_cursor(100).await.unwrap();

        assert!(store.apply_finalized(150).await.unwrap().is_empty());
        let cursor = store.load_cursor().await.unwrap().unwrap();
        assert_eq!((cursor.slot, cursor.finalized_slot), (100, 150));

        // No parents for 151..=200 were seen, so the chain is incomplete.
        assert!(store.apply_finalized(200).await.unwrap().is_empty());
        let cursor = store.load_cursor().await.unwrap().unwrap();
        assert_eq!((cursor.slot, cursor.finalized_slot), (100, 200));
    }

    #[tokio::test]
    async fn long_program_name_is_truncated_in_column_only() {
        let store = memory_store().await;
        // 'é' is 2 bytes and starts at odd offsets, so byte 128 is inside a char.
        let name = format!("a{}", "é".repeat(100));
        let event = IndexEvent::ProgramIx {
            slot: 1,
            signature: "sig".into(),
            ix_index: 0,
            ix_path: "0".into(),
            program_id: "prog".into(),
            program_name: name.clone(),
            instruction: "create".into(),
            accounts: vec![],
            args: None,
            named_accounts: None,
            failed: false,
        };
        assert!(store.write_event(&event).await.unwrap());

        let DbBackend::Sqlite(pool) = &store.backend else {
            panic!("expected sqlite");
        };
        let column: String = sqlx::query_scalar("SELECT program FROM events")
            .fetch_one(pool)
            .await
            .unwrap();
        assert_eq!(column.len(), 127);
        assert!(name.starts_with(&column));

        let back = store.query_events(all_events()).await.unwrap();
        assert_eq!(back[0].program(), name);
    }
}
