# Quickstart

This guide runs the indexer locally.

## Prerequisites

- [Rust](https://rustup.rs/) 1.88 or later (edition 2024 with let chains; `tonic` 0.14 needs 1.88)
- `protoc` on `PATH`: `build.rs` compiles the protos in `src/proto`
- A Yellowstone / Dragon's Mouth gRPC endpoint and `x-token`
- (Optional) Solana JSON-RPC URL for gap backfill

## 1. Clone the repository

```bash
git clone https://github.com/senzenn/Solana-gRPC_indexer-Rust.git
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
the env or `.env`. Do not pass secrets as flags: see [CLI overrides](./config-reference.md#cli-overrides-index-run).

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
cargo run -- parser new --idl ./path/to/idl.json
```

Other parser commands:

```bash
cargo run -- parser list
cargo run -- parser diff ./old.json ./new.json
cargo run -- parser fetch --program-id <PROGRAM_ID>
cargo run -- parser watch   # poll chain for IDL drift
```

Use `--force` to accept breaking IDL changes.

`parser new` and `parser fetch` write `./idls/<program-id>.json`. If a file
there already holds the same address, they update that file in place.

For a typed parser built into the binary, put the IDL in `./parsers/` and
build again. See [parsers/README.md](../parsers/README.md). An IDL file in
`./idls/` wins over a compiled parser for the same program.

`parser fetch` also reads pre-0.30 Anchor IDLs: see [Fetching from chain](./operations-runbook.md#fetching-from-chain).

Check that the IDL loaded:

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

You can also run with no config file. The endpoint and token then come from `.env`.
This run has no sinks and no ClickHouse copy. Other fields come from env vars and flags, or use their defaults:

```bash
cargo run -- run
```

At start, the process loads the cursor from the store (default `./index.db`). It resumes from the last committed slot only when `replay` allows it. Otherwise the stream starts at the tip, and the down time becomes a gap. See [Cursor semantics](./operations-runbook.md#cursor-semantics).

## 6. Open the live view

```bash
open http://127.0.0.1:8080
```

The HTML table at `/` shows decoded events in real time. WebSocket clients can connect to `WS /stream`.

Open it as `127.0.0.1` or `localhost`. With no API keys, other host names get `403`. Bind and TLS: see [`serve`](./config-reference.md#serve).

API keys and protected routes: see [`auth`](./config-reference.md#auth). With keys set, open the live view as `http://127.0.0.1:8080/#key=<key>`.

## 7. Docker alternative

```bash
cp .env.example .env
cp index.example.yaml index.yaml
# set YELLOWSTONE_ENDPOINT and YELLOWSTONE_AUTH_TOKEN in .env

docker compose up --build
```

To serve the stored events with no geyser credentials, run
`docker compose --profile demo up --build demo`. It listens on `127.0.0.1:8081`.

The compose file publishes port `8080` on `127.0.0.1` only, persists SQLite under a Docker volume, and mounts `./idls`. It passes `.env` (including the `x-token`) to the container as environment, not as command arguments. To use `./index.yaml`, uncomment its volume line and add `--config /app/index.yaml` to `command`.

## Next steps

- Full field reference: [config-reference.md](./config-reference.md)
- Operations (cursor, gaps, backfill, IDL drift, troubleshooting): [operations-runbook.md](./operations-runbook.md)
- Serve stored events without geyser: `cargo run -- demo --db sqlite:./index.db` (see [Demo mode](../README.md#demo-mode-no-geyser))
