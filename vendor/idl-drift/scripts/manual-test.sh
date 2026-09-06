#!/usr/bin/env bash
# End-to-end CLI driver for fixtures/manual/.
# Runs `cargo run --quiet -- diff <old> <new>` for each pair and checks exit codes.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

FIX="$ROOT/fixtures/manual"
PASS=0
FAIL=0
TOTAL=0

# pair_name | old | new | expected_exit
CASES=(
  "00_identical|00_identical_old.json|00_identical_new.json|0"
  "01_default_omission|01_default_omission_old.json|01_default_omission_new.json|0"
  "02_new_instruction|02_new_instruction_old.json|02_new_instruction_new.json|0"
  "03_enum_append|03_enum_append_old.json|03_enum_append_new.json|0"
  "10_field_widen|10_field_widen_old.json|10_field_widen_new.json|1"
  "11_enum_mid_insert|11_enum_mid_insert_old.json|11_enum_mid_insert_new.json|1"
  "12_arg_added|12_arg_added_old.json|12_arg_added_new.json|1"
  "13_account_reorder|13_account_reorder_old.json|13_account_reorder_new.json|1"
  "14_disc_len_change|14_disc_len_change_old.json|14_disc_len_change_new.json|1"
  "15_alias_retarget|15_alias_retarget_old.json|15_alias_retarget_new.json|1"
  "16_collision_introduced|16_collision_introduced_old.json|16_collision_introduced_new.json|1"
  "17_disc_reencoded|17_disc_reencoded_old.json|17_disc_reencoded_new.json|0"
  "90_broken_json|90_broken_json_old.json|00_identical_new.json|2"
  "91_missing_field|91_missing_field_old.json|00_identical_new.json|2"
)

echo "== idl-drift manual CLI suite =="
echo

for entry in "${CASES[@]}"; do
  IFS='|' read -r name old new expect <<<"$entry"
  TOTAL=$((TOTAL + 1))
  set +e
  cargo run --quiet -- diff "$FIX/$old" "$FIX/$new" >/tmp/idl-drift-manual-out.txt 2>/tmp/idl-drift-manual-err.txt
  code=$?
  set -e

  if [[ "$code" -eq "$expect" ]]; then
    echo "${name}: exit=${code} PASS"
    PASS=$((PASS + 1))
  else
    echo "${name}: exit=${code} FAIL (expected=${expect})"
    echo "----- stdout -----"
    cat /tmp/idl-drift-manual-out.txt || true
    echo "----- stderr -----"
    cat /tmp/idl-drift-manual-err.txt || true
    echo "------------------"
    FAIL=$((FAIL + 1))
  fi
done

echo
echo "${PASS}/${TOTAL} pairs behaved as expected"
if [[ "$FAIL" -ne 0 ]]; then
  exit 1
fi
