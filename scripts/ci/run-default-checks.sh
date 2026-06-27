#!/usr/bin/env bash
set -euo pipefail

mkdir -p target/ci-logs
{
  echo "== cargo fmt =="
  cargo fmt --all -- --check
  echo "== cargo clippy =="
  cargo clippy --workspace --all-targets --all-features -- -D warnings
  echo "== cargo test =="
  scripts/ci-test-summary.sh target/ci-logs/default-cargo-test.log
  echo "== git diff --check =="
  git diff --check
} 2>&1 | tee target/ci-logs/default-checks.log
