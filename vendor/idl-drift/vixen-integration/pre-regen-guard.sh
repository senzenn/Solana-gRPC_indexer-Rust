#!/usr/bin/env bash
# Mode 2 pre-regeneration guard (#108 step 3).
# Slot this BEFORE Vixen `cargo insta` / parser regeneration:
#   SAFE TO REGENERATE          → exit 0 (no Breaking)
#   BREAKING — DO NOT REGENERATE → exit 1
# No network — pass two local IDL paths.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if [[ $# -lt 2 ]]; then
  echo "usage: $0 <OLD.json> <NEW.json>" >&2
  echo "example: $0 vixen-integration/idls/limit_order_v1.json vixen-integration/idls/limit_order_v2.json" >&2
  exit 2
fi

OLD="$1"
NEW="$2"

echo "== pre-regen guard (Vixen #108 step 3) =="
echo "old: $OLD"
echo "new: $NEW"
echo

set +e
JSON_OUT="$(cargo run -q -- diff "$OLD" "$NEW" --json 2>/tmp/idl-drift-pre-regen.err)"
code=$?
set -e

if [[ "$code" -eq 2 ]]; then
  echo "error: parse/io failure (exit 2) — do not regenerate on a bad artifact" >&2
  cat /tmp/idl-drift-pre-regen.err >&2 || true
  exit 2
fi

if [[ "$code" -eq 0 ]]; then
  echo "SAFE TO REGENERATE"
  exit 0
fi

breaking="$(printf '%s\n' "$JSON_OUT" | python3 -c '
import json,sys
try:
    d=json.load(sys.stdin)
    print(d.get("counts",{}).get("breaking", "?"))
except Exception:
    print("?")
' 2>/dev/null || echo "?")"

echo "BREAKING — DO NOT REGENERATE: ${breaking} changes"
exit 1
