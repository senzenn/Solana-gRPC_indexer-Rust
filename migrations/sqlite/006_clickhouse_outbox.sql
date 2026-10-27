-- Events that still need a copy in ClickHouse. A row is written in the same
-- transaction as its event, so a crash or a ClickHouse outage loses nothing.
CREATE TABLE IF NOT EXISTS ch_outbox (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id INTEGER NOT NULL,
    slot INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS ch_outbox_slot_idx ON ch_outbox (slot);
