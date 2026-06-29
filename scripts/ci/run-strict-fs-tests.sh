#!/usr/bin/env bash
set -euo pipefail

mkdir -p target/ci-proof target/ci-logs
results=target/ci-proof/results.env
: > "$results"
STRICT_TEST_TIMEOUT_SECONDS="${STRICT_TEST_TIMEOUT_SECONDS:-90}"
cleanup_fuse_mounts() {
  findmnt -rn -t fuse,fuse.kage-rofs 2>/dev/null | awk '/kage-rofs/ {print $1}' | while read -r mp; do
    [ -n "$mp" ] || continue
    echo "cleanup kage-rofs mount: $mp"
    fusermount3 -uz "$mp" 2>/dev/null || umount -l "$mp" 2>/dev/null || true
  done
  pkill -f "target/.*/(kage-rofs|kage-cli)|rofs_mount_strict_requires_real_read_only_mount" 2>/dev/null || true
}
trap cleanup_fuse_mounts EXIT INT TERM

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
  echo "== $name: timeout ${STRICT_TEST_TIMEOUT_SECONDS}s $* ==" | tee "$log"
  set +e
  timeout --kill-after=10s --preserve-status "${STRICT_TEST_TIMEOUT_SECONDS}s" "$@" >> "$log" 2>&1
  local status=$?
  set -e
  if [ "$status" -eq 124 ] || [ "$status" -eq 137 ]; then cleanup_fuse_mounts >> "$log" 2>&1 || true; fi
  record_status "$name" "$status"
  record_zero_test_failure "$name"
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

zero_test_log() {
  local log="$1"
  grep -Eq 'running 0 tests|0 passed; 0 failed; .*filtered out' "$log"
}
record_zero_test_failure() {
  local name="$1"
  local log="target/ci-logs/${name}.log"
  if [[ "$(status_value "${name}_status")" == "0" ]] && zero_test_log "$log"; then
    echo "${name}_status=2" >> "$results"
    echo "${name}_error_kind=strict_test_filter_matched_zero_tests" >> "$results"
    echo "${name} zero-test false positive converted to failure" | tee -a "$log"
  fi
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
error_kind_from_log() {
  local log="$1"
  local body
  body="$(tail -n +2 "$log" 2>/dev/null || true)"
  if grep -Eqi 'fuser_backend_unavailable|fuser_mount_error' <<<"$body"; then
    printf '%s' fuser_backend_failure
  elif grep -Eqi 'Invalid argument|os error 22|EINVAL' <<<"$body"; then
    printf '%s' direct_mount_einval
  elif grep -Eqi '/dev/fuse is unavailable|No such file or directory.*/dev/fuse' <<<"$body"; then
    printf '%s' missing_dev_fuse
  elif grep -Eqi 'fuser_first_read_timeout' <<<"$body"; then
    printf '%s' fuser_first_read_timeout
  elif grep -Eqi 'fuser_readdir_timeout' <<<"$body"; then
    printf '%s' fuser_readdir_timeout
  elif grep -Eqi 'fuser_unmount_timeout' <<<"$body"; then
    printf '%s' fuser_unmount_timeout
  elif grep -Eqi 'timed out|timeout|Command exited with non-zero status 124' <<<"$body"; then
    printf '%s' strict_command_timeout
  elif grep -Eqi 'Operation not permitted|os error 1|EPERM|must be superuser|CAP_SYS_ADMIN' <<<"$body"; then
    printf '%s' permission_denied
  elif grep -Eqi 'appears as both a file and as a directory|cannot add to the index|git update-index|tree hash mismatch|assertion.*left.*right' <<<"$body"; then
    printf '%s' git_index_conflict
  else
    printf '%s' unknown
  fi
}
record_rofs_probe_statuses() {
  local nonsudo_status
  local sudo_status
  nonsudo_status="$(status_value strict_rofs_nonsudo_status)"
  sudo_status="$(status_value strict_rofs_sudo_status)"
  echo "kage_rofs_non_sudo_mount_status=$nonsudo_status" >> "$results"
  if [[ "$nonsudo_status" == "0" ]]; then
    echo "kage_rofs_non_sudo_mount_error_kind=none" >> "$results"
  else
    echo "kage_rofs_non_sudo_mount_error_kind=$(error_kind_from_log target/ci-logs/strict_rofs_nonsudo.log)" >> "$results"
  fi
  echo "kage_rofs_sudo_mount_status=$sudo_status" >> "$results"
  if [[ "$sudo_status" == "0" ]]; then
    echo "kage_rofs_sudo_mount_error_kind=none" >> "$results"
  else
    echo "kage_rofs_sudo_mount_error_kind=$(error_kind_from_log target/ci-logs/strict_rofs_sudo.log)" >> "$results"
  fi
}

run_case strict_rofs_nonsudo env KAGE_TEST_ROFS=1 KAGE_ROFS_BACKEND=fuser cargo test -p kage-rofs --lib rofs_mount_strict_requires_real_read_only_mount -- --nocapture --test-threads=1
run_sudo_case strict_rofs_sudo env KAGE_TEST_ROFS=1 KAGE_ROFS_BACKEND=fuser cargo test -p kage-rofs --lib rofs_mount_strict_requires_real_read_only_mount -- --nocapture --test-threads=1
record_rofs_probe_statuses
aggregate_status strict_rofs strict_rofs_nonsudo_status strict_rofs_sudo_status

run_case strict_overlay_nonsudo env KAGE_TEST_OVERLAY=1 cargo test -p kage-git --test backend_trees overlayfs_backend_tree_matches_fallback_tree_when_enabled -- --exact --nocapture --test-threads=1
run_sudo_case strict_overlay_sudo env KAGE_TEST_OVERLAY=1 cargo test -p kage-git --test backend_trees overlayfs_backend_tree_matches_fallback_tree_when_enabled -- --exact --nocapture --test-threads=1
aggregate_status strict_overlay strict_overlay_nonsudo_status strict_overlay_sudo_status

run_case strict_combined_nonsudo env KAGE_TEST_ROFS=1 KAGE_ROFS_BACKEND=fuser KAGE_TEST_OVERLAY=1 cargo test -p kage-git --test backend_trees rofs_overlay_backend_tree_matches_fallback_tree_when_enabled -- --exact --nocapture --test-threads=1
run_sudo_case strict_combined_sudo env KAGE_TEST_ROFS=1 KAGE_ROFS_BACKEND=fuser KAGE_TEST_OVERLAY=1 cargo test -p kage-git --test backend_trees rofs_overlay_backend_tree_matches_fallback_tree_when_enabled -- --exact --nocapture --test-threads=1
aggregate_status strict_combined strict_combined_nonsudo_status strict_combined_sudo_status

run_case strict_runtime_nonsudo env KAGE_TEST_ROFS=1 KAGE_ROFS_BACKEND=fuser KAGE_TEST_OVERLAY=1 KAGE_TEST_RUNTIME=1 cargo test -p kage-cli --test cli strict_rofs_overlay_runtime_smoke_when_enabled -- --exact --nocapture --test-threads=1
run_sudo_case strict_runtime_sudo env KAGE_TEST_ROFS=1 KAGE_ROFS_BACKEND=fuser KAGE_TEST_OVERLAY=1 KAGE_TEST_RUNTIME=1 cargo test -p kage-cli --test cli strict_rofs_overlay_runtime_smoke_when_enabled -- --exact --nocapture --test-threads=1
aggregate_status strict_runtime strict_runtime_nonsudo_status strict_runtime_sudo_status

cat "$results"
