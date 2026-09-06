#!/usr/bin/env bash
# Fetch an on-chain Anchor IDL and drop it into fixtures/real/ for inspect/diff.
# Usage: ./scripts/fetch-idl.sh <PROGRAM_ID> [outfile_basename]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if [[ $# -lt 1 ]]; then
  echo "usage: $0 <PROGRAM_ID> [outfile_basename]" >&2
  exit 2
fi

PID="$1"
OUT_BASE="${2:-${PID}}"
OUT="fixtures/real/${OUT_BASE}.json"
mkdir -p fixtures/real

echo "anchor idl fetch ${PID} --provider.cluster mainnet -o ${OUT}"
anchor idl fetch "$PID" --provider.cluster mainnet -o "$OUT"

# Legacy Anchor IDLs omit address; stamp the fetched program id.
python3 - "$OUT" "$PID" <<'PY'
import json, sys
path, pid = sys.argv[1], sys.argv[2]
with open(path) as f:
    data = json.load(f)
data["address"] = pid
with open(path, "w") as f:
    json.dump(data, f, indent=2)
    f.write("\n")
print(f"stamped address={pid}")
PY

echo "fetched -> ${OUT}"
cargo run --quiet -- inspect "$OUT"
echo "exit=$?"
