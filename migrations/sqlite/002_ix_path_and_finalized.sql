-- Path-based event identity + dual cursor + slot parent map.

ALTER TABLE cursors ADD COLUMN finalized_slot INTEGER NOT NULL DEFAULT 0;

CREATE TABLE IF NOT EXISTS slots (
    slot INTEGER PRIMARY KEY,
    parent INTEGER,
    status TEXT NOT NULL DEFAULT 'confirmed'
);

CREATE TABLE events_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    slot INTEGER NOT NULL,
    signature TEXT NOT NULL DEFAULT '',
    ix_index INTEGER NOT NULL DEFAULT 0,
    ix_path TEXT NOT NULL DEFAULT '0',
    program TEXT NOT NULL,
    kind TEXT NOT NULL,
    payload TEXT NOT NULL,
    UNIQUE(signature, ix_path, kind)
);

INSERT INTO events_new (id, slot, signature, ix_index, ix_path, program, kind, payload)
SELECT id, slot, signature, ix_index, CAST(ix_index AS TEXT), program, kind, payload
FROM events;

DROP TABLE events;
ALTER TABLE events_new RENAME TO events;

CREATE INDEX IF NOT EXISTS events_slot_idx ON events(slot);
