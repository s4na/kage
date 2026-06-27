#!/usr/bin/env bash
set -euo pipefail

mkdir -p target/ci-proof target/ci-logs
results=target/ci-proof/results.env
: > "$results"

if [[ -n "${KAGE_TEST_ROFS_ALLOW_SKIP:-}" || -n "${KAGE_TEST_OVERLAY_ALLOW_SKIP:-}" ]]; then
  echo "allow_skip_used=true" >> "$results"
else
  echo "allow_skip_used=false" >> "$results"
fi

run_case() {
  local name="$1"
  shift
  local log="target/ci-logs/${name}.log"
  echo "== $name: $* ==" | tee "$log"
  set +e
  "$@" >> "$log" 2>&1
  local status=$?
  set -e
  echo "${name}_status=${status}" >> "$results"
  echo "${name}_command=$(printf '%q ' "$@")" >> "$results"
  echo "$name status: $status"
}

run_case strict_rofs env KAGE_TEST_ROFS=1 cargo test --workspace --all-features -- --nocapture
run_case strict_overlay env KAGE_TEST_OVERLAY=1 cargo test --workspace --all-features -- --nocapture
run_case strict_combined env KAGE_TEST_ROFS=1 KAGE_TEST_OVERLAY=1 cargo test --workspace --all-features -- --nocapture
run_case strict_runtime env KAGE_TEST_ROFS=1 KAGE_TEST_OVERLAY=1 KAGE_TEST_RUNTIME=1 cargo test --workspace --all-features -- --nocapture

cat "$results"
