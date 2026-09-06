# cpi_event_anchor_to_pinocchio

## Real-world situation
Vixen self-CPI event decode depends on the envelope discriminator length and
payload offset (`cpi_event_discriminator` / `cpi_event_payload_offset`). Anchor
defaults to an 8-byte event-ix tag; many Pinocchio programs use a 1-byte wrap.
Migrating the program changes where payload bytes start — a hard parse break.

## Expected
- Classification: **Breaking** (`event envelope discriminator LENGTH` … `8->1`)
- Exit code: **1**
