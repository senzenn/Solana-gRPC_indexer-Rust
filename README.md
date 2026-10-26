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
| [Product gaps](docs/feature-gaps.md) | Feature roadmap vs shipstern (decode, reorg, sinks) |

## Primary commands

| Command | Purpose |
|---------|---------|
| `index run` | Start indexer (geyser → decode → store → serve) |
| `index parser new --idl <file>` | Register a program IDL (idl-drift gated) |
| `index parser list` | List loaded program IDs |
| `index parser diff <old> <new>` | Diff two IDLs |
| `index parser fetch --program-id <addr>` | Fetch Anchor IDL from chain (stamps the program id, derives legacy discriminators) |
| `index parser watch` | Poll chain for IDL drift and update local copies |
| `index config validate` | Validate `index.yaml` |
| `index demo` | Serve stored events only (no geyser) |
| `index probe` | `GetVersion` + replay check against a Triton/Richat gRPC URL |

## HTTP API

| Endpoint | Description |
|----------|-------------|
| `GET /` | Live event table (HTML) |
| `WS /stream` | Real-time JSON events |
| `GET /events` | Query historical events (`limit`, `offset`, `since_slot`, `until_slot`, `type`, `program`, `signature`) |
| `GET /cursor` | Confirmed + finalized slot |
| `GET /gaps` | Slot gaps (`open_only=true` default) |
| `GET /health` | Health check, no key: `ok`, `slot`, `finalized_slot`, `events_total`, `open_gaps` |
| `GET /stats` | Live counters |
| `GET /metrics` | Prometheus metrics, no key |

The same query routes are also served under `/v1` (`/v1/events`, `/v1/cursor`, `/v1/gaps`, `/v1/stats`, `WS /v1/stream`, `/v1/health`).

Set `auth.api_keys` in `index.yaml` or `INDEX_API_KEYS` env to protect `/events`, `/stream`, `/cursor`, `/gaps`, `/stats` and the `/v1` copies. `/`, `/health`, `/v1/health`, and `/metrics` stay public.

The default bind is `127.0.0.1:8080`; a non-loopback bind with no API keys logs a warning. Neither the HTTP server nor the gRPC export has TLS. For any non-loopback bind, set API keys and put a TLS reverse proxy in front that sets `X-Forwarded-Host`. WebSocket clients send the key as `Sec-WebSocket-Protocol: index-key, <key>` (not `?api_key=`). The live view reads it from the page URL: `http://127.0.0.1:8080/#key=<key>`. Keys must use only letters, digits and `-._~`.

With no API keys, on any bind, every route returns `403` unless `Host` is `localhost`, `*.localhost` or an IP literal (DNS-rebinding guard). A keyless server behind a reverse proxy that sends its domain name as `Host` gets `403`: set keys, or make the proxy send an IP literal such as `Host: 127.0.0.1`. The HTTP API serves HTTP/1.1 only.

`/events` answers `400` for an `offset` above 10000 or a slot above `i64::MAX`, `503 {"error":"busy"}` when the API's DB connections stay busy for 5s, and a generic `500` for store errors. Connection and stream limits: [runbook](docs/operations-runbook.md#server-limits).

### gRPC export

When `grpc.bind` is set in config or `--grpc-bind 127.0.0.1:50051` is passed to `run`:

- `StreamEvents` — live server-stream of parsed events (optional `kind` / `program` filter)
- `GetEvents` — historical query (same filters as REST: `offset` max 10000, `until_slot`, `signature`; out-of-range values give `InvalidArgument`)

With API keys set, each call needs metadata `x-api-key: <key>` or `authorization: Bearer <key>`.

Proto: `src/proto/index_export.proto`

## Storage

| URL | Backend |
|-----|---------|
| `sqlite:./index.db` | SQLite (default) |
| `postgresql://...` | Postgres |

Other URL schemes (for example `mysql://`) are rejected. SQLite runs in WAL mode, so `-wal` and `-shm` files appear next to the database.

Migrations under `migrations/<backend>/` run on startup. `events` is indexed on
`slot`, `kind`, `program`, and `(signature, ix_path, kind)`, which covers every
`/events` filter.

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

Compose publishes on `127.0.0.1` only and passes the geyser `x-token` from `.env` as environment. Read-only demo: `docker compose --profile demo up --build demo` on `127.0.0.1:8081`. See [docs/quickstart.md](docs/quickstart.md#7-docker-alternative) for optional `index.yaml` mounting.

## What works

- Anchor IDL decode (instruction name, args, named accounts)
- Compiled shipstern parsers from IDLs in `parsers/` (see [parsers/README.md](parsers/README.md))
- Configurable geyser filters (`track_tokens`, `track_failed`, `accounts_include`)
- CPI-stable `ix_path` identity + upsert (geyser and RPC backfill)
- Confirmed + finalized cursors with orphan-slot rollback
- Cursor resume + gap detection + RPC backfill (`rpc_url`)
- idl-drift safety on `parser new` / `parser watch`
- Webhook/stdout sinks with kind/program/instruction routing
- Postgres or SQLite store
- Prometheus metrics on `/metrics`
- gRPC event export
- `parser fetch` / `parser watch` for on-chain IDL (Codama, Anchor 0.30+, and pre-0.30 legacy)
- TUI: `index run --tui` (logs go to `./index.log`)

## License

MIT
