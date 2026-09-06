# map_value_widen

## Real-world situation
A reachable account field is a Borsh map (`u32` len + `(key, value)*`). Widening
the value type (`u64` → `u128`) changes every entry’s byte width. Before
structural `Type::Map` support this was a false-negative (Generic compared equal
to itself).

## Expected
- Classification: **Breaking** (Borsh layout on the reachable `balances` field)
- Exit code: **1**
- Map/Set/Tuple structural support is present — this pair is expected to **PASS**.
