#!/usr/bin/env bash
set -euo pipefail

mkdir -p target/ci-proof target/ci-logs
log=target/ci-logs/fs-probe.log
env_file=target/ci-proof/env.sh
: > "$log"

record() {
  printf '%s=%q\n' "$1" "$2" >> "$env_file"
}
run_log() {
  echo "== $* ==" | tee -a "$log"
  "$@" 2>&1 | tee -a "$log" || true
}

: > "$env_file"
record runner_os "${RUNNER_OS:-$(uname -s)}"
record runner_arch "${RUNNER_ARCH:-$(uname -m)}"
record runner_name "${RUNNER_NAME:-unknown}"
record image_label "${ImageOS:-${ImageVersion:-unknown}}"
record kernel "$(uname -a)"
record os_release "$(tr '\n' ';' < /etc/os-release 2>/dev/null || true)"
record id "$(id)"
record groups "$(groups || true)"
record sudo_available "$(command -v sudo >/dev/null 2>&1 && echo true || echo false)"
record dev_fuse_exists "$([ -e /dev/fuse ] && echo true || echo false)"
record dev_fuse_rw "$([ -r /dev/fuse ] && [ -w /dev/fuse ] && echo true || echo false)"
record overlay_listed "$(grep -qw overlay /proc/filesystems 2>/dev/null && echo true || echo false)"
record mount_path "$(command -v mount || true)"
record findmnt_path "$(command -v findmnt || true)"
record docker_version "$(docker --version 2>/dev/null || true)"

run_log uname -a
run_log cat /etc/os-release
run_log id
run_log groups
run_log command -v mount
run_log command -v findmnt
run_log ls -l /dev/fuse
run_log cat /proc/filesystems
run_log findmnt -T .
if command -v capsh >/dev/null 2>&1; then run_log capsh --print; fi
if command -v docker >/dev/null 2>&1; then run_log docker --version; fi

set +e
{
  echo "== apt-get install filesystem prerequisites =="
  if command -v sudo >/dev/null 2>&1; then sudo apt-get update && sudo apt-get install -y fuse3 libfuse3-dev pkg-config build-essential attr util-linux fuse-overlayfs; else apt-get update && apt-get install -y fuse3 libfuse3-dev pkg-config build-essential attr util-linux fuse-overlayfs; fi
} >> "$log" 2>&1
apt_status=$?
set -e
record apt_status "$apt_status"
record fuse3_installed "$(command -v fusermount3 >/dev/null 2>&1 && echo true || echo false)"
record fuse_overlayfs_installed "$(command -v fuse-overlayfs >/dev/null 2>&1 && echo true || echo false)"

# Direct overlay probe. Failure is diagnostic, not immediately fatal.
probe_root="$(mktemp -d)"
mkdir -p "$probe_root/lower" "$probe_root/upper" "$probe_root/work" "$probe_root/merged"
echo probe > "$probe_root/lower/file.txt"
set +e
mount -t overlay overlay -o "lowerdir=$probe_root/lower,upperdir=$probe_root/upper,workdir=$probe_root/work,redirect_dir=off" "$probe_root/merged" >> "$log" 2>&1
overlay_probe_status=$?
if [ "$overlay_probe_status" -eq 0 ]; then
  umount "$probe_root/merged" >> "$log" 2>&1 || true
fi
set -e
record overlay_mount_probe_status "$overlay_probe_status"
rm -rf "$probe_root"

cat "$env_file" | tee -a "$log"
