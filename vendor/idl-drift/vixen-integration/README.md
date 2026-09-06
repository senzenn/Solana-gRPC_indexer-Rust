# Vixen fixture integration

Validation scaffolding for [rpcpool/yellowstone-vixen](https://github.com/rpcpool/yellowstone-vixen):
real Codama IDLs under `tests/idls/`, including a true **limit_order v1→v2** pair and
edge-case fixtures that stress disc handling / collisions / inline structs.

**No network in scripts.** Copy JSON into `vixen-integration/idls/` yourself, then:

```bash
bash vixen-integration/run.sh
bash vixen-integration/pre-regen-guard.sh \
  vixen-integration/idls/limit_order_v1.json \
  vixen-integration/idls/limit_order_v2.json
```

`idls/*.json` is gitignored; this README stays tracked.

## Files to place in `idls/`

Raw GitHub URLs (blob/main). Download or `curl -L -o idls/<name> <raw-url>` using the
**raw** host if you prefer:

`https://raw.githubusercontent.com/rpcpool/yellowstone-vixen/main/tests/idls/<name>`

| File | Why it matters | Blob URL |
|------|----------------|----------|
| `limit_order_v1.json` | Real program IDL **v1** (headline before) | https://github.com/rpcpool/yellowstone-vixen/blob/main/tests/idls/limit_order_v1.json |
| `limit_order_v2.json` | Real program IDL **v2** (headline after) | https://github.com/rpcpool/yellowstone-vixen/blob/main/tests/idls/limit_order_v2.json |
| `inline_struct.json` | Inline struct-as-field → likely `Type::Generic` blind spot | https://github.com/rpcpool/yellowstone-vixen/blob/main/tests/idls/inline_struct.json |
| `inline_struct_collisions.json` | Inline structs + disc/account-count collisions | https://github.com/rpcpool/yellowstone-vixen/blob/main/tests/idls/inline_struct_collisions.json |
| `constant_bytes_account.json` | Constant-bytes account disc encoding | https://github.com/rpcpool/yellowstone-vixen/blob/main/tests/idls/constant_bytes_account.json |
| `order_engine.json` | Compact real Codama program (self-diff / inspect baseline) | https://github.com/rpcpool/yellowstone-vixen/blob/main/tests/idls/order_engine.json |
| `pump_fun.json` | Large real program | https://github.com/rpcpool/yellowstone-vixen/blob/main/tests/idls/pump_fun.json |
| `raydium_amm_v4_with_swapv2.json` | Real AMM IDL | https://github.com/rpcpool/yellowstone-vixen/blob/main/tests/idls/raydium_amm_v4_with_swapv2.json |
| `dca.json` | Real DCA program | https://github.com/rpcpool/yellowstone-vixen/blob/main/tests/idls/dca.json |
| `metaplex.json` | Metaplex | https://github.com/rpcpool/yellowstone-vixen/blob/main/tests/idls/metaplex.json |
| `spl_governance.json` | SPL Governance | https://github.com/rpcpool/yellowstone-vixen/blob/main/tests/idls/spl_governance.json |
| `perpetuals.json` | Perpetuals | https://github.com/rpcpool/yellowstone-vixen/blob/main/tests/idls/perpetuals.json |
| `squads_multisig_program.json` | Squads multisig | https://github.com/rpcpool/yellowstone-vixen/blob/main/tests/idls/squads_multisig_program.json |

Optional extras also under `tests/idls/` (same URL pattern): `glow.json`,
`invite_escrow.json`, `loopscale.json`, `okx_labs1.json`,
`dynamic_bonding_curve.json`, `jupiter_log_scope_regression.json`.

### Quick copy from a local Vixen checkout

```bash
cp /path/to/yellowstone-vixen/tests/idls/*.json vixen-integration/idls/
```

## What `run.sh` does

1. **Headline:** `diff limit_order_v1.json limit_order_v2.json` — full classified report + exit code.
2. **Inspect** every other `idls/*.json` — reachable-Generic count + PASS/FAIL.
3. Coverage summary line.

## Pre-regen guard (#108)

See `pre-regen-guard.sh` + its section in this folder’s scripts: Mode 2 “diff before
`cargo insta` regenerate” — [yellowstone-vixen#108](https://github.com/rpcpool/yellowstone-vixen/issues/108) step 3.

## Findings

If the headline diff or inspect surface a missed/misclassified change, capture it in
`FINDINGS.md` (do not silently change `diff.rs`).
