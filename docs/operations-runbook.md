# Operations runbook

Day-two operations for the typed-event indexer: cursor semantics, gap repair, IDL drift, health checks, and common failure modes.

## Cursor semantics

The indexer persists a **dual cursor** in the `cursors` table (id `1`):

- `slot`: highest **confirmed** slot
- `finalized_slot`: highest **finalized** slot that survived fork rollback

Confirmed slot updates advance `slot` and run gap detection. Finalized slot updates walk the parent chain and **delete events from orphaned slots** (slots we indexed that are not ancestors of the new finalized tip). If the parent chain is incomplete, nothing is deleted. A dead slot only logs `dead slot`: the stream is at confirmed commitment, so it has no events, and its slot row is removed as an orphan at finalization.

**Slot jumps:** a confirmed, finalized or event slot more than 1,000,000 above
the last confirmed slot is skipped with a warning, as corrupt data. After 32
such confirmed slots in a row, the jump is accepted (a long outage moves the
tip that far too) and the missing range is recorded as a gap.

**Resume on restart:** When `index run` starts, it loads the confirmed cursor (log field `resume_slot`). If [`replay`](./config-reference.md#geyser) allows it, the cursor slot goes to geyser as `from_slot`, and that slot is replayed. The replay is idempotent (see below). If not, the stream starts at the tip, and the down time becomes a gap.

```bash
curl http://127.0.0.1:8080/cursor
# {"slot": 285001234, "finalized_slot": 285001200, "updated_at": "..."}
```

The ingest loop also keeps the last committed confirmed slot in memory. When the geyser stream drops, the client reconnects with that slot as `from_slot`, under the same `replay` rule.

**Triton / Richat:** both speak Yellowstone `geyser.Geyser/Subscribe`. Before relying on `from_slot`, run:

```bash
cargo run -- probe   # reads YELLOWSTONE_ENDPOINT and YELLOWSTONE_AUTH_TOKEN from the env or .env
```

Do not pass secrets as flags: see [CLI overrides](./config-reference.md#cli-overrides-index-run).

Richat (seen on 14.1.1) accepts `from_slot` only at `processed` commitment. The
indexer subscribes at `confirmed`, so Richat rejects the replay
(`from_slot rejected; resubscribing from tip`). The indexer then starts at the
tip and records the down time as a gap. Backfill repairs the gap over `rpc_url`.
For RPC speed, see [Backfill load](#backfill-load).

Live check of an endpoint: set `YELLOWSTONE_ENDPOINT` and `YELLOWSTONE_AUTH_TOKEN`,
then run `cargo test --release live_geyser -- --ignored --nocapture`. The test
reads 20 confirmed slots with token transfers, then replays from 50 slots back.
Official Yellowstone (Triton) passes. Richat fails at the replay step.

Client settings (`endpoint` scheme, `replay`, `ping_secs`, `compression`): see [`geyser`](./config-reference.md#geyser). The client also answers server pings.

**Idempotency:** Events upsert on `(signature, ix_path, kind)`. `ix_path` is a CPI path (`0`, `0.1`, `0.1.2`), stable across geyser and RPC backfill. Replaying the same instruction updates the payload if it changed and does not insert a duplicate row.

---

## Gap detection and RPC backfill

### How gaps are detected

Gaps are checked when a slot reaches **confirmed** status. `L` is the last
confirmed slot, `S` is the new one, and `P` is the parent of `S` (from the slot
update, or from the earlier processed update for `S`):

- `P` known and `P > L`: the range `(L+1)..P` is a gap.
- `P` known and `P <= L` (a fork switch): no gap.
- Slots between `P` and `S` were skipped by their leaders. They have no block,
  so they are never a gap.
- `P` not known: the range `(L+1)..(S-1)` is a gap.

For each gap:

1. A `gap` event is written to the store and broadcast on `/stream`
2. A row is inserted into the `gaps` table (`filled = false`)
3. If a backfill worker is running, the range is queued for repair

```bash
curl 'http://127.0.0.1:8080/gaps?open_only=true'
```

### Enabling backfill

Set `rpc_url` in `index.yaml` or `SOLANA_RPC_URL` in `.env`. Without an RPC URL, gaps are **detected and logged** but slots are **not** fetched.

The backfill worker:

1. Receives gap ranges on an internal channel. When it has no work, it also
   reads open gaps from the store: at start, then at most every 60s
2. Repairs a gap in chunks of 1000 slots. The rest of a larger gap is saved as
   a new open gap
3. Calls `getBlock` at `confirmed` commitment for each slot of the chunk,
   decodes the instructions and stores the events. New events also go to the
   live bus and the sinks, with `try_send`, so a slow sink never stalls backfill
4. Closes the chunk. Slots that failed with a retryable error stay open as
   smaller gaps, and a later re-scan tries them again. Permanent RPC errors
   (`-32001` block cleaned up, `-32011` no transaction history) close the slot
   with a warning. A "skipped" answer for the last slot of a gap is retried;
   the third such answer closes it as skipped

For RPC choice, see [Backfill load](#backfill-load).

### Monitoring gaps

| Signal | Where |
|--------|-------|
| Open gap count | `GET /health`, `GET /stats`, `GET /gaps` |
| Gap events | WebSocket `/stream`, `GET /events?type=gap` |
| Prometheus | `indexer_open_gaps` gauge on `/metrics` |

Investigate sustained non-zero `open_gaps`. The usual causes are geyser disconnects, provider lag, or RPC backfill failures.

---

## IDL drift

Program interfaces change on-chain. The indexer uses **idl-drift** for safe IDL management.

### Registering IDLs (`parser new`)

```bash
cargo run -- parser new --idl ./path/to/idl.json
```

Runs a safety gate, then writes `./idls/<program-id>.json`. If a file in
`./idls/` already holds the same address, that file is the drift baseline and
is updated in place. The write goes to a temp file first, then a rename.

The gate:

- **Breaking** changes block the install (exit non-zero), unless you pass `--force`
- **Dangerous** changes install, and the command prints them. Review them
- **Additive** and **cosmetic** changes install
- Reachable unmapped (`Generic`) fields block the install, unless you pass `--force`

### Diffing IDLs

```bash
cargo run -- parser diff ./idls/pumpFun.json ./downloads/pump_fun_new.json
```

Exits with code 1 on breaking drift.

### Fetching from chain

```bash
cargo run -- parser fetch --program-id <PROGRAM_ID> --rpc-url "$SOLANA_RPC_URL"
```

Reads the Anchor IDL account (`create_with_seed(program_signer, "anchor:idl")`),
zlib-decompresses it, and installs it through the same idl-drift gate as
`parser new`.

Pre-0.30 Anchor IDLs lack two things the router needs:

- **`address`**: the router keys programs by it. Fetch always sets it to the
  `--program-id` you passed, also when the IDL names another address (Codama
  documents: `program.publicKey`). The IDL account is derived from that
  program id, so an address inside the IDL is not trusted.
- **instruction discriminators**: derived when the IDL is parsed, as
  `sha256("global:" + snake_case(name))[..8]`, the same value Anchor computes
  from the Rust fn name. Explicit discriminators (Anchor 0.30+) are never
  overwritten.

Confirm the IDL loads before relying on the parser:

```bash
cargo run -- parser list --idl-dir ./idls
# prints one program id per loaded IDL
```

An IDL with an empty or invalid `address` does not load: the log shows
`skip IDL file` and `parser list` does not print it.

### Watching for drift (`parser watch`)

Polls on-chain IDLs on an interval (default 300s, minimum 10s):

```bash
cargo run -- parser watch --interval 300
```

- Logs warnings on dangerous/breaking drift
- Auto-updates local files on dangerous-only drift
- Breaking drift requires `--force` to overwrite

Run `parser watch` as a sidecar or cron job in production. After an IDL update, restart the indexer to reload parsers.

---

## Health endpoints

| Endpoint | Auth | Description |
|----------|------|-------------|
| `GET /health` | Public, no key | `{"ok": true, "slot", "finalized_slot", "events_total", "open_gaps"}` |
| `GET /stats` | API key if configured | Live counters: `slot`, `finalized_slot`, `events_total`, `token_transfers`, `program_ix`, `open_gaps`, `lag_slots` |
| `GET /metrics` | Public, no key | Prometheus text exposition |

`/health` and `/metrics` never need an API key, so they show slot progress, the
event count and the open gap count to anyone who can reach the port. With no API
keys, every route (these two too) needs a `localhost` or IP-literal `Host`;
see [403 on every route](#403-on-every-route).

### Prometheus metrics

| Metric | Type | Meaning |
|--------|------|---------|
| `indexer_events_total` | counter | Events decoded and stored |
| `indexer_slot_current` | gauge | Latest confirmed slot |
| `indexer_slot_finalized` | gauge | Latest finalized slot |
| `indexer_lag_slots` | gauge | Confirmed slot minus finalized slot |
| `indexer_open_gaps` | gauge | Unfilled gap ranges |
| `indexer_orphan_slots_total` | counter | Slots dropped by fork rollback at finalization |
| `indexer_geyser_reconnects_total` | counter | Geyser reconnect attempts |
| `indexer_geyser_errors_total` | counter | Geyser errors before reconnect |
| `indexer_stream_lagged_events_total` | counter | Live events skipped for slow WebSocket or gRPC stream clients |

Scrape example:

```yaml
# prometheus.yml
scrape_configs:
  - job_name: solana-indexer
    static_configs:
      - targets: ["localhost:8080"]
    metrics_path: /metrics
```

---

## Common failures

### Geyser disconnect / stream closed

**Symptoms:** Log lines `geyser stream closed; reconnecting` or `geyser stream error; reconnecting`. `indexer_geyser_reconnects_total` increases.

**Behavior:** Exponential backoff (500ms doubling to a 15s cap), reconnect from last committed slot. The backoff resets only after a session that delivered slot or transaction data; pings and pongs do not count.

**Actions:**

1. Check provider status and endpoint URL
2. Verify auth token has not expired
3. Watch for gap growth during extended outages; ensure `rpc_url` is set for backfill

### Authentication errors

**Symptoms:** Repeated connect failures, HTTP 401 from provider, empty stream.

**Actions:**

1. Confirm `YELLOWSTONE_AUTH_TOKEN` / `geyser.auth_token`
2. Test endpoint with provider CLI or grpcurl if available
3. Ensure `.env` is loaded (process cwd matters for `dotenv`)

### Empty subscription / no events

**Symptoms:** `/health` shows slot advancing but `events_total` stays at 0.

**Causes and fixes:**

| Cause | Fix |
|-------|-----|
| No IDLs loaded and `track_tokens: false` | Add IDLs to `programs` or enable `track_tokens` |
| Wallet filter too narrow | Clear `wallets` or broaden `accounts_include` |
| Wrong network IDL | Match IDL program id to chain (mainnet vs devnet) |
| Program not in subscription | Ensure program id appears in loaded IDL router |
| IDL file skipped at load | The log shows `skip IDL file` (empty or invalid `address`, or a second IDL for the same address). Re-fetch so the program id is stamped in. |
| Filter matches, but no events for a busy program | Some programs (seen with Drift and marginfi) yield nothing for minutes, while others on the same subscription flow normally. Both use many address lookup tables (ALTs). The provider's geyser build decides if `account_include` matches ALT addresses. Ask your provider, and try to add specific accounts to `accounts_include`. |

### IDL fails to parse

**Symptoms:** `parser new` / `parser fetch` exits with `parse IDL: unknown variant '<type>'`.

The flat model in `vendor/idl-drift/src/model.rs` covers the Borsh scalars
(`bool`, `u8`-`u128`, `i8`-`i128`, `f32`, `f64`, `bytes`, `string`, `pubkey`)
plus `option`, `vec`, `array`, `map`, `set`, `tuple`, `defined`, and `generic`.
A type outside that list is a gap in the model, not a bad IDL: add the variant
to `Type`, map it in `codama_convert.rs`, and decode it in
`src/pipeline/decode_args.rs`.

### Database errors

**Symptoms:** `500` with `{"error": "internal error"}` on `/events`, `/cursor` or `/gaps`; the log line `store query failed` holds the cause. Insert errors in logs.

A `503` with `{"error": "busy"}` (gRPC: `UNAVAILABLE`) means the API reads,
which share 3 of the 5 DB connections, were all busy for 5s. Retry later.
Ingest writes never wait for these reads.

A `400` on `/events` is a bad query, not a store fault: `offset` above 10000, or
`since_slot` / `until_slot` above `i64::MAX`.

**Actions:**

1. Check disk space (SQLite) or Postgres connectivity
2. Verify `DATABASE_URL` / `store.url`. Only `sqlite:`, a file path, and `postgres://` / `postgresql://` are accepted; `mysql://` and other schemes stop startup
3. For Postgres, ensure migrations ran (tables `events`, `cursors`, `gaps`, `slots`, `ch_outbox`)

Migrations in `migrations/<backend>/` run at startup, including on an existing
database, so a stopped indexer picks up new ones on its next boot. Check what
applied:

```bash
sqlite3 ./index.db "select version, description from _sqlx_migrations order by version;"
psql "$DATABASE_URL" -c "select version, description from _sqlx_migrations order by version;"
```

### Docker volume permission denied

**Symptoms:** In Docker, the indexer cannot open or write `/data/index.db`
(permission denied).

**Cause:** The image now runs as the non-root user `indexer` (uid 10001). An
`index-data` volume from the older root image holds a root-owned `index.db`.

**Actions:** Change the owner of the volume:

```bash
docker compose run --rm --user root --entrypoint chown indexer -R indexer:indexer /data
```

Or delete the volume and start again. This deletes all indexed data:
`docker compose down -v`.

### Webhook sink failures

**Symptoms:** `event sink failed` warnings.

**Behavior:** Up to 3 attempts per event with backoff. Each request has a 2s
connect and a 5s total timeout. Redirects are not followed, so a `3xx` counts
as a failure. Sinks run off the ingest loop; when their queue is full, live and backfilled events
skip the sinks but are still stored. The indexer continues.

A bad sink entry in the config stops startup; the error lists each bad entry.

**Actions:** Fix downstream URL, timeouts, or rate limits. Point the webhook at its final URL. Use sink filters to reduce volume.

### API key rejected

**Symptoms:** `401` on `/events`, `/stream`, etc.

**Actions:** Send `X-API-Key` or `Authorization: Bearer`. Keys come from `auth.api_keys` or `INDEX_API_KEYS`.

- WebSocket: send `Sec-WebSocket-Protocol: index-key, <key>`. A `?api_key=` query is not accepted. In the live view, open `/#key=<key>`; the key must use only letters, digits and `-._~`.
- gRPC export: send metadata `x-api-key: <key>` or `authorization: Bearer <key>`; a missing key gives `Unauthenticated`.

### 403 on every route

**Cause:** With no API keys, on any bind, the server accepts only a `Host` of
`localhost`, `*.localhost` or an IP literal (for example `127.0.0.1:8080`,
`10.0.0.5`, `[::1]`). Any other name gets `403`, and so does a request with no
`Host` and no authority in the URI. This stops DNS rebinding: a web page cannot
read an open API through its own domain name. A keyless server behind a reverse
proxy that sends its domain name as `Host` gets `403`.

**Actions:** Set API keys, or make the proxy send an IP literal as `Host`
(for example `Host: 127.0.0.1`).

A `403` on `/stream` only can also mean a cross-site page: the browser `Origin`
must match `Host` or `X-Forwarded-Host`.

---

## Capacity notes

### Token traffic volume

`track_tokens: true` subscribes to SPL Token and Token-2022 programs. Mainnet token activity is **very high**. Expect large event volume, higher CPU, and faster DB growth.

**Recommendations:**

- Leave `track_tokens: false` unless you need `token_transfer` events
- Prefer wallet-scoped or program-scoped filters via `wallets`, `accounts_include`, and IDL program ids
- Use sink `types` / `instructions` filters for webhook fan-out
- Consider Postgres for production stores under heavy write load

### Geyser throughput

Each decoded instruction generates at least one DB write and a broadcast. Monitor:

- `events_total` growth rate via `/stats`
- SQLite file size or Postgres table bloat
- Memory from WebSocket subscribers (broadcast channel capacity 1024)

### Query indexes

`/events` filters on `kind`, `program`, `signature`, and slot range. Each has an
index, so a filtered query seeks instead of scanning:

| Filter | Index |
|--------|-------|
| `type` | `events_kind_idx (kind, id)` |
| `program` | `events_program_idx (program, id)` |
| `signature` | `UNIQUE (signature, ix_path, kind)` |
| `since_slot` / `until_slot` | `events_slot_idx (slot)` |

`id` trails `kind` and `program` because every query ends in `ORDER BY id ASC`.
The planner walks the index already ordered and skips the sort. Without them a
`program =` filter scanned the whole table: 1.48s at 586k rows, versus under
10ms with them. The indexes add about 9% on disk (48 MB at 586k rows).

### Server limits

| Limit | Value | When reached |
|-------|-------|--------------|
| HTTP protocol | HTTP/1.1 only | h2c (cleartext HTTP/2) is not served |
| HTTP connections | 512 | Extra connections are closed with no response and no log |
| HTTP request header read | 10s | Connection closed. This also closes a silent new connection and an idle keep-alive connection |
| WebSocket clients | 256 | `503` |
| WebSocket message / frame from a client | 4 KiB | Connection closed |
| WebSocket ping | every 30s | A peer that sends nothing back for a full interval is closed |
| API DB reads | 3 of 5 connections, 5s wait | HTTP `503 {"error":"busy"}`, gRPC `UNAVAILABLE` |
| Postgres statement time | 30s (`statement_timeout`) | The query is canceled. Set `options[statement_timeout]=<value>` in the database URL to change it |
| gRPC streams per connection | 64 | HTTP/2 setting; the client cannot open more |
| gRPC `StreamEvents` streams per process | 256 | `RESOURCE_EXHAUSTED` |
| gRPC `StreamEvents` slow reader | 10s | When its buffer is full and it reads nothing for 10s, the slot is freed. The client gets the buffered events, then `DATA_LOSS`. Resync with `GetEvents` |
| gRPC in-flight requests per connection | 64 | Requests wait |
| gRPC request timeout | 30s, to response headers only | Open `StreamEvents` streams are not cut |
| gRPC HTTP/2 keepalive ping | every 30s | A connection that does not answer is closed |

### ClickHouse sink

With a `clickhouse` section, the log shows `clickhouse sink on` at start. A
failed send logs `clickhouse sink failed; rows stay in the outbox` and retries.

Rows waiting for ClickHouse:

```bash
sqlite3 index.db "SELECT count(*), min(slot), max(slot) FROM ch_outbox;"
```

A count that only grows means ClickHouse rejects the inserts or cannot be
reached. Rows above the finalized slot wait by design.

Read the merged rows (one row per event):

```sql
SELECT kind, count() FROM index_events FINAL GROUP BY kind;
```

### Data checks

- A transaction whose signature is not 64 bytes is dropped. An instruction
  with an account index outside the account list is dropped. Each logs one
  warning per process.
- Inner-instruction stack heights above 16 are taken as 16 for `ix_path`.
- Argument decode has a work budget of 1 MiB per instruction. Decoding stops at
  the argument that goes over it; the earlier arguments are kept.
- An IDL whose program name is longer than 64 bytes, or holds control or bidi
  characters, is rejected by `parser new` / `parser fetch` and skipped at load
  (`skip IDL file`).
- The `program` column keeps at most 128 bytes of the name; the payload keeps
  the full name.

### Backfill load

Large gap ranges trigger sequential `getBlock` RPC calls, 1000 slots per chunk. The public mainnet RPC repairs about 1 slot per second and can throttle heavy repair. A provider rate limit leaves `open_gaps` high. For repair, use a fast RPC with a dedicated key and higher limits.

---

## Upgrading from an older version

- Each start resumes at the cursor slot and replays it, when `replay` allows
  it. For current rows the replay changes nothing. Rows from before the upgrade
  carry `ix_path` `legacy:N` (migration 005), so they do not match the new
  paths. On the first start after the upgrade, the replay stores new-format
  copies of that one slot next to its legacy rows. Sinks and `/stream` get
  those events again.
- Rows stored with `track_failed: true` before the upgrade have no `failed`
  marker, so an old failed-transaction row reads as `"failed": false`.

---

## Useful commands

```bash
# Validate config
cargo run -- config validate

# Demo from an existing DB (no geyser; not read-only)
cargo run -- demo --db sqlite:./index.db

# Terminal live view (logs go to ./index.log)
cargo run -- run --config index.yaml --tui

# Query recent program instructions
curl 'http://127.0.0.1:8080/events?type=program_ix&limit=10'
```
