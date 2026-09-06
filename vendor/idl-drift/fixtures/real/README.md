# Real Codama IDL fixtures

Drop real Yellowstone Vixen / Codama `rootNode` JSON files here as `*.json`.
Nothing in this directory is fetched by the build or tests — place files manually.

Then run:

```bash
./scripts/inspect-real.sh
```

## Where to get them

### 1. From the Yellowstone Vixen repo (preferred)

Clone or copy from [rpcpool/yellowstone-vixen](https://github.com/rpcpool/yellowstone-vixen).
Codama IDL fixtures live under:

```
tests/idls/*.json
```

As of recent main, that includes (among others):

- `tests/idls/dca.json`
- `tests/idls/dynamic_bonding_curve.json`
- `tests/idls/glow.json`
- `tests/idls/invite_escrow.json`
- `tests/idls/jupiter_log_scope_regression.json`
- `tests/idls/limit_order_v1.json`
- `tests/idls/limit_order_v2.json`
- `tests/idls/loopscale.json`
- `tests/idls/metaplex.json`
- `tests/idls/okx_labs1.json`
- `tests/idls/order_engine.json`
- `tests/idls/perpetuals.json`
- `tests/idls/pump_fun.json`
- `tests/idls/raydium_amm_v4_with_swapv2.json`
- `tests/idls/spl_governance.json`
- `tests/idls/squads_multisig_program.json`

Copy any subset into this directory, e.g.:

```bash
cp /path/to/yellowstone-vixen/tests/idls/order_engine.json fixtures/real/
```

Each file should have `"kind": "rootNode"` / `"standard": "codama"` at the top level.

### 3. From mainnet via `anchor idl fetch` (live program IDL)

Requires the Anchor CLI and RPC access:

```bash
# helpers stamp `address` (legacy IDLs omit it) and run inspect
./scripts/fetch-idl.sh jupoNjAxXgZ4rjzxzPMP4oxduvQsQtZzyknqvzYNrNu limit_order_v1_onchain
./scripts/fetch-idl.sh j1o2qRpjcyUwEvwtcfhEQefh773ZgjxcVRry7LDqg5X limit_order_v2_onchain

# then diff two versions / related programs
cargo run --quiet -- diff \
  fixtures/real/limit_order_v1_onchain.json \
  fixtures/real/limit_order_v2_onchain.json
```

Or raw:

```bash
anchor idl fetch <PROGRAM_ID> --provider.cluster mainnet -o fixtures/real/<name>.json
```

`Idl::from_json` accepts both modern Solana IDL-spec JSON and legacy Anchor 0.x
shapes (`name`/`version` at top level, `publicKey`, `isMut`/`isSigner`, inline
account/event types).

