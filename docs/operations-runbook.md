# Operations runbook

Day-two operations for the typed-event indexer: cursor semantics, gap repair, IDL drift, health checks, and common failure modes.

## Cursor semantics

The indexer persists a **slot cursor** in the `cursors` table (id `1`). On every processed slot update, `commit_cursor` writes the highest observed slot.

**Resume on restart:** When `index run` starts, it loads the cursor and sets the geyser `from_slot` to `last_slot + 1`. No manual checkpointing is required.

```bash
curl http://127.0.0.1:8080/cursor
# {"slot": 285001234, "updated_at": "..."}
```

The ingest loop also keeps an in-memory atomic slot for reconnect: if the geyser stream drops, the client reconnects from the last committed slot rather than replaying from genesis.

**Idempotency:** Events are deduplicated on insert (signature + ix index). Re-processing overlapping slots does not duplicate rows.

---

## Gap detection and RPC backfill

### How gaps are detected

When a new slot `S` arrives and the previous committed slot was `L`, if `S > L + 1` the range `(L+1)..(S-1)` is recorded as a gap:

1. A `gap` event is written to the store and broadcast on `/stream`
2. A row is inserted into the `gaps` table (`filled = false`)
3. If a backfill worker is running, the range is queued for repair

```bash
curl 'http://127.0.0.1:8080/gaps?open_only=true'
```

### Enabling backfill

Set `rpc_url` in `index.yaml` or `SOLANA_RPC_URL` in `.env`. Without an RPC URL, gaps are **detected and logged** but slots are **not** fetched.

The backfill worker:

1. Receives gap ranges on an internal channel
2. Calls `getBlock` for each missing slot
3. Decodes instructions and stores events
4. Marks the gap `filled = true`

Use a reliable, rate-limit-aware RPC provider for production backfill. Public mainnet endpoints may throttle heavy gap repair.

### Monitoring gaps

| Signal | Where |
|--------|-------|
| Open gap count | `GET /health`, `GET /stats`, `GET /gaps` |
| Gap events | WebSocket `/stream`, `GET /events?type=gap` |
| Prometheus | `indexer_open_gaps` gauge on `/metrics` |

Investigate sustained non-zero `open_gaps` — usually geyser disconnects, provider lag, or RPC backfill failures.

---

## IDL drift

Program interfaces change on-chain. The indexer uses **idl-drift** for safe IDL management.

### Registering IDLs (`parser new`)

```bash
cargo run -- parser new --idl ./path/to/idl.json
```

Runs a safety gate before copying into `./idls/`:

- **Breaking** changes block install (exit non-zero)
- **Dangerous** changes are reported; review before `--force`
- **Additive** / **cosmetic** changes are allowed

### Diffing IDLs

```bash
cargo run -- parser diff ./idls/pump_fun.json ./downloads/pump_fun_new.json
```

Exits with code 1 on breaking drift.

### Fetching from chain

```bash
cargo run -- parser fetch --program-id <PROGRAM_ID> --rpc-url "$SOLANA_RPC_URL"
```

### Watching for drift (`parser watch`)

Polls on-chain IDLs on an interval (default 300s):

```bash
cargo run -- parser watch --interval 300
```

- Logs warnings on dangerous/breaking drift
- Auto-updates local files on dangerous-only drift
- Breaking drift requires `--force` to overwrite

Run `parser watch` as a sidecar or cron job in production. After an IDL update, restart the indexer to reload parsers (or rely on a future hot-reload if added).

---

## Health endpoints

| Endpoint | Auth | Description |
|----------|------|-------------|
| `GET /health` | Public | `{"ok": true, "slot", "events_total", "open_gaps"}` |
| `GET /stats` | API key if configured | Live counters: `slot`, `events_total`, `token_transfers`, `program_ix`, `open_gaps`, `lag_slots` |
| `GET /metrics` | Public | Prometheus text exposition |

### Prometheus metrics

| Metric | Type | Meaning |
|--------|------|---------|
| `indexer_events_total` | counter | Events decoded and stored |
| `indexer_slot_current` | gauge | Latest observed slot |
| `indexer_lag_slots` | gauge | Slots behind chain head (when computable) |
| `indexer_open_gaps` | gauge | Unfilled gap ranges |
| `indexer_geyser_reconnects_total` | counter | Geyser reconnect attempts |
| `indexer_geyser_errors_total` | counter | Geyser errors before reconnect |

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

**Behavior:** Exponential backoff (500ms → 15s cap), reconnect from last committed slot.

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

### Database errors

**Symptoms:** `500` on `/events`, insert errors in logs.

**Actions:**

1. Check disk space (SQLite) or Postgres connectivity
2. Verify `DATABASE_URL` / `store.url`
3. For Postgres, ensure migrations ran (tables `events`, `cursors`, `gaps`)

### Webhook sink failures

**Symptoms:** `event sink failed` warnings.

**Behavior:** Three retries with exponential backoff per event; indexer continues.

**Actions:** Fix downstream URL, timeouts, or rate limits. Use sink filters to reduce volume.

### API key rejected

**Symptoms:** `401` on `/events`, `/stream`, etc.

**Actions:** Send `X-API-Key` or `Authorization: Bearer`. Keys come from `auth.api_keys` or `INDEX_API_KEYS`.

---

## Capacity notes

### Token traffic volume

`track_tokens: true` subscribes to SPL Token and Token-2022 programs. Mainnet token activity is **very high** — expect large event volume, higher CPU, and faster DB growth.

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

### Backfill load

Large gap ranges trigger sequential `getBlock` RPC calls. A provider rate limit during backfill leaves `open_gaps` elevated. Use a dedicated RPC key with higher limits for repair workloads.

---

## Useful commands

```bash
# Validate config
cargo run -- config validate

# Read-only demo from existing DB
cargo run -- demo --db sqlite:./index.db

# Terminal live view
cargo run -- run --config index.yaml --tui

# Query recent program instructions
curl 'http://127.0.0.1:8080/events?type=program_ix&limit=10'
```
