# Operations runbook

## Cursor semantics

The `cursors` table (id `1`) holds two slots:

- `slot`: highest **confirmed** slot
- `finalized_slot`: highest **finalized** slot that survived fork rollback

A confirmed slot advances `slot` and runs gap detection. A finalized slot walks
the parent chain and deletes events from orphaned slots (indexed slots that are
not ancestors of the new finalized tip). If the parent chain is incomplete,
nothing is deleted. A dead slot only logs `dead slot`; its slot row is removed
as an orphan at finalization.

**Slot jumps:** a confirmed, finalized or event slot more than 1,000,000 above
the last confirmed slot is skipped with a warning, as corrupt data. After 32
such confirmed slots in a row, the jump is accepted and the missing range
becomes a gap.

**Resume:** at start, `run` loads the confirmed cursor (log field `resume_slot`).
If [`replay`](./config-reference.md#geyser) allows it, that slot goes to geyser
as `from_slot` and is replayed. Otherwise the stream starts at the tip, and the
down time becomes a gap. A dropped stream reconnects from the last committed
slot under the same rule.

```bash
curl http://127.0.0.1:8080/cursor
# {"slot": 285001234, "finalized_slot": 285001200, "updated_at": "..."}
```

**Idempotency:** events upsert on `(signature, ix_path, kind)`. `ix_path` is a
CPI path (`0`, `0.1`, `0.1.2`), the same for geyser and RPC backfill. A replay
updates a changed payload and adds no duplicate row.

**Triton / Richat:** run `cargo run -- probe` before you rely on `from_slot`.
Richat (seen on 14.1.1) accepts `from_slot` only at `processed` commitment. The
indexer subscribes at `confirmed`, so Richat rejects the replay
(`from_slot rejected; resubscribing from tip`) and backfill repairs the gap.

Live endpoint test: set `YELLOWSTONE_ENDPOINT` and `YELLOWSTONE_AUTH_TOKEN`,
then run `cargo test --release live_geyser -- --ignored --nocapture`. It reads
20 confirmed slots, then replays from 50 slots back. Triton passes. Richat fails
at the replay step.

## Gaps and backfill

Gaps are checked when a slot is **confirmed**. `L` is the last confirmed slot,
`S` the new one, `P` the parent of `S`:

- `P` known and `P > L`: `(L+1)..P` is a gap.
- `P` known and `P <= L` (fork switch): no gap.
- Slots between `P` and `S` were skipped by their leaders. They are never a gap.
- `P` not known: `(L+1)..(S-1)` is a gap.

Each gap writes a `gap` event (also on `/stream`), adds a `gaps` row with
`filled = false`, and queues the range for backfill.

Backfill needs `rpc_url` or `SOLANA_RPC_URL`. Without it, gaps are recorded but
not repaired. The worker:

1. Takes ranges from the channel. When idle, it also reads open gaps from the
   store: at start, then at most every 60s.
2. Repairs 1000 slots per chunk. The rest of a larger gap is saved as a new open gap.
3. Calls `getBlock` at `confirmed` for each slot and stores the events. New
   events also go to `/stream` and the sinks with `try_send`, so a slow sink
   never stalls backfill.
4. Leaves slots with a retryable error open as smaller gaps. Permanent errors
   (`-32001` block cleaned up, `-32011` no transaction history) close the slot
   with a warning. A "skipped" answer for the last slot of a gap is retried;
   the third one closes it.

The public mainnet RPC repairs about 1 slot per second and can throttle. Use a
dedicated RPC key for large repairs.

Watch `open_gaps` on `/health`, `/stats`, `/gaps?open_only=true`, or the
`indexer_open_gaps` gauge. Usual causes: geyser disconnects, provider lag, RPC
backfill failures.

## IDL drift

`parser new --idl <file>` and `parser fetch` write `./idls/<program-id>.json`
through the idl-drift gate. If a file there already holds the same address, it
is the baseline and is updated in place (temp file, then rename).

- **Breaking**: blocked unless `--force`
- **Dangerous**: installed and printed. Review them
- **Additive** / **cosmetic**: installed
- Reachable unmapped (`Generic`) fields: blocked unless `--force`

`parser diff <old> <new>` exits 1 on breaking drift.

### Fetching from chain

`parser fetch --program-id <ID>` reads the Anchor IDL account
(`create_with_seed(program_signer, "anchor:idl")`) and zlib-decompresses it.
For pre-0.30 Anchor IDLs:

- **`address`** is always set to `--program-id`. An address inside the IDL is
  not trusted, because the IDL account is derived from the program id.
- **discriminators** are derived at parse time as
  `sha256("global:" + snake_case(name))[..8]`. Explicit discriminators
  (Anchor 0.30+) are never overwritten.

Check with `parser list --idl-dir ./idls`. A skipped IDL logs `skip IDL file`.

### Watching (`parser watch`)

Polls on-chain IDLs every `--interval` seconds (default 300, minimum 10). It
logs dangerous and breaking drift, updates local files on dangerous-only drift,
and needs `--force` to overwrite on breaking drift. Restart the indexer after
an update to reload parsers.

## Metrics

`/health` and `/metrics` need no API key. They show slot progress, the event
count and the gap count to anyone who can reach the port.

| Metric | Type | Meaning |
|--------|------|---------|
| `indexer_events_total` | counter | Events decoded and stored |
| `indexer_slot_current` | gauge | Latest confirmed slot |
| `indexer_slot_finalized` | gauge | Latest finalized slot |
| `indexer_lag_slots` | gauge | Confirmed minus finalized slot |
| `indexer_open_gaps` | gauge | Unfilled gap ranges |
| `indexer_orphan_slots_total` | counter | Slots dropped by fork rollback |
| `indexer_geyser_reconnects_total` | counter | Geyser reconnect attempts |
| `indexer_geyser_errors_total` | counter | Geyser errors before reconnect |
| `indexer_stream_lagged_events_total` | counter | Live events skipped for slow WS or gRPC clients |

## Common failures

### Geyser disconnect

Log: `geyser stream closed; reconnecting` or `geyser stream error; reconnecting`.
Backoff starts at 500ms and doubles to 15s. It resets only after a session that
delivered slot or transaction data. Check the provider, the endpoint and the
token. Set `rpc_url` so backfill repairs the outage.

### No events

`/health` shows the slot moving, but `events_total` stays at 0.

| Cause | Fix |
|-------|-----|
| No IDLs and `track_tokens: false` | Add IDLs or set `track_tokens: true` |
| IDL for the wrong network | Match the program id to the chain |
| IDL skipped at load | Log shows `skip IDL file` (bad `address`, or a second IDL for the same address). Re-fetch |
| Busy program, no events for minutes | Seen with Drift and marginfi, which use many address lookup tables. The provider's geyser build decides if `account_include` matches ALT addresses. Ask the provider, or add accounts to `accounts_include` |

### IDL fails to parse

`parse IDL: unknown variant '<type>'` means the type is missing from the model
in `vendor/idl-drift/src/model.rs`. Add the variant to `Type`, map it in
`codama_convert.rs`, and decode it in `src/pipeline/decode_args.rs`.

### Database errors

- `500 {"error": "internal error"}`: the log line `store query failed` holds the cause.
- `503 {"error": "busy"}` (gRPC `UNAVAILABLE`): API reads share 3 of the 5 DB
  connections, and all were busy for 5s. Ingest writes never wait for them.
- `400` on `/events`: `offset` above 10000, or a slot above `i64::MAX`.

Migrations run at every start. Check what applied:

```bash
sqlite3 ./index.db "select version, description from _sqlx_migrations order by version;"
```

### Docker: permission denied on `/data/index.db`

The image runs as `indexer` (uid 10001). A volume from the older root image
holds a root-owned file. Fix the owner:

```bash
docker compose run --rm --user root --entrypoint chown indexer -R indexer:indexer /data
```

### Sink failures

Log: `event sink failed`. Webhooks try 3 times with a 2s connect and 5s total
timeout, and do not follow redirects. When the sink queue (1024) is full,
events skip the sinks but are still stored.

### 401 or 403

- `401`: send the key. See [`auth`](./config-reference.md#auth).
- `403` on every route: with no API keys, `Host` must be `localhost`,
  `*.localhost` or an IP literal. This blocks DNS rebinding. Behind a proxy,
  set API keys or send an IP literal as `Host`.
- `403` on `/stream` only: the browser `Origin` must match `Host` or `X-Forwarded-Host`.

## Limits

### Server limits

| Limit | Value | When reached |
|-------|-------|--------------|
| HTTP protocol | HTTP/1.1 only | h2c is not served |
| HTTP connections | 512 | Extra connections close with no response |
| HTTP header read | 10s | Connection closed, also for idle keep-alive |
| WebSocket clients | 256 | `503` |
| WebSocket client message | 4 KiB | Connection closed |
| WebSocket ping | 30s | A silent peer is closed |
| API DB reads | 3 of 5 connections, 5s wait | `503 busy`, gRPC `UNAVAILABLE` |
| Postgres statement time | 30s | Query canceled. Change with `options[statement_timeout]=<value>` in the URL |
| gRPC streams per connection | 64 | Client cannot open more |
| gRPC `StreamEvents` per process | 256 | `RESOURCE_EXHAUSTED` |
| gRPC `StreamEvents` slow reader | 10s | Buffered events, then `DATA_LOSS`. Resync with `GetEvents` |
| gRPC in-flight requests per connection | 64 | Requests wait |
| gRPC request timeout | 30s, to response headers | Open streams are not cut |
| gRPC keepalive ping | 30s | A silent connection is closed |

### Data checks

- A signature that is not 64 bytes drops the transaction. An account index
  outside the account list drops the instruction. Each logs one warning per process.
- Stack heights above 16 count as 16 for `ix_path`.
- Argument decode has a 1 MiB budget per instruction. Decode stops at the
  argument that goes over it; earlier arguments are kept.
- A program name over 64 bytes, or with control or bidi characters, is rejected
  by `parser new` / `parser fetch` and skipped at load.
- The `program` column keeps at most 128 bytes of the name; the payload keeps it all.

### Query indexes

| `/events` filter | Index |
|------------------|-------|
| `type` | `events_kind_idx (kind, id)` |
| `program` | `events_program_idx (program, id)` |
| `signature` | `UNIQUE (signature, ix_path, kind)` |
| `since_slot` / `until_slot` | `events_slot_idx (slot)` |

`id` trails `kind` and `program` because every query ends in `ORDER BY id`.
At 586k rows, a `program` filter took 1.48s without the index and under 10ms
with it. The indexes add about 9% on disk.

### ClickHouse

Start logs `clickhouse sink on`. A failed send logs
`clickhouse sink failed; rows stay in the outbox` and retries. Rows above the
finalized slot wait by design. A count that only grows means ClickHouse rejects
the inserts or is unreachable:

```bash
sqlite3 index.db "SELECT count(*), min(slot), max(slot) FROM ch_outbox;"
```

Read merged rows with `SELECT kind, count() FROM index_events FINAL GROUP BY kind;`.

## Upgrading

- Rows from before migration 005 carry `ix_path` `legacy:N` and do not match
  new paths. The first start after the upgrade replays the cursor slot, so that
  slot gets new-format copies next to its legacy rows. Sinks and `/stream` get
  those events again.
- Rows stored with `track_failed: true` before the upgrade have no `failed`
  marker and read as `"failed": false`.
