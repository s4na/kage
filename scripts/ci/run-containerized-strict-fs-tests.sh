#!/usr/bin/env bash
set -euo pipefail

mkdir -p target/ci-proof target/ci-logs
for log in \
  target/ci-logs/container_probe.log \
  target/ci-logs/container_strict_rofs.log \
  target/ci-logs/container_strict_overlay.log \
  target/ci-logs/container_strict_combined.log \
  target/ci-logs/container_strict_runtime.log; do
  : > "$log"
done
container_env=target/ci-proof/container.env
host_container_env=target/ci-proof/container-host.env
: > "$container_env"
: > "$host_container_env"
STRICT_TEST_TIMEOUT_SECONDS="${STRICT_TEST_TIMEOUT_SECONDS:-90}"
CONTAINER_STRICT_TIMEOUT_SECONDS="${CONTAINER_STRICT_TIMEOUT_SECONDS:-600}"
CIDFILE="target/ci-proof/container.cid"
rm -f "$CIDFILE"
cleanup_container() {
  if [ -f "$CIDFILE" ]; then docker rm -f "$(cat "$CIDFILE")" >/dev/null 2>&1 || true; rm -f "$CIDFILE"; fi
}
trap cleanup_container EXIT INT TERM
IMAGE="${KAGE_CONTAINER_STRICT_IMAGE:-kage-strict:ci}"
DOCKERFILE="${KAGE_CONTAINER_STRICT_DOCKERFILE:-.github/docker/kage-strict/Dockerfile}"

echo "containerized_strict_attempted=true" >> "$host_container_env"
echo "containerized_strict_image=$IMAGE" >> "$host_container_env"
echo "containerized_privileged_attempted=true" >> "$host_container_env"

if ! command -v docker >/dev/null 2>&1; then
  echo "containerized_docker_available=false" >> "$host_container_env"
  echo "docker unavailable; containerized strict proof not attempted" | tee target/ci-logs/container_probe.log
  exit 0
fi
echo "containerized_docker_available=true" >> "$host_container_env"

set +e
docker build -f "$DOCKERFILE" -t "$IMAGE" . 2>&1 | tee target/ci-logs/container_image_build.log
build_status=${PIPESTATUS[0]}
set -e
echo "containerized_image_build_status=$build_status" >> "$host_container_env"
if [ "$build_status" -ne 0 ]; then
  echo "container strict image build failed with status $build_status; see target/ci-logs/container_image_build.log" | tee -a target/ci-logs/container_probe.log
  exit 0
fi

container_payload='set -euo pipefail
mkdir -p /out/logs /out/proof /work/kage
: > /out/proof/container.env
record() { printf "%s=%s\n" "$1" "$2" >> /out/proof/container.env; }
record containerized_strict_attempted true
record containerized_privileged_attempted true
record containerized_strict_image "${KAGE_CONTAINER_STRICT_IMAGE:-unknown}"
status_value=999
zero_test_log() { grep -Eq "running 0 tests|0 passed; 0 failed; .*filtered out" "$1"; }
error_kind_from_log() {
  local log="$1"
  if zero_test_log "$log"; then echo strict_test_filter_matched_zero_tests
  elif grep -Eqi "fuser_backend_unavailable|fuser_mount_error" "$log"; then echo fuser_backend_failure
  elif grep -Eqi "Invalid argument|os error 22|EINVAL" "$log"; then echo direct_mount_einval
  elif grep -Eqi "/dev/fuse is unavailable|No such file or directory.*/dev/fuse" "$log"; then echo missing_dev_fuse
  elif grep -Eqi "fuser_first_read_timeout" "$log"; then echo fuser_first_read_timeout
  elif grep -Eqi "fuser_readdir_timeout" "$log"; then echo fuser_readdir_timeout
  elif grep -Eqi "fuser_unmount_timeout" "$log"; then echo fuser_unmount_timeout
  elif grep -Eqi "timed out|timeout|Command exited with non-zero status 124" "$log"; then echo strict_command_timeout
  elif grep -Eqi "Operation not permitted|os error 1|EPERM|must be superuser|CAP_SYS_ADMIN|permission denied" "$log"; then echo permission_denied
  elif grep -Eqi "appears as both a file and as a directory|cannot add to the index|git update-index|tree hash mismatch|assertion.*left.*right" "$log"; then echo tree_mismatch
  else echo unknown
  fi
}
run_strict() {
  local name="$1"
  shift
  local log="/out/logs/container_strict_${name}.log"
  echo "== container ${name}: timeout ${STRICT_TEST_TIMEOUT_SECONDS}s $* ==" | tee "$log"
  set +e
  timeout --kill-after=10s --preserve-status "${STRICT_TEST_TIMEOUT_SECONDS}s" "$@" >> "$log" 2>&1
  local st=$?
  set -e
  if [ "$st" -eq 124 ] || [ "$st" -eq 137 ]; then findmnt -rn -t fuse,fuse.kage-rofs 2>/dev/null | awk '/kage-rofs/ {print $1}' | while read -r mp; do fusermount3 -uz "$mp" 2>/dev/null || umount -l "$mp" 2>/dev/null || true; done >> "$log" 2>&1 || true; fi
  record "containerized_${name}_attempted" true
  if [ "$st" -eq 0 ] && zero_test_log "$log"; then
    st=2
    echo "container ${name} zero-test false positive converted to failure" | tee -a "$log"
  fi
  record "containerized_${name}_status" "$st"
  if [ "$st" -eq 0 ]; then
    record "containerized_${name}_passed" true
    record "containerized_${name}_error_kind" none
  else
    record "containerized_${name}_passed" false
    record "containerized_${name}_error_kind" "$(error_kind_from_log "$log")"
  fi
  echo "container ${name} status: $st"
}
{
  echo "== container probe =="
  uname -a || true
  id || true
  cat /proc/self/mountinfo | head -20 || true
  ls -l /dev/fuse || true
  if test -r /dev/fuse && test -w /dev/fuse; then record containerized_dev_fuse_readable_writable true; else record containerized_dev_fuse_readable_writable false; fi
  if test -e /dev/fuse; then record containerized_dev_fuse_exists true; else record containerized_dev_fuse_exists false; fi
  if command -v fusermount3 >/dev/null 2>&1; then record containerized_fusermount3_available true; command -v fusermount3; fusermount3 --version || true; else record containerized_fusermount3_available false; fi
  if command -v fuse-overlayfs >/dev/null 2>&1; then record containerized_fuse_overlayfs_available true; command -v fuse-overlayfs; fuse-overlayfs --version || true; else record containerized_fuse_overlayfs_available false; fi
  if grep -qw overlay /proc/filesystems; then record containerized_overlay_available true; else record containerized_overlay_available false; fi
  capsh --print || true
  probe_root="$(mktemp -d)"
  mkdir -p "$probe_root/lower" "$probe_root/upper" "$probe_root/work" "$probe_root/merged"
  echo lower > "$probe_root/lower/file.txt"
  set +e
  mount -t overlay overlay -o "lowerdir=$probe_root/lower,upperdir=$probe_root/upper,workdir=$probe_root/work" "$probe_root/merged"
  overlay_status=$?
  set -e
  record containerized_overlay_mount_status "$overlay_status"
  if [ "$overlay_status" -eq 0 ]; then findmnt "$probe_root/merged" || true; umount "$probe_root/merged" || true; fi
  rm -rf "$probe_root"
  fuse_root="$(mktemp -d)"
  mkdir -p "$fuse_root/lower" "$fuse_root/upper" "$fuse_root/work" "$fuse_root/merged"
  echo lower > "$fuse_root/lower/file.txt"
  set +e
  fuse-overlayfs -o "lowerdir=$fuse_root/lower,upperdir=$fuse_root/upper,workdir=$fuse_root/work" "$fuse_root/merged"
  fuse_status=$?
  set -e
  record containerized_fuse_overlayfs_status "$fuse_status"
  if [ "$fuse_status" -eq 0 ]; then findmnt "$fuse_root/merged" || true; fusermount3 -u "$fuse_root/merged" || umount "$fuse_root/merged" || true; fi
  rm -rf "$fuse_root"
} 2>&1 | tee /out/logs/container_probe.log

tar -C /src --exclude=target -cf - . | tar -C /work/kage -xf -
cd /work/kage
cargo --version | tee -a /out/logs/container_probe.log
rustc --version | tee -a /out/logs/container_probe.log
run_strict rofs env KAGE_TEST_ROFS=1 KAGE_ROFS_BACKEND=fuser cargo test -p kage-rofs --lib rofs_mount_strict_requires_real_read_only_mount -- --nocapture --test-threads=1
run_strict overlay env KAGE_TEST_OVERLAY=1 cargo test -p kage-git --test backend_trees overlayfs_backend_tree_matches_fallback_tree_when_enabled -- --exact --nocapture --test-threads=1
run_strict combined env KAGE_TEST_ROFS=1 KAGE_ROFS_BACKEND=fuser KAGE_TEST_OVERLAY=1 cargo test -p kage-git --test backend_trees rofs_overlay_backend_tree_matches_fallback_tree_when_enabled -- --exact --nocapture --test-threads=1
run_strict runtime env KAGE_TEST_ROFS=1 KAGE_ROFS_BACKEND=fuser KAGE_TEST_OVERLAY=1 KAGE_TEST_RUNTIME=1 cargo test -p kage-cli --test cli strict_rofs_overlay_runtime_smoke_when_enabled -- --exact --nocapture --test-threads=1
cat /out/proof/container.env
'

run_container() {
  timeout --kill-after=10s "${CONTAINER_STRICT_TIMEOUT_SECONDS}s" docker run --cidfile "$CIDFILE" \
    --privileged \
    --device /dev/fuse \
    --cap-add SYS_ADMIN \
    "$@" \
    --tmpfs /tmp:exec,size=2g \
    -e STRICT_TEST_TIMEOUT_SECONDS="$STRICT_TEST_TIMEOUT_SECONDS" \
    -e KAGE_CONTAINER_STRICT_IMAGE="$IMAGE" \
    -v "$PWD:/src:ro" \
    -v "$PWD/target/ci-logs:/out/logs" \
    -v "$PWD/target/ci-proof:/out/proof" \
    "$IMAGE" \
    bash -lc "$container_payload"
}

set +e
run_container --security-opt apparmor=unconfined
status=$?
set -e
if [ "$status" -ne 0 ]; then
  echo "containerized privileged run with apparmor=unconfined exited $status; retrying without apparmor" | tee -a target/ci-logs/container_probe.log
  echo "containerized_apparmor_unconfined_status=$status" >> "$host_container_env"
  set +e
  cleanup_container
  run_container
  status=$?
  set -e
fi
echo "containerized_docker_run_status=$status" >> "$host_container_env"
if [ -f target/ci-proof/container.env ]; then
  # Container writes this file via /out/proof; keep it as the canonical container result.
  true
fi
exit 0
