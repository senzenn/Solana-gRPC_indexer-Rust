# idl-drift

Semantic drift detection for Solana program IDLs — flags parser-breaking
changes before they silently corrupt data. Vixen-compatible.

## Problem

Programs upgrade. IDLs drift. Generated parsers keep reading the old layout.

Yellowstone Vixen generates account/instruction/event parsers from a Codama
IDL (`include_vixen_parser!`). Those parsers assume positional accounts,
discriminator tag-dispatch, and Borsh field widths that match the IDL they
were built from. When the IDL changes — `u16` → `u32`, a middle enum insert,
an 8-byte event envelope → 1-byte Pinocchio wrap — a stale parser still
compiles and still runs. It just misreads bytes.

See [rpcpool/yellowstone-vixen#108](https://github.com/rpcpool/yellowstone-vixen/issues/108).

`idl-drift` diffs two IDL versions and classifies every change by whether a
Vixen-generated parser would silently corrupt data.

## Usage

```bash
cargo run -- diff old.json new.json
# or, once installed:
idl-drift diff old.json new.json
```

Accepts Codama `RootNode` JSON (the shape Vixen loads) or Anchor / Solana
IDL-spec JSON.

**Exit codes** (oasdiff-style CI contract):

| Code | Meaning |
|------|---------|
| `0`  | No breaking changes |
| `1`  | At least one `Breaking` change |
| `2`  | I/O or parse error |

Wire into CI:

```bash
idl-drift diff idl/baseline.json idl/current.json
```

## Severity tiers

| Tier | Meaning | Examples |
|------|---------|----------|
| **Breaking** | Parser will misread bytes or fail to route. Fail the build. | Field width change on a reachable type (`u16`→`u32`); enum variant inserted mid-list (tag shift); instruction/account/event discriminator length change (scheme swap); self-CPI event envelope 8-byte→1-byte; arg add/remove; instruction removed; type-alias target change (`Amount=u64`→`u32`) |
| **Dangerous** | Existing tags/positions intact, but parsers must grow. Review. | Enum variant *appended* (new tag only); new account appended to an instruction (`check_min_accounts_req` expects more); optional account removed from the tail |
| **Additive** | Purely additive surface; old parsers unaffected. | New instruction; new account/event type |
| **Cosmetic** | No parser impact (unreachable / dead types). | Field or serialization change on a type not reachable from any instruction arg, account, or event |

Reachability is computed from account/event names and `Defined` refs in
instruction args, then closed transitively through struct fields. Changes to
unreached types stay Cosmetic even when the layout would otherwise be Breaking.

## idl-drift vs. `deserialize_checked`

These are complementary halves of the same chain, not competitors
([yellowstone-vixen#108](https://github.com/rpcpool/yellowstone-vixen/issues/108)).
Vixen already ships a first-party `deserialize_checked` that alerts at **runtime**
when a live IDL no longer matches the compiled parser. `idl-drift` is the
**pre-merge** half: offline, no RPC, fail CI before a breaking IDL lands.

| | **idl-drift** | **`deserialize_checked` (Vixen)** |
|---|---|---|
| When | CI / pre-merge | Runtime / post-deploy |
| Input | Two local IDL JSON files | Live IDL vs compiled parser |
| Network | Offline | Needs the IDL source at decode time |
| Failure mode | Exit 1 — block the merge | Alert on a stale-IDL transaction |

## Limitations

**Declared-IDL drift only.** `idl-drift` compares two IDL documents. It cannot
detect IDL-vs-deployed-bytecode divergence.

A program can be upgraded on-chain without updating its IDL account. The
validator (Agave) executes bytecode, not IDLs — so a parser built from a
stale IDL will happily decode live transactions against a different layout,
and this tool will not see it.

File-first (diff two local JSON files) is intentional: some real-world IDLs
aren't on-chain or public — they're built from program source — so an
RPC-only tool would miss part of a maintainer's fleet (#108).

Pair with transaction-replay validation (fixtures / historical txs against
the generated parser) and Vixen's `deserialize_checked` to catch the
runtime / bytecode classes of failure.

## Oracle

Classification rules were ported from the awk prototype in `proto/`
(`proto/diff.awk` + `*.facts` fixtures). That suite is the reference oracle;
the Rust engine is expected to match it case-for-case.

## License

MIT OR Apache-2.0
