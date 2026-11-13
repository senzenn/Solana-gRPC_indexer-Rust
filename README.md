# Solana Typed-Event Indexer

Yellowstone geyser -> IDL decode -> SQLite/Postgres -> HTTP, WebSocket and gRPC.

## Quick start

Needs Rust 1.88+, `protoc` on `PATH` (`build.rs` compiles `src/proto`), and a
Yellowstone / Dragon's Mouth gRPC endpoint with its `x-token`.

```bash
git clone https://github.com/senzenn/Solana-gRPC_indexer-Rust.git
cd Solana-gRPC_indexer-Rust
cp .env.example .env             # set YELLOWSTONE_ENDPOINT, YELLOWSTONE_AUTH_TOKEN
cp index.example.yaml index.yaml
cargo run -- probe               # GetVersion + replay check
cargo run -- config validate --file index.yaml
cargo run -- run --config index.yaml
open http://127.0.0.1:8080
```

- Non-empty env vars override YAML. Keep secrets in `.env`: flags show in `ps`.
- `run` with no `--config` takes its settings from env vars, flags and defaults. It has no sinks.
- `--tui` shows a terminal view. Logs then go to `./index.log`.
- At start, the indexer resumes from the stored cursor when `replay` allows it.
  Otherwise it starts at the tip, and the down time becomes a gap. See
  [cursor semantics](docs/operations-runbook.md#cursor-semantics).

## Commands

| Command | Purpose |
|---------|---------|
| `index run` | Geyser -> decode -> store -> serve |
| `index probe` | `GetVersion` + replay check of the geyser endpoint |
| `index config validate` | Validate `index.yaml` |
| `index demo` | Serve stored events with no geyser |
| `index parser new --idl <file>` | Install an IDL through the idl-drift gate |
| `index parser fetch --program-id <addr>` | Fetch an Anchor IDL from chain and install it |
| `index parser watch` | Poll chain for IDL drift and update local copies |
| `index parser list` | List loaded program ids |
| `index parser diff <old> <new>` | Diff two IDLs (exit 1 on breaking drift) |

## IDLs

- `idls/` holds the IDLs loaded at run time: Codama, Anchor 0.30+ and pre-0.30
  Anchor. `parser new` and `parser fetch` write `idls/<program-id>.json`.
  `--force` accepts breaking changes.
- `parsers/` holds IDLs compiled into typed parsers at build time. See
  [parsers/README.md](parsers/README.md). A file in `idls/` wins for the same program.
- An IDL with an empty or invalid `address` is skipped with the warning `skip IDL file`.

## HTTP API

| Endpoint | Description |
|----------|-------------|
| `GET /` | Live event table (HTML) |
| `WS /stream` | Live JSON events |
| `GET /events` | History: `limit`, `offset`, `since_slot`, `until_slot`, `type`, `program`, `signature` |
| `GET /cursor` | Confirmed + finalized slot |
| `GET /gaps` | Slot gaps (`open_only=true` default) |
| `GET /stats` | Live counters |
| `GET /health` | `ok`, `slot`, `finalized_slot`, `events_total`, `open_gaps` |
| `GET /metrics` | Prometheus metrics |

The query routes are also served under `/v1`.

Set `auth.api_keys` or `INDEX_API_KEYS` to protect every route except `/`,
`/health`, `/v1/health` and `/metrics`. With no keys, the server accepts only a
`localhost`, `*.localhost` or IP-literal `Host`. See
[`auth`](docs/config-reference.md#auth) and [`serve`](docs/config-reference.md#serve).
Limits: [runbook](docs/operations-runbook.md#server-limits).

### gRPC export

Set `grpc.bind` or pass `--grpc-bind 127.0.0.1:50051`. Proto: `src/proto/index_export.proto`.

- `StreamEvents`: live events, optional `kind` / `program` filter
- `GetEvents`: history, same filters as `/events`

With API keys set, send metadata `x-api-key: <key>` or `authorization: Bearer <key>`.

## Storage

`sqlite:./index.db` (default) or `postgresql://...`. Other schemes are rejected.
Migrations in `migrations/<backend>/` run at startup. Events upsert on
`(signature, ix_path, kind)`, so replay and backfill add no duplicates.

An optional `clickhouse` section copies finalized events to ClickHouse through
a durable outbox. See [`clickhouse`](docs/config-reference.md#clickhouse).

## Docker

```bash
cp .env.example .env
docker compose up --build
```

Compose publishes `127.0.0.1:8080` only and passes `.env` as environment. To
use `./index.yaml`, uncomment its volume line and add `--config /app/index.yaml`
to `command`.

Demo with no geyser: `docker compose --profile demo up --build demo` on `127.0.0.1:8081`.

## Demo mode

```bash
cargo run -- demo --db sqlite:./index.db
```

Demo is not read-only: it runs migrations and creates the file if it is missing.
To share a snapshot, serve a copy of it.

## Local smoke

`./scripts/smoke-local.sh` needs no geyser key. It builds, runs the tests and
clippy, validates `index.example.yaml`, then starts demo and curls `/health` and `/metrics`.

## What works

- Anchor and Codama IDL decode: instruction name, args, named accounts
- Compiled shipstern parsers from `parsers/`
- Geyser filters: `track_tokens`, `track_failed`, `accounts_include`, `wallets`
- CPI-stable `ix_path` identity, the same for geyser and RPC backfill
- Confirmed + finalized cursors with orphan-slot rollback
- Gap detection and RPC backfill
- idl-drift gate on `parser new`, `parser fetch` and `parser watch`
- Webhook and stdout sinks with kind / program / instruction routing
- ClickHouse copy of finalized events
- Prometheus metrics, gRPC export, TUI

## Docs

- [Config reference](docs/config-reference.md): every `index.yaml` field, env and CLI overrides
- [Operations runbook](docs/operations-runbook.md): cursor, gaps, backfill, IDL drift, limits, failures
- [Roadmap](docs/feature-gaps.md): open feature gaps and what to borrow from shipstern

## License

MIT
