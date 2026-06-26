#!/usr/bin/env sh
set -eu

echo "== kernel =="
uname -a

echo "== os =="
if [ -r /etc/os-release ]; then cat /etc/os-release; fi

echo "== architecture =="
uname -m

echo "== identity =="
id
echo "USER=${USER:-unknown}"

echo "== capability hints =="
if [ -r /proc/self/status ]; then awk '/^Cap(Eff|Prm|Bnd):/ { print }' /proc/self/status; fi

echo "== mount helpers =="
command -v mount || true
command -v fusermount3 || true
command -v fusermount || true

echo "== fuse availability =="
if [ -e /dev/fuse ]; then ls -l /dev/fuse; else echo "/dev/fuse unavailable"; fi

echo "== overlay availability =="
if [ -r /proc/filesystems ]; then grep -w overlay /proc/filesystems || true; fi

echo "== tempdir filesystem =="
TMPDIR_ROOT="${TMPDIR:-/tmp}"
findmnt -T "$TMPDIR_ROOT" -o TARGET,FSTYPE,OPTIONS || true

echo "== direct rofs mount probe =="
echo "KAGE_TEST_ROFS=1 cargo test -p kage-rofs rofs_mount_strict_requires_real_read_only_mount -- --nocapture"
KAGE_TEST_ROFS=1 cargo test -p kage-rofs rofs_mount_strict_requires_real_read_only_mount -- --nocapture

echo "== direct overlay mount probe =="
echo "KAGE_TEST_OVERLAY=1 cargo test -p kage-git overlayfs_backend_tree_matches_fallback_tree_when_enabled -- --nocapture"
KAGE_TEST_OVERLAY=1 cargo test -p kage-git overlayfs_backend_tree_matches_fallback_tree_when_enabled -- --nocapture

echo "== running strict rofs + overlay tests =="
echo "KAGE_TEST_ROFS=1 KAGE_TEST_OVERLAY=1 cargo test --workspace --all-features -- --nocapture"
KAGE_TEST_ROFS=1 KAGE_TEST_OVERLAY=1 cargo test --workspace --all-features -- --nocapture
