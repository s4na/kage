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

record_status() {
  echo "${1}_status=${2}" >> "$results"
}
run_case() {
  local name="$1"
  shift
  local log="target/ci-logs/${name}.log"
  echo "== $name: $* ==" | tee "$log"
  set +e
  "$@" >> "$log" 2>&1
  local status=$?
  set -e
  record_status "$name" "$status"
  printf '%s_command=%q\n' "$name" "$(printf '%q ' "$@")" >> "$results"
  echo "$name status: $status"
}
run_sudo_case() {
  local name="$1"
  shift
  if ! command -v sudo >/dev/null 2>&1 || ! sudo -n true >/dev/null 2>&1; then
    record_status "$name" 999
    echo "${name}_command=not-attempted-no-passwordless-sudo" >> "$results"
    echo "$name status: 999"
    return
  fi
  local cargo_home="${CARGO_HOME:-$HOME/.cargo}"
  local rustup_home="${RUSTUP_HOME:-$HOME/.rustup}"
  run_case "$name" sudo -E env "PATH=$PATH" "HOME=$HOME" "CARGO_HOME=$cargo_home" "RUSTUP_HOME=$rustup_home" "$@"
}
status_value() {
  local key="$1"
  local value
  value="$(awk -F= -v key="$key" '$1 == key { value=$2 } END { print value }' "$results")"
  printf '%s' "${value:-999}"
}
aggregate_status() {
  local aggregate="$1"
  local nonsudo_var="$2"
  local sudo_var="$3"
  local nonsudo_status
  local sudo_status
  nonsudo_status="$(status_value "$nonsudo_var")"
  sudo_status="$(status_value "$sudo_var")"
  if [[ "$nonsudo_status" == "0" || "$sudo_status" == "0" ]]; then
    echo "${aggregate}_status=0" >> "$results"
  elif [[ "$nonsudo_status" != "999" ]]; then
    echo "${aggregate}_status=$nonsudo_status" >> "$results"
  else
    echo "${aggregate}_status=$sudo_status" >> "$results"
  fi
}

run_case strict_rofs_nonsudo env KAGE_TEST_ROFS=1 cargo test -p kage-rofs --lib -- --nocapture rofs_mount_strict_requires_real_read_only_mount
run_sudo_case strict_rofs_sudo env KAGE_TEST_ROFS=1 cargo test -p kage-rofs --lib -- --nocapture rofs_mount_strict_requires_real_read_only_mount
aggregate_status strict_rofs strict_rofs_nonsudo_status strict_rofs_sudo_status

run_case strict_overlay_nonsudo env KAGE_TEST_OVERLAY=1 cargo test -p kage-git --test backend_trees -- --nocapture overlayfs_backend_tree_matches_fallback_tree_when_enabled
run_sudo_case strict_overlay_sudo env KAGE_TEST_OVERLAY=1 cargo test -p kage-git --test backend_trees -- --nocapture overlayfs_backend_tree_matches_fallback_tree_when_enabled
aggregate_status strict_overlay strict_overlay_nonsudo_status strict_overlay_sudo_status

run_case strict_combined_nonsudo env KAGE_TEST_ROFS=1 KAGE_TEST_OVERLAY=1 cargo test --workspace --all-features -- --nocapture
run_sudo_case strict_combined_sudo env KAGE_TEST_ROFS=1 KAGE_TEST_OVERLAY=1 cargo test --workspace --all-features -- --nocapture
aggregate_status strict_combined strict_combined_nonsudo_status strict_combined_sudo_status

run_case strict_runtime_nonsudo env KAGE_TEST_ROFS=1 KAGE_TEST_OVERLAY=1 KAGE_TEST_RUNTIME=1 cargo test --workspace --all-features -- --nocapture
run_sudo_case strict_runtime_sudo env KAGE_TEST_ROFS=1 KAGE_TEST_OVERLAY=1 KAGE_TEST_RUNTIME=1 cargo test --workspace --all-features -- --nocapture
aggregate_status strict_runtime strict_runtime_nonsudo_status strict_runtime_sudo_status

cat "$results"
