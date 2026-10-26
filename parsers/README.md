# Compiled parsers

Each `*.json` IDL in this directory becomes a typed parser at build time.
`build.rs` writes one shipstern `include_shipstern_parser!` call per file.

## Add a parser

1. Copy a Codama IDL or an Anchor 0.30+ IDL into this directory.
2. Build again: `cargo build --release`.
3. Run `index parser list`. The program shows as `(compiled)`.

The run adds compiled programs to the Geyser filter.

## Rules

- An IDL file in `--idl-dir` wins over a compiled parser for the same program.
  `parser watch` can then fix a program upgrade without a new build.
  Remove the IDL file to use the compiled parser.
- Names come from the generated Rust code. The instruction is the enum variant
  (`Sell`). Args and account names are snake_case (`min_sol_output`).
- u128 and i128 values are strings, the same as the IDL decoder.
- Named accounts are sorted by name and have no `writable` field.
- A bad IDL fails the build, not the run.

See shipstern `docs/codama-parser-generation.md` for the IDL subset the macro reads.
