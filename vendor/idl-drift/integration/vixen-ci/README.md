# Vixen CI cron demo (yellowstone-vixen issue #108)

This directory simulates the **diff-and-classify** step of the proposed IDL
watch cron in [rpcpool/yellowstone-vixen#108](https://github.com/rpcpool/yellowstone-vixen/issues/108).

## What #108 wants (simplified)

1. Periodically fetch / discover the current on-chain (or published) IDL for a
   program Vixen parsers cover.
2. Compare it to the IDL the generated parsers were built from (the “stored”
   baseline checked into the repo).
3. **Diff and classify** — if anything Breaking appears, regenerate parsers /
   open a PR; if not, leave parsers alone.

This demo is **only step 3**. There is no network fetch here: `sample_stored.json`
is the baseline, and `sample_fresh_*.json` stand in for a freshly fetched IDL.

## Run

```bash
# Breaking fresh IDL → should print REGENERATION NEEDED and exit 1
bash integration/vixen-ci/demo_cron.sh

# Identical fresh IDL → should print PARSERS OK and exit 0
bash integration/vixen-ci/demo_cron.sh \
  integration/vixen-ci/sample_stored.json \
  integration/vixen-ci/sample_fresh_ok.json
```

Under the hood:

```bash
cargo run --quiet -- diff <stored> <fresh> --json
# exit 0 → PARSERS OK
# exit 1 → REGENERATION NEEDED: <n> breaking changes
# exit 2 → parse/io error (bad artifact; retry)
```

## Samples

| File | Role |
|------|------|
| `sample_stored.json` | Baseline IDL baked into “generated parsers” |
| `sample_fresh_ok.json` | Same layout → OK |
| `sample_fresh_breaking.json` | `State.counter` u64→u128 → Breaking / regenerate |
