#!/usr/bin/env bash
# Simulate yellowstone-vixen issue #108 cron step 3 (diff-and-classify).
# No network: "stored" vs "freshly fetched" are local JSON files.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
STORED="${1:-$DIR/sample_stored.json}"
FRESH="${2:-$DIR/sample_fresh_breaking.json}"

echo "== vixen-ci demo (#108 step 3: diff-and-classify) =="
echo "stored: $STORED"
echo "fresh:  $FRESH"
echo

set +e
JSON_OUT="$(cargo run --quiet -- diff "$STORED" "$FRESH" --json 2>/tmp/idl-drift-vixen-ci.err)"
code=$?
set -e

if [[ "$code" -eq 2 ]]; then
  echo "error: parse/io failure (exit 2)" >&2
  cat /tmp/idl-drift-vixen-ci.err >&2 || true
  exit 2
fi

if [[ "$code" -eq 0 ]]; then
  echo "PARSERS OK"
  exit 0
fi

# Prefer counts.breaking from --json; fall back to summary parsing.
breaking="$(printf '%s\n' "$JSON_OUT" | python3 -c '
import json,sys
try:
    d=json.load(sys.stdin)
    print(d.get("counts",{}).get("breaking", d.get("exit_code",1)))
except Exception:
    print("?")
' 2>/dev/null || echo "?")"

echo "REGENERATION NEEDED: ${breaking} breaking changes"
exit 1
