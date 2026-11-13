# Roadmap

Open feature gaps, checked against `src/pipeline/`, and what to borrow from
shipstern (`solana-rpc/shipstern`).

This repo is the one-binary shell: cursor, gaps, queryable store, HTTP/WS/gRPC,
TUI. Shipstern is a parser and runtime library with no queryable store. Borrow
its parsers, CPI tree, handlers and Kafka sink. Do not vendor the framework.

## Priorities

Reorg-safe correctness first (P0), then decode breadth (P1), then sinks (P2).

| P | Item | State | Shipstern borrow |
|---|------|-------|------------------|
| P0 | Path-based ix identity + upsert | Done: `ix_path` unique key from CPI stack heights | `Path` on `InstructionUpdate` |
| P0 | Finalized cursor + orphan rollback | Done: dual cursor, parent walk, orphan delete | `block-coordinator` |
| P1 | `ProgramEvent` + `emit!` / log decode | Open. `IndexEvent` has no `ProgramEvent`, and logs are dropped | `crates/core/src/log_messages.rs`, `tests/proc-macro-events` |
| P1 | Log range per instruction | Open. `ix_path` holds the CPI path; the parent is its prefix | `InstructionUpdate.log_range` |
| P1 | Full Token + Token-2022 decode | Open. `track_tokens` decodes only tags `3` (transfer) and `12` (transfer-checked) | `spl-token-parser`, `spl-token-extensions-parser` |
| P1 | Account-state decode | Open. `decode_update` handles only `Slot` and `Transaction` | `AccountParser` in each parser crate |
| P2 | Kafka sink + public `EventSink` | Open. `build_sinks` accepts only `stdout` and `webhook` | `crates/kafka-sink`, `Handler<T, R>` |
| P2 | System / BPF loader / Metaplex decode | Open. Wire as built-in routers next to `IdlRouter` | `bpf-loader-parser`, `include_shipstern_parser!` |
| P3 | Sink queue metrics | Open. Need a `sink_queue_depth` gauge and a sink-drop counter | runtime metrics |
| P3 | Prometheus + Grafana in compose | Open. `/metrics` exists; compose runs no scraper | shipstern `docker-compose.yaml` |
| P4 | OpenAPI spec + TS/Python clients | Open. gRPC has only `src/proto/index_export.proto` | none |
| P5 | Typed tables or GraphQL | Open. `/events` is one flat JSON table | none |
| P5 | Sharded ingest / parallel backfill | Open. One process | fumarole / jetstreamer sources |

Do not add Kafka or GraphQL on a store that cannot replay or survive a fork.

## Out of scope

- Vendoring shipstern, carbon or richat.
- A custom event log in place of SQLite/Postgres.
- Multi-chain.
- Code changes per protocol. Downstream users configure `index.yaml` only.
