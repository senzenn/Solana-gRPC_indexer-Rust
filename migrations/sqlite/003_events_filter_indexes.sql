-- `/events` filters on kind and program were full table scans: at ~320k rows a
-- `WHERE program = ?` took 1.4s and tripped sqlx's slow-statement warning.
--
-- `id` is the ORDER BY column, so it trails each key: the planner seeks the
-- equality prefix and walks the index in id order, skipping the sort.
-- `signature` is already served by the UNIQUE(signature, ix_path, kind) index
-- and `slot` by events_slot_idx.

CREATE INDEX IF NOT EXISTS events_kind_idx ON events (kind, id);
CREATE INDEX IF NOT EXISTS events_program_idx ON events (program, id);
