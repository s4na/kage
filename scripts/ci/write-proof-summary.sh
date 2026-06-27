#!/usr/bin/env bash
set -euo pipefail

mkdir -p target/ci-proof target/ci-logs
summary_json=target/ci-proof/summary.json
summary_md=target/ci-proof/summary.md
: > /tmp/ci-env-combined.sh
[ -f target/ci-proof/env.sh ] && cat target/ci-proof/env.sh >> /tmp/ci-env-combined.sh
[ -f target/ci-proof/results.env ] && cat target/ci-proof/results.env >> /tmp/ci-env-combined.sh
# shellcheck disable=SC1091
source /tmp/ci-env-combined.sh || true

strict_rofs_status=${strict_rofs_status:-999}
strict_overlay_status=${strict_overlay_status:-999}
strict_combined_status=${strict_combined_status:-999}
strict_runtime_status=${strict_runtime_status:-999}
apt_status=${apt_status:-999}
allow_skip_used=${allow_skip_used:-false}

env_text="$(cat target/ci-logs/*.log 2>/dev/null || true)"
classification="implementation_failure"
classification_detail="strict tests failed for an unclassified reason"
proof_level=1
strict_proof_obtained=false

if [[ "$allow_skip_used" == "true" ]]; then
  classification="implementation_failure"
  classification_detail="allow-skip variable was set in a strict proof job"
elif [[ "$strict_runtime_status" == "0" ]]; then
  classification="proof_passed"
  classification_detail="Level 4 runtime smoke passed without allow-skip"
  proof_level=4
  strict_proof_obtained=true
elif [[ "$strict_combined_status" == "0" ]]; then
  classification="proof_passed"
  classification_detail="Combined rofs+overlay strict tests passed, runtime smoke did not"
  proof_level=3
  strict_proof_obtained=true
elif [[ "$strict_rofs_status" == "0" && "$strict_overlay_status" == "0" ]]; then
  classification="proof_passed"
  classification_detail="Level 2 and Level 3 passed separately"
  proof_level=3
  strict_proof_obtained=true
elif [[ "$strict_rofs_status" == "0" ]]; then
  classification="environment_unsupported"
  classification_detail="Level 2 rofs passed, overlay/runtime did not pass; inspect overlay logs"
  proof_level=2
elif [[ "$strict_overlay_status" == "0" ]]; then
  classification="environment_unsupported"
  classification_detail="Level 3 overlay passed, rofs/runtime did not pass; inspect rofs logs"
  proof_level=3
elif grep -Eqi '/dev/fuse is unavailable|/dev/fuse unavailable|No such file or directory.*dev/fuse|fuse mount failed|CAP_SYS_ADMIN|permission denied|Operation not permitted|overlay mount failed|mount.*denied' <<< "$env_text"; then
  classification="environment_unsupported"
  classification_detail="strict filesystem proof did not pass because runner lacks /dev/fuse and/or mount capability"
  proof_level=1
elif [[ "$apt_status" != "0" ]]; then
  classification="setup_defect"
  classification_detail="filesystem prerequisite installation failed and strict failure was not otherwise classified"
  proof_level=1
fi

if [[ "${FAIL_ON_ENVIRONMENT_LIMIT:-false}" == "true" && "$classification" == "environment_unsupported" ]]; then
  final_status=2
elif [[ "$classification" == "implementation_failure" || "$classification" == "setup_defect" ]]; then
  final_status=1
else
  final_status=0
fi

export runner_os runner_arch runner_name image_label kernel os_release id sudo_available dev_fuse_exists dev_fuse_rw overlay_listed mount_path findmnt_path fuse3_installed fuse_overlayfs_installed classification classification_detail allow_skip_used strict_proof_obtained
export apt_status overlay_mount_probe_status strict_rofs_status strict_overlay_status strict_combined_status strict_runtime_status proof_level
python3 - <<'PY'
import json, os

def as_int(name, default=999):
    try:
        return int(os.environ.get(name, str(default)))
    except ValueError:
        return default

summary = {
  "runner": {
    "os": os.environ.get("runner_os", ""),
    "arch": os.environ.get("runner_arch", ""),
    "name": os.environ.get("runner_name", ""),
    "image_label": os.environ.get("image_label", ""),
  },
  "kernel": os.environ.get("kernel", ""),
  "os_release": os.environ.get("os_release", ""),
  "id": os.environ.get("id", ""),
  "sudo_available": os.environ.get("sudo_available", "unknown"),
  "dev_fuse_exists": os.environ.get("dev_fuse_exists", "unknown"),
  "dev_fuse_readable_writable": os.environ.get("dev_fuse_rw", "unknown"),
  "overlay_listed": os.environ.get("overlay_listed", "unknown"),
  "mount_path": os.environ.get("mount_path", ""),
  "findmnt_path": os.environ.get("findmnt_path", ""),
  "fuse3_installed": os.environ.get("fuse3_installed", "unknown"),
  "fuse_overlayfs_installed": os.environ.get("fuse_overlayfs_installed", "unknown"),
  "apt_status": as_int("apt_status"),
  "overlay_mount_probe_status": as_int("overlay_mount_probe_status"),
  "strict_results": {
    "rofs": as_int("strict_rofs_status"),
    "overlay": as_int("strict_overlay_status"),
    "combined": as_int("strict_combined_status"),
    "runtime": as_int("strict_runtime_status"),
  },
  "proof_level_reached": as_int("proof_level", 1),
  "allow_skip_used": os.environ.get("allow_skip_used") == "true",
  "strict_proof_obtained": os.environ.get("strict_proof_obtained") == "true",
  "classification": os.environ.get("classification", "implementation_failure"),
  "classification_detail": os.environ.get("classification_detail", ""),
}
with open("target/ci-proof/summary.json", "w", encoding="utf-8") as f:
    json.dump(summary, f, indent=2, ensure_ascii=False)
PY
cat > "$summary_md" <<MD
# kage CI filesystem proof summary

**Classification:** ${classification}  
**Detail:** ${classification_detail}  
**Proof level reached:** Level ${proof_level}  
**Allow-skip used:** ${allow_skip_used}  
**Strict proof obtained:** ${strict_proof_obtained}

| Probe | Result |
| --- | --- |
| Runner | ${runner_os:-unknown} / ${runner_arch:-unknown} / ${runner_name:-unknown} |
| Kernel | ${kernel:-unknown} |
| /dev/fuse exists | ${dev_fuse_exists:-unknown} |
| /dev/fuse read/write | ${dev_fuse_rw:-unknown} |
| overlay in /proc/filesystems | ${overlay_listed:-unknown} |
| apt status | ${apt_status} |
| overlay mount probe status | ${overlay_mount_probe_status:-999} |
| strict rofs status | ${strict_rofs_status} |
| strict overlay status | ${strict_overlay_status} |
| strict combined status | ${strict_combined_status} |
| strict runtime status | ${strict_runtime_status} |

Artifacts to inspect:

- \`target/ci-proof/summary.json\`
- \`target/ci-proof/summary.md\`
- \`target/ci-logs/fs-probe.log\`
- \`target/ci-logs/strict_rofs.log\`
- \`target/ci-logs/strict_overlay.log\`
- \`target/ci-logs/strict_combined.log\`
- \`target/ci-logs/strict_runtime.log\`
MD

cat "$summary_md"
if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
  cat "$summary_md" >> "$GITHUB_STEP_SUMMARY"
fi
exit "$final_status"
