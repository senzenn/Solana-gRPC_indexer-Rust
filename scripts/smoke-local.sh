#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

cargo build --bin index
cargo test --quiet
cargo clippy --bin index -- -D warnings

export YELLOWSTONE_ENDPOINT="${YELLOWSTONE_ENDPOINT:-https://geyser.example.invalid}"
cargo run --quiet -- config validate --file index.example.yaml

DEMO_PID=""
cleanup() {
  if [[ -n "${DEMO_PID}" ]] && kill -0 "${DEMO_PID}" 2>/dev/null; then
    kill "${DEMO_PID}" 2>/dev/null || true
    wait "${DEMO_PID}" 2>/dev/null || true
  fi
  rm -f .smoke-index.db
}
trap cleanup EXIT

cargo run --quiet -- demo --db sqlite:./.smoke-index.db --bind 127.0.0.1:18080 &
DEMO_PID=$!
sleep 2

curl -sf http://127.0.0.1:18080/health
curl -sf http://127.0.0.1:18080/metrics | head -5

echo "smoke ok"
