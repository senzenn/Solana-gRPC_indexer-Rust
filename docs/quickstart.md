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
| `YELLOWSTONE_ENDPOINT` | Geyser gRPC URL. Triton or Richat Dragon's Mouth. Local: `http://127.0.0.1:10000`. A `host:port` with no scheme means `https://` unless the host is loopback; a remote plaintext endpoint needs `http://` |
| `YELLOWSTONE_AUTH_TOKEN` | Provider `x-token` |

Check the endpoint before a long run:

```bash
cargo run -- probe
```

`probe` and `run` read `YELLOWSTONE_ENDPOINT` and `YELLOWSTONE_AUTH_TOKEN` from
the env or `.env`. Do not pass the token (or an endpoint URL that holds a key)
as `--auth-token` / `--endpoint`: command-line arguments are visible in `ps`.

Optional variables are documented in [config-reference.md](./config-reference.md#environment-variable-overrides).

## 3. Create `index.yaml`

```bash
cp index.example.yaml index.yaml
```

The example config points at `./idls/pumpFun.json` and a public mainnet RPC for backfill. Geyser credentials can stay empty in YAML if you set them in `.env` (env vars override YAML).

Validate the file:

```bash
cargo run -- config validate --file index.yaml
```

## 4. Register program IDLs (optional)

Sample IDLs ship in `./idls/` (`pumpFun.json`). To add or refresh an IDL with the idl-drift safety gate:

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

`parser new` and `parser fetch` write `./idls/<program-id>.json`. If a file
there already holds the same address, they update that file in place.

`parser fetch` pulls the IDL straight off chain and works with pre-0.30 Anchor
IDLs, which omit the program address and instruction discriminators. Fetch
always sets the address to `--program-id`; discriminators are derived when the
IDL is parsed. Check it registered:

```bash
cargo run -- parser list
# dRiftyHA39MWEi3m9aunc5MzRF1JYuBsbn6VPcn33UH
```

An IDL with an empty or invalid `address` is not listed: it is skipped at load
with the warning `skip IDL file`.

## 5. Start the indexer

```bash
cargo run -- run --config index.yaml
```

Add `--tui` for a terminal view; logs then go to `./index.log`, not the screen.

Equivalent without a config file (endpoint and token come from `.env`):

```bash
cargo run -- run
```

On startup the process loads the SQLite cursor from `./index.db` (or your configured store) and resumes from the last committed slot.

## 6. Open the live view

```bash
open http://127.0.0.1:8080
```

The HTML table at `/` shows decoded events in real time. WebSocket clients can connect to `WS /stream`.

The server binds `127.0.0.1:8080` by default. A non-loopback bind with no API keys logs a warning. Neither the HTTP server nor the gRPC export has TLS: for a non-loopback bind, set API keys and put a TLS reverse proxy in front that sets `X-Forwarded-Host`. With no keys, the server answers `403` unless the `Host` is `localhost`, `*.localhost` or an IP literal, so open it as `127.0.0.1` or `localhost`, not by a domain name.

Protected routes (`/events`, `/stream`, `/cursor`, `/gaps`, `/stats`) require an API key when `auth.api_keys` or `INDEX_API_KEYS` is set. `/`, `/health`, and `/metrics` stay public and never need a key. `/health` shows `slot`, `finalized_slot`, `events_total` and `open_gaps`. With keys set, open the live view as `http://127.0.0.1:8080/#key=<key>`; the page sends the key as the WebSocket subprotocol `index-key, <key>`. Use only letters, digits and `-._~` in keys.

## 7. Docker alternative

```bash
cp .env.example .env
cp index.example.yaml index.yaml
# set YELLOWSTONE_ENDPOINT and YELLOWSTONE_AUTH_TOKEN in .env

docker compose up --build
```

For a read-only server over the stored events (no geyser credentials), run
`docker compose --profile demo up --build demo`. It listens on `127.0.0.1:8081`.

The compose file publishes port `8080` on `127.0.0.1` only, persists SQLite under a Docker volume, and mounts `./idls`. It passes `.env` (including the `x-token`) to the container as environment, not as command arguments. To use `./index.yaml`, uncomment its volume line and add `--config /app/index.yaml` to `command`.

## Next steps

- Full field reference: [config-reference.md](./config-reference.md)
- Operations (cursor, gaps, backfill, IDL drift, troubleshooting): [operations-runbook.md](./operations-runbook.md)
- Serve a snapshot without geyser: `cargo run -- demo --db sqlite:./index.db`
