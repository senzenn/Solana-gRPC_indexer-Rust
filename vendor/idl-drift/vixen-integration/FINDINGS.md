# Findings — Vixen limit_order v1→v2 (Codama fixtures)

Captured against `yellowstone-vixen/tests/idls/limit_order_v1.json` →
`limit_order_v2.json` via `idl-drift diff`. **No classification logic was
changed** — these are review notes for messaging / future work.

## Headline result (sensible overall)

```
summary: 96 breaking, 10 dangerous, 2 additive, 0 cosmetic
exit code: 1
```

Major real breaks the engine **does** surface:

- Massive **positional account meta** churn on `cancelOrder` / `fillOrder` /
  `flashFillOrder` / `initializeOrder` / …
- **Account struct `order` layout rewrite** (fields inserted/reordered/widened)
- **Fee account** field shrink (`u64`×4 → `u16`×2)
- **Event payload** growth (`createOrderEvent`, `tradeEvent` renames + adds)
- Instructions removed (`cancelExpiredOrder`, `initFee`) and added
  (`cancelDustOrder` + event)
- Dangerous: new accounts appended (min-accounts / position shifts)

Program IDs also differ (`jupo…` vs `j1o2…`) — this is effectively a successor
program, not an in-place upgrade; the classified output is still a useful
parser-compatibility report.

## Gap A — messages omit field/arg **names** (looks like false positives)

Several Breaking lines render as type→same-type, e.g.:

```text
[BREAKING] arg changed (fillOrder#0): u64 -> u64 (Borsh layout)
[BREAKING] FIELD changed (order#6) via reachable type: u64 -> u64 (Borsh layout)
[BREAKING] FIELD changed (tradeEvent#2) via reachable type: u64 -> u64 (Borsh layout)
```

**Not a silent miss:** `Field` / arg equality includes the **name**, so a rename
at a fixed index correctly trips Breaking. The message only prints `tystr(old)` /
`tystr(new)`, so humans see a no-op.

### Excerpt — `fillOrder` args (Codama `arguments`, disc omitted)

v1:

```json
{ "name": "makingAmount", "type": { "kind": "numberTypeNode", "format": "u64", "endian": "le" } }
```

v2:

```json
{ "name": "inputAmount", "type": { "kind": "numberTypeNode", "format": "u64", "endian": "le" } }
```

### Excerpt — `tradeEvent` fields #2 (rename, same u64)

v1: `"name": "remainingInAmount"` · v2: `"name": "remainingMakingAmount"`
(both `numberTypeNode` / `u64` / `le`).

**Suggested follow-up (not done here):** include field/arg names in the
Breaking message, e.g. `makingAmount:u64 -> inputAmount:u64`.

## Gap B — inline struct still Generic (fixture may be absent locally)

`inline_struct.json` / `inline_struct_collisions.json` are listed in the README
for download from Vixen `main`. This local Vixen checkout did **not** ship them
under `tests/idls/`. When present, expect reachable `Generic("structTypeNode")`
unless/until `Type::InlineStruct` lands — documented residual gap in
`convert_type`.

## Inspect coverage (fixtures present locally)

| Result | Fixtures |
|--------|----------|
| Generic=0 (clean) | `dca`, `dynamic_bonding_curve`, `glow`, `invite_escrow`, `jupiter_log_scope_regression`, `limit_order_v1/v2`, `loopscale`, `metaplex`, `okx_labs1`, `order_engine`, `pump_fun`, `raydium_amm_v4_with_swapv2`, `spl_governance`, `squads_multisig_program` |
| Reachable Generic | `perpetuals.json` — 4× `Generic("f32")` on `priceImpactExponent` / `priceImpactBuffer.exponent` (by design: F32 opaque) |

`constant_bytes_account.json` / `inline_struct*.json`: not in this checkout;
re-run `run.sh` after downloading from the README URLs.
