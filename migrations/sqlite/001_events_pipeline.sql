CREATE TABLE cursors (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    slot INTEGER NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE gaps (
    from_slot INTEGER NOT NULL,
    to_slot INTEGER NOT NULL,
    filled INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (from_slot, to_slot)
);

CREATE TABLE events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    slot INTEGER NOT NULL,
    signature TEXT NOT NULL DEFAULT '',
    ix_index INTEGER NOT NULL DEFAULT 0,
    program TEXT NOT NULL,
    kind TEXT NOT NULL,
    payload TEXT NOT NULL,
    UNIQUE(signature, ix_index, kind)
);

CREATE TABLE program_idls (
    program_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    idl_json TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
