#!/usr/bin/env bash
# Run every integration/scenarios pair + optional inspect on integration/real/.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

SCEN="$ROOT/integration/scenarios"
PASS=0
FAIL=0
TOTAL=0

# scenario | expected_exit
CASES=(
  "anchor_029_to_030|1"
  "account_removed_collision|1"
  "mid_optional_account_removed|1"
  "cpi_event_anchor_to_pinocchio|1"
  "map_value_widen|1"
  "truncated_idl|2"
)

echo "== idl-drift integration scenarios =="
echo

for entry in "${CASES[@]}"; do
  IFS='|' read -r name expect <<<"$entry"
  TOTAL=$((TOTAL + 1))
  old="$SCEN/$name/old.json"
  new="$SCEN/$name/new.json"

  set +e
  cargo run --quiet -- diff "$old" "$new" \
    >/tmp/idl-drift-integ-out.txt 2>/tmp/idl-drift-integ-err.txt
  code=$?
  set -e

  if [[ "$code" -eq "$expect" ]]; then
    echo "${name}: exit=${code} PASS"
    PASS=$((PASS + 1))
  else
    echo "${name}: exit=${code} FAIL (expected=${expect})"
    echo "----- stdout -----"
    cat /tmp/idl-drift-integ-out.txt || true
    echo "----- stderr -----"
    cat /tmp/idl-drift-integ-err.txt || true
    echo "------------------"
    FAIL=$((FAIL + 1))
  fi
done

echo
echo "== inspect integration/real/*.json (if any) =="
shopt -s nullglob
real_files=(integration/real/*.json)
if ((${#real_files[@]} == 0)); then
  echo "(none present — see integration/real/README.md)"
else
  for f in "${real_files[@]}"; do
    set +e
    out="$(cargo run --quiet -- inspect "$f" 2>&1)"
    icode=$?
    set -e
    base="$(basename "$f")"
    if ((icode == 2)); then
      echo "${base}  ERROR (parse/io)  FAIL"
      echo "$out" >&2
      continue
    fi
    count="$(printf '%s\n' "$out" | sed -n 's/^unmapped (Generic) fields: //p' | head -n1)"
    [[ -z "$count" ]] && count="?"
    if ((icode == 0)); then
      echo "${base}  Generic=${count}  PASS"
    else
      echo "${base}  Generic=${count}  FAIL"
    fi
  done
fi

echo
echo "${PASS}/${TOTAL} scenarios behaved as expected"
if [[ "$FAIL" -ne 0 ]]; then
  exit 1
fi
