# Indexer "No Assumptions" Modernization Plan

This document outlines the complete plan to transform the Solana-gRPC_indexer-Rust pipeline into a production-ready, generic, "no assumptions" architecture. The goal is to allow downstream systems (like Vixen and Carbon data lakes) to consume this pipeline entirely via configuration (`index.yaml`) without requiring modifications to the Rust source code.

## Phase 1: The "Clean Slate" Refactor
**Goal:** Remove all technical debt and legacy code to make the repository approachable for open-source contributors and standardise around the modern pipeline architecture.
*   [x] Delete unused legacy files in `src/` (`api.rs`, `database.rs`, `cache.rs`, `metrics.rs`, `grpc_server.rs`, `webhooks.rs`, `slot_tracker.rs`, `wallet_tracker.rs`, `account_watcher.rs`, `yellowstone_monitor.rs`).
*   [x] Remove the hidden `legacy` subcommand block from `src/main.rs`.
*   [x] Clean up `src/config.rs` if it's no longer used by the main CLI.

## Phase 2: Dependency & Build Fixes
**Goal:** Ensure the repository can be built and deployed out-of-the-box by any user anywhere.
*   [x] Resolve the `../idl-drift` local path dependency in `Cargo.toml` by vendoring the crate at `vendor/idl-drift` (no upstream git PR required).
*   [x] Update the `Dockerfile` to remove the context dependency on the parent directory (`../`), allowing standard `docker build .` to succeed.

## Phase 3: Advanced Geyser Configuration
**Goal:** Remove hardcoded assumptions about what data the user wants to index (e.g., forcing heavy SPL Token parsing).
*   [x] Update `src/pipeline/config_file.rs` to expose Yellowstone Geyser filter parameters directly (e.g., `track_failed: bool`, `accounts_include: [String]`).
*   [x] Remove the hardcoded `TOKEN_PROGRAM` and `TOKEN_2022_PROGRAM` from the default Geyser subscription in `src/pipeline/ingest.rs`. 
*   [x] Add an explicit `track_tokens: bool` flag to `index.yaml` so users can consciously opt-in to heavy token traffic if their use case requires it.

## Phase 4: True "No Assumptions" Storage Layer
**Goal:** Ensure the database schema is flexible enough to handle any program's IDL without breaking or requiring schema migrations for new protocols.
*   [x] Refactor `src/pipeline/store.rs` to use `sqlx::migrate!()` to run the actual SQL files in the `migrations/` folder, instead of the current inline `CREATE TABLE` strings.
*   [x] Review the `events` table schema to ensure it heavily relies on Postgres `JSONB` / SQLite `JSON` for the `payload`, keeping the table structure entirely protocol-agnostic.

## Phase 5: Smart Sink Routing
**Goal:** Allow users to route specific instructions to specific destinations (e.g., lightweight webhooks for alerts, heavy DBs for analytics).
*   [x] Enhance `SinkConfig` in `index.yaml` to accept an `instructions: [String]` array.
*   [x] Update `matches_filter` in `src/pipeline/sink.rs` to filter out events if their instruction name doesn't match the configured list (e.g., routing only `Swap` instructions to a webhook while saving everything else to the DB).

## Phase 6: Documentation
**Goal:** Ship operator and contributor docs so the indexer runs from clone + config alone.
*   [x] Quickstart (`docs/quickstart.md`) — git clone → `.env` → `index.yaml` → run
*   [x] Config reference (`docs/config-reference.md`) — every `index.yaml` field + env overrides
*   [x] Operations runbook (`docs/operations-runbook.md`) — cursor, gaps, backfill, IDL drift, troubleshooting
*   [x] README links to `docs/` and removes sibling `idl-drift` clone requirement
