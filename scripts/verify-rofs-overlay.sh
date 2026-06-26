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

echo "== fuse availability =="
if [ -e /dev/fuse ]; then ls -l /dev/fuse; else echo "/dev/fuse unavailable"; fi

echo "== overlay availability =="
if [ -r /proc/filesystems ]; then grep -w overlay /proc/filesystems || true; fi

echo "== tempdir filesystem =="
TMPDIR_ROOT="${TMPDIR:-/tmp}"
findmnt -T "$TMPDIR_ROOT" -o TARGET,FSTYPE,OPTIONS || true

echo "== running strict rofs + overlay tests =="
KAGE_TEST_ROFS=1 KAGE_TEST_OVERLAY=1 cargo test --workspace --all-features -- --nocapture
