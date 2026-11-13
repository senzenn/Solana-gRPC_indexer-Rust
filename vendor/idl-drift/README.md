# idl-drift (vendored)

Diffs two Solana program IDLs and classifies each change by whether a parser
generated from the old IDL would misread bytes. Vendored so the indexer builds
from a plain clone. The indexer uses it in `parser new`, `parser fetch`,
`parser watch` and `parser diff`.

Input: Codama `rootNode` JSON or Anchor IDL JSON.

```bash
cargo run -- diff old.json new.json
```

Exit codes: `0` no breaking change, `1` at least one breaking change, `2` I/O or parse error.

| Tier | Meaning | Examples |
|------|---------|----------|
| **Breaking** | Parser misreads bytes or fails to route | Field width change on a reachable type; enum variant inserted mid-list; discriminator length change; arg added or removed; instruction removed; type-alias target change |
| **Dangerous** | Old tags and positions hold, but parsers must grow | Enum variant appended; account appended to an instruction; optional tail account removed |
| **Additive** | Old parsers are not affected | New instruction, account or event type |
| **Cosmetic** | No parser impact | Change on a type no instruction arg, account or event reaches |

Reachability starts at account and event names and `Defined` refs in
instruction args, then follows struct fields.

Limit: it compares two IDL documents only. It cannot see a program upgraded
on-chain without an IDL update.

License: MIT OR Apache-2.0
