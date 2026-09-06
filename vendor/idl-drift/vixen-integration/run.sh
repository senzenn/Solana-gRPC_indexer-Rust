#!/usr/bin/env bash
# Run idl-drift against Vixen tests/idls fixtures copied into idls/.
# No network — populate idls/ first (see README.md).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

IDLS="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/idls"
V1="$IDLS/limit_order_v1.json"
V2="$IDLS/limit_order_v2.json"

if [[ ! -f "$V1" || ! -f "$V2" ]]; then
  echo "error: missing $V1 and/or $V2" >&2
  echo "Copy Vixen fixtures into vixen-integration/idls/ (see README.md)." >&2
  exit 2
fi

echo "================================================================"
echo "HEADLINE: limit_order v1 -> v2 (real Vixen Codama pair)"
echo "================================================================"
set +e
cargo run -q -- diff "$V1" "$V2"
headline_code=$?
set -e
echo "exit code: ${headline_code}"
echo

echo "================================================================"
echo "INSPECT: every other idls/*.json"
echo "================================================================"

shopt -s nullglob
pass=0
fail=0
err=0
total=0
generic_hits=()

for f in "$IDLS"/*.json; do
  base="$(basename "$f")"
  if [[ "$base" == "limit_order_v1.json" || "$base" == "limit_order_v2.json" ]]; then
    # Still inspect the pair for Generic coverage, but label clearly.
    :
  fi
  total=$((total + 1))
  set +e
  out="$(cargo run -q -- inspect "$f" 2>&1)"
  code=$?
  set -e

  count="$(printf '%s\n' "$out" | sed -n 's/^unmapped (Generic) fields: //p' | head -n1)"
  [[ -z "$count" ]] && count="?"

  note=""
  case "$base" in
    inline_struct.json)
      note="  # expect Generic UNLESS inline-struct mapping landed"
      ;;
    inline_struct_collisions.json)
      note="  # collisions + inline Generic caveat"
      ;;
    constant_bytes_account.json)
      note="  # expect clean disc handling (Generic=0)"
      ;;
  esac

  if ((code == 2)); then
    echo "${base}  ERROR (parse/io)  FAIL${note}"
    echo "$out" >&2
    err=$((err + 1))
    fail=$((fail + 1))
  elif ((code == 0)); then
    echo "${base}  Generic=${count}  PASS${note}"
    pass=$((pass + 1))
  else
    echo "${base}  Generic=${count}  FAIL${note}"
    printf '%s\n' "$out" | grep '^UNMAPPED:' || true
    fail=$((fail + 1))
    generic_hits+=("${base}:${count}")
  fi
done

echo
echo "----------------------------------------------------------------"
echo "coverage: ${pass}/${total} inspect-clean (Generic=0); ${fail} with gaps/errors; headline exit=${headline_code}"
if ((${#generic_hits[@]} > 0)); then
  echo "reachable-Generic hits: ${generic_hits[*]}"
fi
if ((total == 0)); then
  echo "coverage: 0/0 — idls/ empty"
fi
