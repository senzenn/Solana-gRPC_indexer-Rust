# Indexer "No Assumptions" Plan

This plan makes the Solana-gRPC_indexer-Rust pipeline generic, with no built-in assumptions about the data. Downstream systems (for example shipstern and Carbon data lakes) configure it with `index.yaml` only. They do not change the Rust source code.

## Phase 1: The "Clean Slate" Refactor
**Goal:** Remove legacy code. Keep one pipeline architecture, so that contributors have less code to read.
*   [x] Delete unused legacy files in `src/` (`api.rs`, `database.rs`, `cache.rs`, `metrics.rs`, `grpc_server.rs`, `webhooks.rs`, `slot_tracker.rs`, `wallet_tracker.rs`, `account_watcher.rs`, `yellowstone_monitor.rs`).
*   [x] Remove the hidden `legacy` subcommand block from `src/main.rs`.
*   [x] Clean up `src/config.rs` if the main CLI no longer uses it.

## Phase 2: Dependency & Build Fixes
**Goal:** Make the repository build and deploy from a plain clone, with no sibling repositories.
*   [x] Resolve the `../idl-drift` local path dependency in `Cargo.toml` by vendoring the crate at `vendor/idl-drift` (no upstream git PR required).
*   [x] Update the `Dockerfile` to remove the context dependency on the parent directory (`../`), allowing standard `docker build .` to succeed.

## Phase 3: Advanced Geyser Configuration
**Goal:** Remove hardcoded assumptions about what data the user wants to index (e.g., forcing heavy SPL Token parsing).
*   [x] Update `src/pipeline/config_file.rs` to expose Yellowstone Geyser filter parameters directly (e.g., `track_failed: bool`, `accounts_include: [String]`).
*   [x] Remove the hardcoded `TOKEN_PROGRAM` and `TOKEN_2022_PROGRAM` from the default Geyser subscription in `src/pipeline/ingest.rs`. 
*   [x] Add an explicit `track_tokens: bool` flag to `index.yaml` so that users opt in to token traffic only when they need it.

## Phase 4: True "No Assumptions" Storage Layer
**Goal:** Ensure the database schema is flexible enough to handle any program's IDL without breaking or requiring schema migrations for new protocols.
*   [x] Refactor `src/pipeline/store.rs` to use `sqlx::migrate!()` to run the actual SQL files in the `migrations/` folder, instead of the current inline `CREATE TABLE` strings.
*   [x] Review the `events` table schema. The `payload` column uses Postgres `JSONB` or SQLite `JSON`, so the table structure does not depend on a protocol.

## Phase 5: Sink Routing
**Goal:** Allow users to route specific instructions to specific destinations (e.g., lightweight webhooks for alerts, heavy DBs for analytics).
*   [x] Add an `instructions: [String]` array to `SinkConfig` in `index.yaml`.
*   [x] Update `matches_filter` in `src/pipeline/sink.rs`. It drops an event when its instruction name is not in the list. For example, a webhook gets only `Swap` instructions, and the DB stores all events.

## Phase 6: Documentation
**Goal:** Ship operator and contributor docs so the indexer runs from clone + config alone.
*   [x] Quickstart (`docs/quickstart.md`): git clone -> `.env` -> `index.yaml` -> run
*   [x] Config reference (`docs/config-reference.md`): every `index.yaml` field + env overrides
*   [x] Operations runbook (`docs/operations-runbook.md`): cursor, gaps, backfill, IDL drift, troubleshooting
*   [x] README links to `docs/` and removes sibling `idl-drift` clone requirement

## Phase 7: Product gaps (open)

Phases 1 to 6 made the pipeline config-driven. The remaining work is features, not hygiene.

See [feature-gaps.md](./feature-gaps.md). Path-based identity and finalized/reorg handling are done. Next: event (`emit!`) decode, full Token parsers and account decode, then more sinks.
