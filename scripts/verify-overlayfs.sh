#!/usr/bin/env sh
set -eu

echo "== kernel =="
uname -a

echo "== identity =="
id

echo "== overlay availability =="
if [ -r /proc/filesystems ]; then
  grep -w overlay /proc/filesystems || true
else
  echo "/proc/filesystems unavailable"
fi

echo "== tempdir filesystem =="
TMPDIR_ROOT="${TMPDIR:-/tmp}"
findmnt -T "$TMPDIR_ROOT" -o TARGET,FSTYPE,OPTIONS || true

echo "== mount capability probe =="
ROOT="$(mktemp -d "${TMPDIR_ROOT%/}/kage-overlay-probe.XXXXXX")"
mkdir -p "$ROOT/lower" "$ROOT/upper" "$ROOT/work" "$ROOT/merged"
echo lower > "$ROOT/lower/file.txt"
if mount -t overlay overlay -o "lowerdir=$ROOT/lower,upperdir=$ROOT/upper,workdir=$ROOT/work,redirect_dir=off" "$ROOT/merged"; then
  echo "overlay mount probe: succeeded"
  cat "$ROOT/merged/file.txt"
  umount "$ROOT/merged"
else
  echo "overlay mount probe: failed (root/CAP_SYS_ADMIN or kernel support likely missing)"
fi
rm -rf "$ROOT"

echo "== running gated overlay tests =="
KAGE_TEST_OVERLAY=1 cargo test --workspace --all-features -- --nocapture
