# Product gaps (vs shipstern)

Feature gaps only. Not repo hygiene.

Sourced from this repo's README **What works** list plus the command / HTTP / gRPC tables, then checked against `src/pipeline/`. Each gap is mapped to what already exists in local shipstern (`~/oss/vixen/shipstern`, upstream `solana-rpc/shipstern`).

The geyser → decode → store → serve pipeline is the product. Shipstern is the parser/runtime/sink library. Steal parsers, CPI trees, handlers, and Kafka. Do not vendor the framework. Keep this repo the one-binary shell: cursor, gaps, typed store, HTTP/WS, TUI.

## What already works (verified)

| Claim | Where |
|-------|-------|
| Anchor IDL decode (name, args, named accounts) | `src/pipeline/idl.rs` (`IdlRouter`, 8-byte discriminator) |
| Opt-in SPL Token / Token-2022 **transfers only** | `src/pipeline/token.rs` (`TRANSFER` / `TRANSFER_CHECKED`) |
| Inner instructions flattened into the same stream | `src/pipeline/ingest.rs` (`meta.inner_instructions`) |
| Geyser filters | `track_tokens`, `track_failed`, `accounts_include` |
| Cursor resume + gap detect + RPC backfill | `store.rs`, `run.rs`, `backfill.rs` |
| Store unique key | `UNIQUE(signature, ix_index, kind)` in `migrations/*/001_events_pipeline.sql` |
| Webhook + stdout sinks with kind/program/ix routing | `src/pipeline/sink.rs` (`EventSink` trait, two impls) |
| SQLite or Postgres | `EventStore` |
| Prometheus `/metrics` | `src/pipeline/metrics.rs` |
| REST `/events` + WS `/stream` + gRPC export | `serve.rs`, `grpc_export.rs` |
| `parser fetch` / `parser watch` (Anchor on-chain IDL) | `idl_fetch.rs`, `idl_watch.rs` |
| Commitment | `Confirmed` only (`ingest.rs` `SubscribeRequest`) |

## Steal map

| Gap in this indexer | Shipstern already has | Local path |
|---------------------|-----------------------|------------|
| Native / SPL / Token-2022 program decoders | Built-in `AccountParser` + `InstructionParser` | `crates/spl-token-parser`, `crates/spl-token-extensions-parser` |
| BPF loader / stake pool | Built-in parsers | `crates/bpf-loader-parser`, `crates/stake-pool-parser` |
| Codama / non-Anchor IDLs | `include_shipstern_parser!("idl.json")` | `crates/proc-macro` |
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

### CPI tree, not a flat list

Inner instructions are ingested, then flattened. `ix_index` is a running counter across outer + inner. There is no tree, no `Path`, no parent ix, no stack height.

That loses which program invoked which, and it makes geyser vs RPC numbering diverge (see idempotency below).

**Steal:** `InstructionUpdate { inner, path, log_range }` in `crates/core/src/instruction.rs`. Persist `path` (for example `0.2.1`) instead of a flat index.

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

Subscribe is `CommitmentLevel::Confirmed` only. Cursor is last slot seen. There is no `finalized` vs `confirmed`, no orphan rollback, no parent-slot walk on fork.

Confirmed slots can be skipped. Events from a losing fork stay in the store.

**Steal:** `crates/block-coordinator` (`BlockMachineCoordinator` + `AccountCommitAt`). Minimum viable here: dual cursor (`confirmed_slot`, `finalized_slot`) and delete-or-tombstone events whose slot is not an ancestor of the new finalized head.

### Replay idempotency

The unique key is `(signature, ix_index, kind)`. That is not enough:

- Flattened inner-ix `ix_index` is not stable across geyser vs `getTransaction` backfill.
- Re-decode after IDL watch can change `kind` or payload without a new key.
- Gap fill can insert the same logical ix under a different index.

**Need:** stable identity, for example `(signature, ix_path, kind)` plus `ON CONFLICT` upsert. Document that backfill is idempotent in the runbook once the key is path-based.

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
- Ingest channel is `mpsc` 4096. Sinks warn-and-continue on error. No geyser subscribe backpressure, no sink queue bounds, no drop/sample policy.

Minimum: bounded sink queue + `lag_slots` / `sink_queue_depth` gauges + documented behavior when full (block vs drop).

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
| P0 | Path-based ix identity + upsert | Makes backfill and CPI work correct | `Path` on `InstructionUpdate` |
| P0 | Finalized cursor + orphan rollback | Confirmed-only is a correctness hole | `block-coordinator` / commit-at |
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
