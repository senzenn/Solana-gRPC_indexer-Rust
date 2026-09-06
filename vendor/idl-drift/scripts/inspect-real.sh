#!/usr/bin/env bash
# Run `idl-drift inspect` on every Codama IDL dropped into fixtures/real/.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

shopt -s nullglob
files=(fixtures/real/*.json)
if ((${#files[@]} == 0)); then
  echo "0/0 real IDLs fully mapped (no fixtures/real/*.json — see fixtures/real/README.md)"
  exit 0
fi

pass=0
total=0

for f in "${files[@]}"; do
  total=$((total + 1))
  name="$(basename "$f")"
  # Capture output even when inspect exits 1 (unmapped fields).
  set +e
  out="$(cargo run --quiet -- inspect "$f" 2>&1)"
  code=$?
  set -e

  if ((code == 2)); then
    echo "$name  ERROR (parse/io)  FAIL"
    echo "$out" >&2
    continue
  fi

  # Parse "unmapped (Generic) fields: N" from the report.
  count="$(printf '%s\n' "$out" | sed -n 's/^unmapped (Generic) fields: //p' | head -n1)"
  if [[ -z "$count" ]]; then
    count="?"
  fi

  if ((code == 0)); then
    echo "$name  Generic=$count  PASS"
    pass=$((pass + 1))
  else
    echo "$name  Generic=$count  FAIL"
  fi
done

echo "${pass}/${total} real IDLs fully mapped"
