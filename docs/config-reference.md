# Config reference (`index.yaml`)

The indexer is driven by a single YAML file. Copy `index.example.yaml` to `index.yaml` and adjust values. CLI flags passed to `index run` override config file values where both apply.

Validate before running:

```bash
cargo run -- config validate --file index.yaml
```

## Top-level structure

```yaml
geyser:
  endpoint: ""
  auth_token: ""
  track_failed: false
  track_tokens: false
  accounts_include: []

store:
  url: "sqlite:./index.db"

serve:
  bind: "0.0.0.0:8080"

programs:
  - "./idls/pump_fun.json"

wallets: []

rpc_url: "https://api.mainnet-beta.solana.com"

sinks:
  - kind: stdout

# Optional sections:
# grpc:
#   bind: "0.0.0.0:50051"
# auth:
#   api_keys: ["dev-key-change-me"]
```

---

## `geyser`

Yellowstone geyser connection and subscription filters.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `endpoint` | string | `""` | Geyser gRPC URL. Required unless set via `YELLOWSTONE_ENDPOINT`. |
| `auth_token` | string | `""` | Provider `x-token`. Override with `YELLOWSTONE_AUTH_TOKEN`. |
| `track_failed` | bool | `false` | When `true`, include failed transactions in the geyser subscription. Default excludes failed txs. |
| `track_tokens` | bool | `false` | When `true`, subscribe to SPL Token and Token-2022 program traffic. **High volume** — enable only when you need token transfer events. |
| `accounts_include` | string[] | `[]` | Additional account pubkeys passed to the geyser `account_include` filter. Program IDs from loaded IDLs are always included. |

### Subscription behavior

The geyser filter is built from:

1. `wallets` (if non-empty) — wallet-centric filtering
2. `geyser.accounts_include` — extra pubkeys
3. Program IDs discovered from IDL files in `programs`
4. Token programs — only when `track_tokens: true`

Votes are always excluded. Failed transactions are included only when `track_failed: true`.

---

## `store`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `url` | string | `sqlite:./index.db` | Database connection URL. SQLite path or Postgres DSN. Override with `DATABASE_URL`. |

| URL pattern | Backend |
|-------------|---------|
| `sqlite:./index.db` | SQLite file (created on first run) |
| `postgresql://user:pass@host:5432/db` | PostgreSQL |

---

## `serve`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `bind` | string | `0.0.0.0:8080` | HTTP/WebSocket listen address. Override with `index run --bind`. |

---

## `programs`

| Type | Description |
|------|-------------|
| `string[]` (paths) | IDL JSON file paths, one Anchor program per file. The parent directory becomes the IDL load path at runtime (default `./idls`). |

At least one IDL is recommended for program instruction decoding. Token transfers decode without IDLs when `track_tokens` is enabled.

---

## `wallets`

| Type | Default | Description |
|------|---------|-------------|
| `string[]` | `[]` | Wallet addresses for geyser `account_include`. When set, replaces the default token-program subscription (use `track_tokens` or `accounts_include` for additional coverage). Override with repeated `index run --wallet <ADDR>`. |

---

## `rpc_url`

| Type | Default | Description |
|------|---------|-------------|
| `string` (optional) | none | Solana JSON-RPC endpoint for gap detection backfill via `getBlock`. Without this (and without `SOLANA_RPC_URL`), gaps are recorded but not repaired. |

---

## `sinks`

Optional event fan-out destinations. Events are always stored in the database; sinks receive a copy after successful insert.

Each sink entry:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `kind` | string | yes | `stdout` or `webhook` |
| `url` | string | webhook only | HTTP POST target for webhook sinks |
| `types` | string[] | no | Filter by event kind: `slot`, `token_transfer`, `program_ix`, `gap`, `idl_drift` |
| `programs` | string[] | no | Filter by program name (IDL metadata name or token program id) |
| `instructions` | string[] | no | Filter `program_ix` events by instruction name (e.g. `buy`, `swap`) |

### Examples

```yaml
sinks:
  - kind: stdout

  - kind: webhook
    url: "https://hooks.example.com/events"
    types: ["program_ix"]
    programs: ["pump"]
    instructions: ["buy", "sell"]
```

Empty filter lists match all events for that dimension. Combined filters use AND logic across `types`, `programs`, and `instructions`.

---

## `auth`

Optional API key protection for sensitive HTTP routes.

| Field | Type | Description |
|-------|------|-------------|
| `api_keys` | string[] | Valid keys. Clients send `X-API-Key: <key>` or `Authorization: Bearer <key>`. |

Protected: `/events`, `/stream`, `/cursor`, `/gaps`, `/stats`, and gRPC export when keys are configured.

Public: `/`, `/health`, `/metrics`.

Override with comma-separated `INDEX_API_KEYS` env var (replaces YAML `auth` block when non-empty).

---

## `grpc`

Optional gRPC export for downstream consumers.

| Field | Type | Description |
|-------|------|-------------|
| `bind` | string | Listen address (e.g. `0.0.0.0:50051`). Override with `index run --grpc-bind`. |

Services (see `src/proto/index_export.proto`):

- `StreamEvents` — live server-stream of parsed events
- `GetEvents` — historical query

---

## Environment variable overrides

Environment variables are loaded from `.env` (via `dotenvy`) and override YAML values when non-empty.

| Env var | Overrides | Notes |
|---------|-----------|-------|
| `YELLOWSTONE_ENDPOINT` | `geyser.endpoint` | Required if not in YAML |
| `YELLOWSTONE_AUTH_TOKEN` | `geyser.auth_token` | |
| `DATABASE_URL` | `store.url` | Also used by `index run --db` default |
| `SOLANA_RPC_URL` | `rpc_url` | Used by backfill and `parser fetch` / `parser watch` |
| `INDEX_API_KEYS` | `auth.api_keys` | Comma-separated; replaces entire auth block |

No env overrides exist for `track_failed`, `track_tokens`, `accounts_include`, `sinks`, `grpc`, or `programs` — set those in YAML.

---

## CLI overrides (`index run`)

| Flag | Overrides |
|------|-----------|
| `--config FILE` | Load base config from YAML |
| `--endpoint` | `geyser.endpoint` |
| `--auth-token` | `geyser.auth_token` |
| `--db` | `store.url` (when not default) |
| `--bind` | `serve.bind` (when not default) |
| `--idl-dir` | Parent of first `programs` entry (when not default `./idls`) |
| `--wallet` (repeatable) | `wallets` |
| `--grpc-bind` | `grpc.bind` |
| `--tui` | Terminal UI instead of log-only mode |
