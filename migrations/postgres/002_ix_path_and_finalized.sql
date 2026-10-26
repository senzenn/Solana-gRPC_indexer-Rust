ALTER TABLE cursors ADD COLUMN IF NOT EXISTS finalized_slot BIGINT NOT NULL DEFAULT 0;

CREATE TABLE IF NOT EXISTS slots (
    slot BIGINT PRIMARY KEY,
    parent BIGINT,
    status TEXT NOT NULL DEFAULT 'confirmed'
);

ALTER TABLE events ADD COLUMN IF NOT EXISTS ix_path TEXT NOT NULL DEFAULT '0';
UPDATE events SET ix_path = ix_index::text;

ALTER TABLE events DROP CONSTRAINT IF EXISTS events_signature_ix_index_kind_key;
CREATE UNIQUE INDEX IF NOT EXISTS events_signature_ix_path_kind
    ON events (signature, ix_path, kind);
CREATE INDEX IF NOT EXISTS events_slot_idx ON events (slot);
