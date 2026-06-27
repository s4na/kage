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
command_status() {
  set +e
  "$@" >> "$log" 2>&1
  local status=$?
  set -e
  printf '%s' "$status"
}
has_cap_sys_admin() {
  grep -Eq '^Current:.*cap_sys_admin([,= ]|$)' <<< "$1" && echo true || echo false
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
record fusermount3_path "$(command -v fusermount3 || true)"
record fuse_overlayfs_path "$(command -v fuse-overlayfs || true)"
record docker_version "$(docker --version 2>/dev/null || true)"

run_log uname -a
run_log cat /etc/os-release
run_log id
run_log groups
run_log command -v mount
run_log command -v findmnt
run_log command -v fusermount3
run_log command -v fuse-overlayfs
run_log ls -l /dev/fuse
run_log cat /proc/filesystems
run_log findmnt -T .
if command -v fusermount3 >/dev/null 2>&1; then
  run_log fusermount3 --version
  run_log ls -l "$(command -v fusermount3)"
fi
if [ -f /etc/fuse.conf ]; then
  echo "== /etc/fuse.conf non-comment lines ==" | tee -a "$log"
  sed -n '/^[[:space:]]*#/d; /^[[:space:]]*$/d; p' /etc/fuse.conf | tee -a "$log" || true
  record fuse_conf_present true
  record fuse_conf_user_allow_other "$(sed -n '/^[[:space:]]*#/d; /^[[:space:]]*user_allow_other[[:space:]]*$/p' /etc/fuse.conf | grep -q user_allow_other && echo true || echo false)"
else
  record fuse_conf_present false
  record fuse_conf_user_allow_other false
fi
if command -v fuse-overlayfs >/dev/null 2>&1; then run_log fuse-overlayfs --version; fi
runner_capsh="$(capsh --print 2>/dev/null || true)"
record runner_has_cap_sys_admin "$(has_cap_sys_admin "$runner_capsh")"
if command -v capsh >/dev/null 2>&1; then run_log capsh --print; fi
sudo_status=999
sudo_id=""
sudo_capsh=""
if command -v sudo >/dev/null 2>&1; then
  sudo_status="$(command_status sudo -n true)"
  sudo_id="$(sudo -n id 2>/dev/null || true)"
  sudo_capsh="$(sudo -n capsh --print 2>/dev/null || true)"
  run_log sudo -n id
  if command -v capsh >/dev/null 2>&1; then run_log sudo -n capsh --print; fi
fi
record sudo_n_status "$sudo_status"
record sudo_id "$sudo_id"
record sudo_has_cap_sys_admin "$(has_cap_sys_admin "$sudo_capsh")"
if command -v docker >/dev/null 2>&1; then run_log docker --version; fi

set +e
{
  echo "== apt-get install filesystem prerequisites =="
  if command -v sudo >/dev/null 2>&1; then sudo apt-get update && sudo apt-get install -y fuse3 libfuse3-dev pkg-config build-essential attr util-linux fuse-overlayfs; else apt-get update && apt-get install -y fuse3 libfuse3-dev pkg-config build-essential attr util-linux fuse-overlayfs; fi
} >> "$log" 2>&1
apt_status=$?
set -e
record apt_status "$apt_status"
record fusermount3_available "$(command -v fusermount3 >/dev/null 2>&1 && echo true || echo false)"
record fusermount3_setuid_or_usable "$(if command -v fusermount3 >/dev/null 2>&1 && { [ -u "$(command -v fusermount3)" ] || [ -x "$(command -v fusermount3)" ]; }; then echo true; else echo false; fi)"
record fuse3_installed "$(command -v fusermount3 >/dev/null 2>&1 && echo true || echo false)"
record fuse_overlayfs_available "$(command -v fuse-overlayfs >/dev/null 2>&1 && echo true || echo false)"
record fuse_overlayfs_installed "$(command -v fuse-overlayfs >/dev/null 2>&1 && echo true || echo false)"

probe_overlay() {
  local mode="$1"
  local root
  root="$(mktemp -d)"
  mkdir -p "$root/lower" "$root/upper" "$root/work" "$root/merged"
  echo probe > "$root/lower/file.txt"
  set +e
  if [ "$mode" = sudo ]; then
    sudo -n mount -t overlay overlay -o "lowerdir=$root/lower,upperdir=$root/upper,workdir=$root/work,redirect_dir=off" "$root/merged" >> "$log" 2>&1
  else
    mount -t overlay overlay -o "lowerdir=$root/lower,upperdir=$root/upper,workdir=$root/work,redirect_dir=off" "$root/merged" >> "$log" 2>&1
  fi
  local status=$?
  if [ "$status" -eq 0 ]; then
    if [ "$mode" = sudo ]; then sudo -n umount "$root/merged" >> "$log" 2>&1 || true; else umount "$root/merged" >> "$log" 2>&1 || true; fi
  fi
  set -e
  rm -rf "$root"
  printf '%s' "$status"
}

probe_fuse_overlayfs() {
  local mode="$1"
  local root
  root="$(mktemp -d)"
  mkdir -p "$root/lower" "$root/upper" "$root/work" "$root/merged"
  echo probe > "$root/lower/file.txt"
  set +e
  if [ "$mode" = sudo ]; then
    sudo -n fuse-overlayfs -o "lowerdir=$root/lower,upperdir=$root/upper,workdir=$root/work" "$root/merged" >> "$log" 2>&1
  else
    fuse-overlayfs -o "lowerdir=$root/lower,upperdir=$root/upper,workdir=$root/work" "$root/merged" >> "$log" 2>&1
  fi
  local status=$?
  if [ "$status" -eq 0 ]; then
    if command -v fusermount3 >/dev/null 2>&1; then fusermount3 -u "$root/merged" >> "$log" 2>&1 || true; fi
    if [ "$mode" = sudo ]; then sudo -n umount "$root/merged" >> "$log" 2>&1 || true; else umount "$root/merged" >> "$log" 2>&1 || true; fi
  fi
  set -e
  rm -rf "$root"
  printf '%s' "$status"
}

echo "== direct non-sudo overlay mount probe ==" | tee -a "$log"
overlay_non_sudo_mount_status="$(probe_overlay nonsudo)"
record overlay_non_sudo_mount_status "$overlay_non_sudo_mount_status"
record overlay_mount_probe_status "$overlay_non_sudo_mount_status"

overlay_sudo_mount_status=999
if command -v sudo >/dev/null 2>&1 && [ "$sudo_status" = 0 ]; then
  echo "== sudo overlay mount probe ==" | tee -a "$log"
  overlay_sudo_mount_status="$(probe_overlay sudo)"
fi
record overlay_sudo_mount_status "$overlay_sudo_mount_status"

fuse_overlayfs_rootless_status=999
if command -v fuse-overlayfs >/dev/null 2>&1; then
  echo "== rootless fuse-overlayfs probe ==" | tee -a "$log"
  fuse_overlayfs_rootless_status="$(probe_fuse_overlayfs nonsudo)"
fi
record fuse_overlayfs_rootless_status "$fuse_overlayfs_rootless_status"

fuse_overlayfs_sudo_status=999
if command -v sudo >/dev/null 2>&1 && [ "$sudo_status" = 0 ] && command -v fuse-overlayfs >/dev/null 2>&1; then
  echo "== sudo fuse-overlayfs probe ==" | tee -a "$log"
  fuse_overlayfs_sudo_status="$(probe_fuse_overlayfs sudo)"
fi
record fuse_overlayfs_sudo_status "$fuse_overlayfs_sudo_status"

# The strict matrix measures the kage-rofs mount path. Keep explicit probe
# fields for summary compatibility without duplicating expensive cargo tests.
record kage_rofs_non_sudo_mount_status 999
record kage_rofs_non_sudo_mount_error_kind not_attempted
record kage_rofs_sudo_mount_status 999
record kage_rofs_sudo_mount_error_kind not_attempted

cat "$env_file" | tee -a "$log"
