# Config reference (`index.yaml`)

The indexer is driven by a single YAML file. Copy `index.example.yaml` to `index.yaml` and adjust values. CLI flags passed to `index run` override config file values where both apply.

Unknown keys are errors in every section (top level, `geyser`, `store`,
`serve`, `sinks`, `auth`, `grpc`). A misspelled `auth:` or `api_keys:` therefore
stops startup and cannot turn API auth off.

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
  bind: "127.0.0.1:8080"

programs:
  - "./idls/pumpFun.json"

wallets: []

rpc_url: "https://api.mainnet-beta.solana.com"

sinks:
  - kind: stdout

# Optional sections:
# grpc:
#   bind: "127.0.0.1:50051"
# auth:
#   api_keys: ["dev-key-change-me"]
```

---

## `geyser`

Yellowstone geyser connection and subscription filters.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `endpoint` | string | `""` | Geyser gRPC URL. Required unless set via `YELLOWSTONE_ENDPOINT`. A `host:port` with no scheme means `https://`, unless the host is loopback (`localhost`, `127.0.0.1`, `[::1]`), which means `http://`. A remote plaintext endpoint needs an explicit `http://`. |
| `auth_token` | string | `""` | Provider `x-token`. Override with `YELLOWSTONE_AUTH_TOKEN`. |
| `track_failed` | bool | `false` | When `true`, include failed transactions in the geyser subscription and in RPC backfill. Their `program_ix` events carry `"failed": true`; their token-program instructions never become `token_transfer` events. Default excludes failed txs. |
| `track_tokens` | bool | `false` | When `true`, subscribe to SPL Token and Token-2022 program traffic. **High volume** — enable only when you need token transfer events. |
| `accounts_include` | string[] | `[]` | Additional account pubkeys passed to the geyser `account_include` filter. Program IDs from loaded IDLs are always included. |
| `replay` | `auto` / `on` / `off` | `auto` | Whether to send `from_slot` on reconnect. `auto` calls `SubscribeReplayInfo` first (Richat often yes, many Triton gateways no). |
| `ping_secs` | u64 | `10` | Bidirectional subscribe ping so load balancers do not drop idle streams. `0` disables client pings. |
| `compression` | `zstd` / `gzip` | `zstd` | Payload compression. `zstd` for Triton/Richat on `:443`. `gzip` is the original Yellowstone default. |

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
| `url` | string | `sqlite:./index.db` | Database connection URL. SQLite path or Postgres DSN. Override with `DATABASE_URL`. Any other scheme (for example `mysql://`) is an error at startup. |

| URL pattern | Backend |
|-------------|---------|
| `sqlite:./index.db` | SQLite file (created on first run) |
| `postgresql://user:pass@host:5432/db` | PostgreSQL |

SQLite runs in WAL mode, so `index.db-wal` and `index.db-shm` appear next to
the database file. Keep them with it.

Migrations from `migrations/<backend>/` run at startup on both new and existing
databases. `events` carries indexes on `slot`, `kind`, `program`, and
`(signature, ix_path, kind)` so each `/events` filter seeks rather than scans;
see [operations-runbook.md](./operations-runbook.md#query-indexes).

---

## `serve`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `bind` | string | `127.0.0.1:8080` | HTTP/WebSocket listen address. Override with `index run --bind`. A non-loopback bind with no API keys logs a warning: all data is then open to the network. The server has no TLS, so a non-loopback bind also needs a TLS reverse proxy that sets `X-Forwarded-Host`. With no API keys, on any bind, every route returns `403` unless `Host` is `localhost`, `*.localhost` or an IP literal (DNS-rebinding guard). The server speaks HTTP/1.1 only. |

---

## `programs`

| Type | Description |
|------|-------------|
| `string[]` (paths) | IDL JSON file paths, one Anchor program per file. The parent directory becomes the IDL load path at runtime (default `./idls`). |

At least one IDL is recommended for program instruction decoding. Token transfers decode without IDLs when `track_tokens` is enabled.

Only the first entry's **parent directory** is used: the router then loads every
`*.json` in it. Listing one file out of a shared directory does not narrow the
load — keep per-deployment IDL sets in separate directories.

Three IDL formats load:

| Format | Detected by | Notes |
|--------|-------------|-------|
| Codama `rootNode` | `"kind": "rootNode"` | What `pumpFun.json` ships as |
| Anchor 0.30+ | `address` + per-instruction `discriminator` | Used as-is |
| Anchor pre-0.30 | neither of the above | `parser fetch` sets `address`; discriminators are derived when the IDL is parsed. See the [runbook](./operations-runbook.md#fetching-from-chain) |

An IDL with an empty or invalid `address`, or a program name longer than 64
bytes or with control or bidi characters, is skipped at load with the warning
`skip IDL file`.

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

Any bad entry (unknown `kind`, webhook without an `http`/`https` `url`) stops
startup, and `config validate` fails the same way. The error lists each bad
entry, for example `sinks[1]: unknown sink kind: kafka`.

Webhooks use a 2s connect timeout and a 5s request timeout, try each event up
to 3 times, and do not follow redirects. Under `--tui`, `stdout` sinks are off.

---

## `auth`

Optional API key protection for sensitive HTTP routes.

| Field | Type | Description |
|-------|------|-------------|
| `api_keys` | string[] | Valid keys. Clients send `X-API-Key: <key>` or `Authorization: Bearer <key>`. A list with no non-empty key is an error. |

Protected: `/events`, `/stream`, `/cursor`, `/gaps`, `/stats`, and gRPC export when keys are configured.

Public: `/`, `/health`, `/metrics`.

WebSocket `/stream`: browsers cannot set headers, so they send the key as a
subprotocol, `Sec-WebSocket-Protocol: index-key, <key>`, and the server echoes
`index-key`. A key in the query string (`?api_key=`) is not accepted. The live
view at `/` reads the key from the page URL: open `http://127.0.0.1:8080/#key=<key>`.
For this, a key must use token characters only (letters, digits, `-._~`).

Override with comma-separated `INDEX_API_KEYS` env var (replaces YAML `auth` block when non-empty). An empty `INDEX_API_KEYS` does not override `index.yaml`. A value with no key in it (for example `,`) is an error.

---

## `grpc`

Optional gRPC export for downstream consumers.

| Field | Type | Description |
|-------|------|-------------|
| `bind` | string | Listen address (e.g. `127.0.0.1:50051`). Override with `index run --grpc-bind`. A non-loopback bind with no API keys logs a warning. No TLS: put a TLS proxy in front of a non-loopback bind. |

Services (see `src/proto/index_export.proto`):

- `StreamEvents` — live server-stream of parsed events
- `GetEvents` — historical query. Fields: `limit`, `since_slot`, `kind`, `program`, `offset` (5), `until_slot` (6), `signature` (7). An `offset` above 10000, or a slot above `i64::MAX`, gives `InvalidArgument`.

With API keys set, every call needs the key in metadata: `x-api-key: <key>` or
`authorization: Bearer <key>`. Otherwise the call fails with `Unauthenticated`.

At most 256 `StreamEvents` streams per process (`RESOURCE_EXHAUSTED`) and 64
streams per connection. Other limits are in the
[runbook](./operations-runbook.md#server-limits).

---

## Environment variable overrides

Environment variables are loaded from `.env` (via `dotenvy`) and override YAML values when non-empty.

| Env var | Overrides | Notes |
|---------|-----------|-------|
| `YELLOWSTONE_ENDPOINT` | `geyser.endpoint` | Required if not in YAML |
| `YELLOWSTONE_AUTH_TOKEN` | `geyser.auth_token` | |
| `DATABASE_URL` | `store.url` | The env of `index run --db`, so it wins over YAML |
| `SOLANA_RPC_URL` | `rpc_url` | Used by backfill and `parser fetch` / `parser watch` |
| `INDEX_API_KEYS` | `auth.api_keys` | Comma-separated; replaces entire auth block. Empty counts as unset; a value with no key in it (for example `,`) is an error |

No env overrides exist for `track_failed`, `track_tokens`, `accounts_include`, `sinks`, `grpc`, or `programs` — set those in YAML.

---

## CLI overrides (`index run`)

A flag given on the command line (or through its env var) wins over YAML, also
when its value equals the default. YAML wins only over a flag that was not given.

| Flag | Overrides |
|------|-----------|
| `--config FILE` | Load base config from YAML |
| `--endpoint` | `geyser.endpoint` |
| `--auth-token` | `geyser.auth_token`. Prefer `YELLOWSTONE_AUTH_TOKEN`: arguments are visible in `ps` |
| `--db` | `store.url` (default `sqlite:./index.db`) |
| `--bind` | `serve.bind` (default `127.0.0.1:8080`) |
| `--idl-dir` | Parent of first `programs` entry (default `./idls`) |
| `--wallet` (repeatable) | `wallets` |
| `--grpc-bind` | `grpc.bind` |
| `--tui` | Terminal UI instead of log-only mode. Logs go to `./index.log`, and `stdout` sinks are off |

`index --verbose run ...` logs at debug, but keeps `solana_rpc_client` at info,
because its debug lines hold the full RPC URL.
