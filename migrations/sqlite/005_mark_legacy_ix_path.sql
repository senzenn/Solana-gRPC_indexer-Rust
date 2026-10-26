-- Before 002, ix_index was one flat counter: inner instructions got numbers
-- after the outer ones. 002 copied that number into ix_path, so a legacy CPI
-- row looks like a top-level instruction that does not exist. Only legacy rows
-- have no ix_path key in the payload. Mark them in the column and the payload
-- so no client reads them as a real path.

UPDATE events
SET ix_path = 'legacy:' || ix_index,
    payload = json_set(payload, '$.ix_path', 'legacy:' || ix_index)
WHERE kind IN ('token_transfer', 'program_ix')
  AND json_type(payload, '$.ix_path') IS NULL;
