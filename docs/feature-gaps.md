# Product gaps (vs shipstern)

Feature gaps only. Not repo hygiene.

Sourced from this repo's README **What works** list plus the command / HTTP / gRPC tables, then checked against `src/pipeline/`. Each gap is mapped to what already exists in local shipstern (`~/oss/vixen/shipstern`, upstream `solana-rpc/shipstern`).

The geyser → decode → store → serve pipeline is the product. Shipstern is the parser/runtime/sink library. Steal parsers, CPI trees, handlers, and Kafka. Do not vendor the framework. Keep this repo the one-binary shell: cursor, gaps, typed store, HTTP/WS, TUI.

## What already works (verified)

| Claim | Where |
|-------|-------|
| Anchor IDL decode (name, args, named accounts) | `src/pipeline/idl.rs` (`IdlRouter`, 8-byte discriminator) |
| Codama `rootNode` IDLs (what `pump_fun.json` is) | `vendor/idl-drift/src/codama_convert.rs` |
| Pre-0.30 Anchor IDLs (no `address`, no discriminators) | `idl_fetch.rs` stamps the id, `model.rs` derives `sha256("global:<fn>")[..8]` |
| Borsh scalars incl. `f32` / `f64` | `model.rs` `Type`, `src/pipeline/decode_args.rs` |
| `/events` filter indexes on `kind` / `program` | `migrations/*/003_events_filter_indexes.sql` |
| Opt-in SPL Token / Token-2022 **transfers only** | `src/pipeline/token.rs` (`TRANSFER` / `TRANSFER_CHECKED`) |
| Inner instructions flattened into the same stream | `src/pipeline/ingest.rs` (`meta.inner_instructions`) |
| Geyser filters | `track_tokens`, `track_failed`, `accounts_include` |
| Cursor resume + gap detect + RPC backfill | `store.rs`, `run.rs`, `backfill.rs` |
| Store unique key | `UNIQUE(signature, ix_path, kind)` in `migrations/*/002_ix_path_and_finalized.sql` |
| Webhook + stdout sinks with kind/program/ix routing | `src/pipeline/sink.rs` (`EventSink` trait, two impls) |
| SQLite or Postgres | `EventStore` |
| Prometheus `/metrics` | `src/pipeline/metrics.rs` |
| REST `/events` + WS `/stream` + gRPC export | `serve.rs`, `grpc_export.rs` |
| `parser fetch` / `parser watch` (Anchor on-chain IDL, incl. pre-0.30) | `idl_fetch.rs` (stamps `address`), `model.rs` (derives `sha256("global:<fn>")[..8]`) |
| Commitment | `Confirmed` only (`ingest.rs` `SubscribeRequest`) |

## Steal map

| Gap in this indexer | Shipstern already has | Local path |
|---------------------|-----------------------|------------|
| Native / SPL / Token-2022 program decoders | Built-in `AccountParser` + `InstructionParser` | `crates/spl-token-parser`, `crates/spl-token-extensions-parser` |
| BPF loader / stake pool | Built-in parsers | `crates/bpf-loader-parser`, `crates/stake-pool-parser` |
| Non-Anchor programs with no IDL | `include_shipstern_parser!("idl.json")` | `crates/proc-macro` |
| Metaplex | Fixture IDL + generated parser tests | `tests/idls/metaplex.json`, `tests/proc-macro/tests/metaplex.rs` |
| CPI tree + instruction path | `InstructionUpdate.inner` + `Path` | `crates/core/src/instruction.rs` |
| `emit!` / program logs | Log attribution onto each ix | `crates/core/src/log_messages.rs`, `tests/proc-macro-events` |
| Account-state decode | `Runtime::account(Pipeline::new(AccountParser, …))` | every parser crate + `examples/idl-parser` |
| Pluggable handlers / custom sinks | `Handler<T, R>` on `Pipeline` | `crates/runtime/src/handler.rs` |
| Kafka / schema registry | `shipstern-kafka-sink` | `crates/kafka-sink` |
| Slot/block commit ordering | `BlockMachineCoordinator` over `yellowstone_block_machine` | `crates/block-coordinator` |
| Prometheus scrape compose | `docker-compose.yaml` + `prometheus-config.yaml` | repo root; `examples/prometheus` |
| Alternate sources (fumarole, jetstreamer, snapshot) | official source crates | `crates/*-source` |

Do not copy shipstern's lack of a queryable event store. That is this product's edge.

---

## Decoding coverage

### Native programs and full SPL / Token-2022 / Metaplex

`IdlRouter` only loads JSON IDLs and matches an 8-byte discriminator. Native programs and most SPL programs are not Anchor.

`track_tokens` is not a Token decoder. It is two instruction tags (`3` transfer, `12` transfer-checked). Mint, account, freeze, and Token-2022 extensions are absent. System, BPF loader, and Metaplex have no parsers.

**Steal:** `shipstern-spl-token-parser`, `shipstern-spl-token-extensions-parser` (extensions under `crates/spl-token-extensions-parser/src/extensions/`), `shipstern-bpf-loader-parser`, Metaplex via `include_shipstern_parser!`. Wire as built-in routers next to `IdlRouter`, not as JSON IDLs.

### CPI tree

Done in part: each event stores `ix_path` (for example `0.2.1`), built from
stack heights, the same for geyser and RPC backfill. The parent of an inner
instruction is its path prefix. Still missing: log ranges per instruction.

**Steal:** `InstructionUpdate { inner, path, log_range }` in `crates/core/src/instruction.rs`.

### Event / log (`emit!`) decode

`IndexEvent` has `ProgramIx`, `TokenTransfer`, `Slot`, `Gap`, `IdlDrift`. No `ProgramEvent`. Logs are dropped. Anchor events and `Program data:` payloads are where swaps, fills, and pool updates usually live.

**Steal:** `crates/core/src/log_messages.rs` (structural invoke/close classification, not substring matching) plus `tests/proc-macro-events` (CPI event envelopes, padded envelopes, Jupiter log-scope). Add an `IndexEvent::ProgramEvent` and decode `idl.events` the same way instructions are decoded.

### Account-state decode

Geyser account filters exist (`accounts_include`). `decode_update` only handles `Slot` and `Transaction`. Account snapshots are never decoded against the IDL.

**Steal:** every shipstern parser crate ships `AccountParser` beside `InstructionParser`. Subscribe to accounts, decode, store as `kind=account_snapshot` (or a typed accounts table).

---

## Data model and querying

### Flat `/events` only

`GET /events` filters `limit/offset/since_slot/until_slot/type/program/signature`. Payload is JSON in one table. No aggregation, no joins, no per-program typed tables, no GraphQL, no materialized views.

Keep the flat log as bronze. Add gold tables later (for example `token_transfers`, `program_events`) when decoders emit stable types. GraphQL is optional and last.

### Reorg / commitment

Done: dual cursor (confirmed `slot`, `finalized_slot`), parent-chain walk on
each finalized update, and delete of events from orphaned slots (dead slot
rows go at finalization too). See
the [runbook](./operations-runbook.md#cursor-semantics).

### Replay idempotency

Done: events upsert on `(signature, ix_path, kind)`, so geyser replay and RPC
backfill do not add duplicate rows. Rows from before migration 005 carry
`ix_path` `legacy:N` and do not match new paths.

---

## Sinks and integrations

`EventSink` exists. `build_sinks` only accepts `stdout` and `webhook`. Unknown `kind` is an error.

Missing: Kafka / NATS / Redis streams, S3/parquet, a first-class way to register a custom handler without forking `sink.rs`.

**Steal:**

- Trait: shipstern `Handler<T, R>` (`crates/runtime/src/handler.rs`). Expose `EventSink` as a public crate so users implement it.
- Kafka: `crates/kafka-sink` (topics, schema registry, transactional producer, slot-commit events). Start with a Kafka sink kind in `index.yaml`; parquet can wait.

---

## Operations

- `/metrics` is exported. Compose does not run Prometheus or Grafana. Shipstern's `docker-compose.yaml` runs Prometheus + pushgateway; still no Grafana dashboards or alert rules there either. Add a scrape job and a small dashboard (lag, decode errors, sink failures, gap count).
- Single process. No shard-by-program, no slot-range workers, no coordinated backfill.
- Ingest channel is `mpsc` 4096. Sinks run off the ingest loop on a bounded queue (1024); when it is full, live and backfilled events skip the sinks (they are still stored) and a warning is logged. `indexer_lag_slots` exists.

Still missing: a `sink_queue_depth` gauge and a sink-drop counter.

---

## Developer experience

gRPC + REST + WS exist. Consumers still hand-roll clients.

- No TypeScript / Python SDK.
- REST has no OpenAPI spec. gRPC has `src/proto/index_export.proto` only.

Generate OpenAPI from the Axum routes (or a hand-written spec next to the proto). `protoc` TS/Python stubs from that proto. SDKs after the event schema stabilizes (path, events, account snapshots).

---

## Prioritized roadmap

Biggest levers: decoding breadth, reorg-safe correctness, more sinks.

| P | Item | Why first | Shipstern borrow |
|---|------|-----------|------------------|
| P0 | Path-based ix identity + upsert | Done: `ix_path` unique key, CPI stack-height paths | `Path` on `InstructionUpdate` |
| P0 | Finalized cursor + orphan rollback | Done: dual cursor, parent walk, delete orphans | `block-coordinator` / commit-at |
| P1 | `ProgramEvent` + log/`emit!` decode | Most useful protocol data | `log_messages.rs`, proc-macro-events |
| P1 | Keep CPI tree (path + parent) | Unlocks P0 + events | `inner` + `Path` |
| P1 | Full Token + Token-2022 parsers | `track_tokens` is transfer-only | `spl-token-parser`, `spl-token-extensions-parser` |
| P1 | Account snapshot decode | Filters already exist | `AccountParser` |
| P2 | Kafka sink + public `EventSink` | Downstream consumers | `kafka-sink`, `Handler` |
| P2 | Built-in System / BPF loader / Metaplex | Non-Anchor coverage | bpf-loader parser, Codama macro |
| P3 | Prometheus compose + Grafana lag dashboard | Ops | shipstern compose (extend with Grafana) |
| P3 | Bounded ingest/sink queues + lag gauges | Backpressure | runtime metrics counters |
| P4 | OpenAPI + proto-generated TS client | DX | none; this is the product shell |
| P5 | Typed/gold tables or GraphQL | Query power users | none; bronze JSON stays |
| P5 | Horizontal shard / multi-worker backfill | Scale | fumarole / jetstreamer sources if needed |

P0 is correctness. Do not add Kafka or GraphQL on a store that cannot replay or survive a fork.

## Out of scope (for now)

- Vendoring shipstern, carbon, or richat.
- Replacing SQLite/Postgres with an event log of our own.
- Multi-chain.
- Changing the config-driven `index.yaml` model from [plan.md](./plan.md).
