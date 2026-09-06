# Quickstart

Get the typed-event indexer running locally in a few minutes. You only need a Yellowstone geyser endpoint, an auth token, and the files in this repo.

## Prerequisites

- [Rust](https://rustup.rs/) 1.75+ (edition 2024)
- A Yellowstone / Dragon's Mouth gRPC endpoint and `x-token`
- (Optional) Solana JSON-RPC URL for gap backfill

The `idl-drift` crate is **vendored** in `vendor/idl-drift` — you do **not** need a sibling `../idl-drift` repo. A plain `cargo build` works from this directory alone.

## 1. Clone the repository

```bash
git clone https://github.com/<org>/Solana-gRPC_indexer-Rust.git
cd Solana-gRPC_indexer-Rust
```

## 2. Configure environment

```bash
cp .env.example .env
```

Edit `.env` and set at minimum:

| Variable | Description |
|----------|-------------|
| `YELLOWSTONE_ENDPOINT` | Geyser gRPC URL (e.g. `https://your-provider.example.com`) |
| `YELLOWSTONE_AUTH_TOKEN` | Provider `x-token` |

Optional variables are documented in [config-reference.md](./config-reference.md#environment-variable-overrides).

## 3. Create `index.yaml`

```bash
cp index.example.yaml index.yaml
```

The example config points at `./idls/pump_fun.json` and a public mainnet RPC for backfill. Geyser credentials can stay empty in YAML if you set them in `.env` (env vars override YAML).

Validate the file:

```bash
cargo run -- config validate --file index.yaml
```

## 4. Register program IDLs (optional)

Sample IDLs ship in `./idls/` (`pump_fun.json`, `pumpFun.json`). To add or refresh an IDL with the idl-drift safety gate:

```bash
cargo run -- parser new --idl ./idls/pumpFun.json
```

Other parser commands:

```bash
cargo run -- parser list
cargo run -- parser diff ./idls/old.json ./idls/new.json
cargo run -- parser fetch --program-id <PROGRAM_ID>
cargo run -- parser watch   # poll chain for IDL drift
```

Use `--force` to accept breaking IDL changes.

## 5. Start the indexer

```bash
cargo run -- run --config index.yaml
```

Equivalent without a config file:

```bash
cargo run -- run \
  --endpoint "$YELLOWSTONE_ENDPOINT" \
  --auth-token "$YELLOWSTONE_AUTH_TOKEN"
```

On startup the process loads the SQLite cursor from `./index.db` (or your configured store) and resumes from the last committed slot.

## 6. Open the live view

```bash
open http://127.0.0.1:8080
```

The HTML table at `/` shows decoded events in real time. WebSocket clients can connect to `WS /stream`.

Protected routes (`/events`, `/stream`, `/cursor`, `/gaps`, `/stats`) require an API key when `auth.api_keys` or `INDEX_API_KEYS` is set. `/`, `/health`, and `/metrics` stay public.

## 7. Docker alternative

```bash
cp .env.example .env
cp index.example.yaml index.yaml
# set YELLOWSTONE_ENDPOINT and YELLOWSTONE_AUTH_TOKEN in .env

docker compose up --build
```

The compose file maps port `8080`, persists SQLite under a Docker volume, mounts `./idls`, and uses `./index.yaml` for configuration.

## Next steps

- Full field reference: [config-reference.md](./config-reference.md)
- Operations (cursor, gaps, backfill, IDL drift, troubleshooting): [operations-runbook.md](./operations-runbook.md)
- Serve a snapshot without geyser: `cargo run -- demo --db sqlite:./index.db`
