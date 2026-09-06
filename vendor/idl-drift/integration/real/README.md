# Real IDLs for local validation

Drop real program IDL JSON files here as `*.json` (gitignored). Nothing in this
directory is fetched by the build or by `integration/run-all.sh` over the
network — place files manually, then re-run the suite.

```bash
bash integration/run-all.sh
# or
cargo run --quiet -- inspect integration/real/<file>.json
```

## Where to get them

### 1. Yellowstone Vixen Codama fixtures (preferred)

From [rpcpool/yellowstone-vixen](https://github.com/rpcpool/yellowstone-vixen):

```
tests/idls/*.json
```

Examples:

- `tests/idls/dca.json`
- `tests/idls/dynamic_bonding_curve.json`
- `tests/idls/glow.json`
- `tests/idls/invite_escrow.json`
- `tests/idls/jupiter_log_scope_regression.json`
- `tests/idls/limit_order_v1.json` / `limit_order_v2.json`
- `tests/idls/loopscale.json`
- `tests/idls/metaplex.json`
- `tests/idls/okx_labs1.json`
- `tests/idls/order_engine.json`
- `tests/idls/perpetuals.json`
- `tests/idls/pump_fun.json`
- `tests/idls/raydium_amm_v4_with_swapv2.json`
- `tests/idls/spl_governance.json`
- `tests/idls/squads_multisig_program.json`

```bash
cp /path/to/yellowstone-vixen/tests/idls/order_engine.json integration/real/
```

Expect `"kind": "rootNode"` / `"standard": "codama"`.

### 2. On-chain via `anchor idl fetch`

```bash
./scripts/fetch-idl.sh <PROGRAM_ID> <basename>
# writes fixtures/real/ by default in that helper — copy into integration/real/
# or:
anchor idl fetch <PROGRAM_ID> --provider.cluster mainnet -o integration/real/<name>.json
```

### 3. Programs without a published IDL

Out of scope for this sandbox unless you obtain a structural IDL another way
(e.g. IDLGuesser). Those reconstructions are incomplete by nature — useful for
exploratory diffs only, not as a CI oracle.

Also see `fixtures/real/README.md` for the same sources used by
`scripts/inspect-real.sh`.
