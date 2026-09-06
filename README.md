# Solana Typed-Event Indexer

Yellowstone geyser → IDL decode → SQLite/Postgres → HTTP/WebSocket live view.

## Quick start

```bash
git clone https://github.com/<org>/Solana-gRPC_indexer-Rust.git
cd Solana-gRPC_indexer-Rust
cp .env.example .env          # set YELLOWSTONE_ENDPOINT, YELLOWSTONE_AUTH_TOKEN
cp index.example.yaml index.yaml
cargo run -- run --config index.yaml
open http://127.0.0.1:8080
```

Full setup (IDL registration, Docker, validation): **[docs/quickstart.md](docs/quickstart.md)**

`idl-drift` is vendored under `vendor/idl-drift` — clone and `cargo build` work standalone with no sibling repo.

## Documentation

| Guide | Contents |
|-------|----------|
| [Quickstart](docs/quickstart.md) | Clone → `.env` → `index.yaml` → run |
| [Config reference](docs/config-reference.md) | Every `index.yaml` field + env overrides |
| [Operations runbook](docs/operations-runbook.md) | Cursor, gaps, backfill, IDL drift, troubleshooting |

## Primary commands

| Command | Purpose |
|---------|---------|
| `index run` | Start indexer (geyser → decode → store → serve) |
| `index parser new --idl <file>` | Register a program IDL (idl-drift gated) |
| `index parser list` | List loaded program IDs |
| `index parser diff <old> <new>` | Diff two IDLs |
| `index parser fetch --program-id <addr>` | Fetch Anchor IDL from chain |
| `index parser watch` | Poll chain for IDL drift and update local copies |
| `index config validate` | Validate `index.yaml` |
| `index demo` | Serve stored events only (no geyser) |
| `index legacy …` | Old RPC/track/cache commands (hidden) |

## HTTP API

| Endpoint | Description |
|----------|-------------|
| `GET /` | Live event table (HTML) |
| `WS /stream` | Real-time JSON events |
| `GET /events` | Query historical events (`limit`, `offset`, `since_slot`, `until_slot`, `type`, `program`, `signature`) |
| `GET /cursor` | Last committed slot |
| `GET /gaps` | Slot gaps (`open_only=true` default) |
| `GET /health` | Health check |
| `GET /stats` | Live counters |
| `GET /metrics` | Prometheus metrics |

Set `auth.api_keys` in `index.yaml` or `INDEX_API_KEYS` env to protect `/events`, `/stream`, `/cursor`, `/gaps`, `/stats`. `/`, `/health`, and `/metrics` stay public.

### gRPC export

When `grpc.bind` is set in config or `--grpc-bind 0.0.0.0:50051` is passed to `run`:

- `StreamEvents` — live server-stream of parsed events (optional `kind` / `program` filter)
- `GetEvents` — historical query (same filters as REST)

Proto: `src/proto/index_export.proto`

## Storage

| URL | Backend |
|-----|---------|
| `sqlite:./index.db` | SQLite (default) |
| `postgresql://...` | Postgres |

## Config (`index.yaml`)

See [docs/config-reference.md](docs/config-reference.md) and `index.example.yaml`. CLI flags override config values.

## Local smoke

No geyser key required: `./scripts/smoke-local.sh` starts demo on `testdata/smoke.db` and curls `/health`, `/events`, `/metrics`.

## Demo mode (no geyser)

Serve an existing database read-only (good for sharing a snapshot without a geyser key):

```bash
cargo run -- demo --db sqlite:./index.db
```

## Docker

```bash
cp .env.example .env
docker compose up --build
```

See [docs/quickstart.md](docs/quickstart.md#7-docker-alternative) for optional `index.yaml` mounting.

## What works

- Anchor IDL decode (instruction name, args, named accounts)
- Configurable geyser filters (`track_tokens`, `track_failed`, `accounts_include`)
- Cursor resume + gap detection + RPC backfill (`rpc_url`)
- idl-drift safety on `parser new` / `parser watch`
- Webhook/stdout sinks with kind/program/instruction routing
- Postgres or SQLite store
- Prometheus metrics on `/metrics`
- gRPC event export
- `parser fetch` / `parser watch` for on-chain IDL
- TUI: `index run --tui`

## Legacy commands

RPC slot polling, wallet watch, cache, webhooks, etc. live under:

```bash
cargo run -- legacy track slots
cargo run -- legacy yellowstone --endpoint … --auth-token …
```

## License

MIT
