CREATE TABLE cursors (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    slot BIGINT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE gaps (
    from_slot BIGINT NOT NULL,
    to_slot BIGINT NOT NULL,
    filled BOOLEAN NOT NULL DEFAULT FALSE,
    PRIMARY KEY (from_slot, to_slot)
);

CREATE TABLE events (
    id BIGSERIAL PRIMARY KEY,
    slot BIGINT NOT NULL,
    signature TEXT NOT NULL DEFAULT '',
    ix_index INTEGER NOT NULL DEFAULT 0,
    program TEXT NOT NULL,
    kind TEXT NOT NULL,
    payload JSONB NOT NULL,
    UNIQUE(signature, ix_index, kind)
);

CREATE TABLE program_idls (
    program_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    idl_json JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
