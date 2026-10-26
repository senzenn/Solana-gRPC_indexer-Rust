-- open_gap_count and `/gaps?open_only=true` filter on filled = FALSE. Without an
-- index each call scanned every gap row, filled or not.

CREATE INDEX IF NOT EXISTS gaps_open_idx ON gaps (from_slot) WHERE filled = FALSE;
