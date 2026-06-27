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
terminal_classification="IMPLEMENTATION_BUG_WITH_REPRO"

if [[ "$allow_skip_used" == "true" ]]; then
  classification="implementation_failure"
  classification_detail="allow-skip variable was set in a strict proof job"
  terminal_classification="IMPLEMENTATION_BUG_WITH_REPRO"
elif [[ "$strict_runtime_status" == "0" ]]; then
  classification="proof_passed"
  classification_detail="Level 4 runtime smoke passed without allow-skip"
  proof_level=4
  strict_proof_obtained=true
  terminal_classification="LEVEL4_RUNTIME_PROVEN"
elif [[ "$strict_combined_status" == "0" ]]; then
  classification="proof_passed"
  classification_detail="Combined rofs+overlay strict tests passed, runtime smoke did not"
  proof_level=3
  strict_proof_obtained=true
  terminal_classification="LEVEL3_OVERLAY_AND_LOWER_PROVEN_BUT_RUNTIME_NOT_PROVEN"
elif [[ "$strict_rofs_status" == "0" && "$strict_overlay_status" == "0" ]]; then
  classification="proof_passed"
  classification_detail="Level 2 and Level 3 passed separately"
  proof_level=3
  strict_proof_obtained=true
  terminal_classification="LEVEL3_OVERLAY_AND_LOWER_PROVEN_BUT_RUNTIME_NOT_PROVEN"
elif [[ "$strict_rofs_status" == "0" ]]; then
  classification="environment_unsupported"
  classification_detail="Level 2 rofs passed, overlay/runtime did not pass; inspect overlay logs"
  proof_level=2
  terminal_classification="LEVEL2_ROFS_PROVEN_BUT_OVERLAY_OR_RUNTIME_NOT_PROVEN"
elif [[ "$strict_overlay_status" == "0" ]]; then
  classification="environment_unsupported"
  classification_detail="Level 3 overlay passed, rofs/runtime did not pass; inspect rofs logs"
  proof_level=3
  terminal_classification="LEVEL3_OVERLAY_AND_LOWER_PROVEN_BUT_RUNTIME_NOT_PROVEN"
elif grep -Eqi '/dev/fuse is unavailable|/dev/fuse unavailable|No such file or directory.*dev/fuse|fuse mount failed|CAP_SYS_ADMIN|permission denied|Operation not permitted|overlay mount failed|mount.*denied' <<< "$env_text"; then
  classification="environment_unsupported"
  classification_detail="strict filesystem proof did not pass because runner lacks /dev/fuse and/or mount capability"
  proof_level=1
  terminal_classification="STRONG_ENVIRONMENT_BLOCKED"
elif [[ "$apt_status" != "0" ]]; then
  classification="setup_defect"
  classification_detail="filesystem prerequisite installation failed and strict failure was not otherwise classified"
  proof_level=1
  terminal_classification="CI_REVIEWABLE_PENDING_GHA_RUN"
fi

if [[ "${FAIL_ON_ENVIRONMENT_LIMIT:-false}" == "true" && "$classification" == "environment_unsupported" ]]; then
  final_status=2
elif [[ "$classification" == "implementation_failure" || "$classification" == "setup_defect" ]]; then
  final_status=1
else
  final_status=0
fi

workflow_run_url=""
if [[ -n "${GITHUB_SERVER_URL:-}" && -n "${GITHUB_REPOSITORY:-}" && -n "${GITHUB_RUN_ID:-}" ]]; then
  workflow_run_url="${GITHUB_SERVER_URL}/${GITHUB_REPOSITORY}/actions/runs/${GITHUB_RUN_ID}"
fi

export runner_os runner_arch runner_name image_label kernel os_release id sudo_available dev_fuse_exists dev_fuse_rw overlay_listed mount_path findmnt_path fuse3_installed fuse_overlayfs_installed classification classification_detail terminal_classification allow_skip_used strict_proof_obtained workflow_run_url
export apt_status overlay_mount_probe_status strict_rofs_status strict_overlay_status strict_combined_status strict_runtime_status proof_level
python3 - <<'PY'
import json, os
from pathlib import Path

def as_int(name, default=999):
    try:
        return int(os.environ.get(name, str(default)))
    except ValueError:
        return default

def log_text(name):
    path = Path("target/ci-logs") / f"{name}.log"
    try:
        return path.read_text(encoding="utf-8", errors="replace")
    except FileNotFoundError:
        return ""

def attempted(name):
    return as_int(f"{name}_status") != 999

def skipped(name):
    # Workspace-wide strict commands can contain unrelated gated-test messages
    # (for example xattr or runtime tests outside the current proof level). A
    # strict proof command is counted as skipped only when allow-skip was
    # explicitly enabled, which CI treats as invalid proof.
    _ = name
    return os.environ.get("allow_skip_used") == "true"

def passed(name):
    return attempted(name) and as_int(f"{name}_status") == 0 and not skipped(name)

strict_names = ["strict_rofs", "strict_overlay", "strict_combined", "strict_runtime"]
failing_tests = [
    name for name in strict_names
    if attempted(name) and as_int(f"{name}_status") != 0
]
environment_blockers = []
if os.environ.get("dev_fuse_exists") == "false":
    environment_blockers.append("missing /dev/fuse")
if os.environ.get("dev_fuse_rw") == "false":
    environment_blockers.append("/dev/fuse is not readable/writable")
if os.environ.get("overlay_listed") == "false":
    environment_blockers.append("overlay is not listed in /proc/filesystems")
if as_int("overlay_mount_probe_status") not in (0, 999):
    environment_blockers.append(f"overlay mount probe exited {as_int('overlay_mount_probe_status')}")

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
  "terminal_classification": os.environ.get("terminal_classification", "IMPLEMENTATION_BUG_WITH_REPRO"),
  "allow_skip_used": os.environ.get("allow_skip_used") == "true",
  "strict_proof_obtained": os.environ.get("strict_proof_obtained") == "true",
  "strict_rofs_attempted": attempted("strict_rofs"),
  "strict_rofs_passed": passed("strict_rofs"),
  "strict_rofs_skipped": skipped("strict_rofs"),
  "strict_overlay_attempted": attempted("strict_overlay"),
  "strict_overlay_passed": passed("strict_overlay"),
  "strict_overlay_skipped": skipped("strict_overlay"),
  "strict_combined_attempted": attempted("strict_combined"),
  "strict_combined_passed": passed("strict_combined"),
  "strict_combined_skipped": skipped("strict_combined"),
  "strict_runtime_attempted": attempted("strict_runtime"),
  "strict_runtime_passed": passed("strict_runtime"),
  "strict_runtime_skipped": skipped("strict_runtime"),
  "environment_blockers": environment_blockers,
  "failing_tests": failing_tests,
  "artifact_paths": [
    "target/ci-proof/summary.json",
    "target/ci-proof/summary.md",
    "target/ci-logs/fs-probe.log",
    "target/ci-logs/strict_rofs.log",
    "target/ci-logs/strict_overlay.log",
    "target/ci-logs/strict_combined.log",
    "target/ci-logs/strict_runtime.log",
  ],
  "workflow_run_url": os.environ.get("workflow_run_url", ""),
  "classification": os.environ.get("classification", "implementation_failure"),
  "classification_detail": os.environ.get("classification_detail", ""),
}
with open("target/ci-proof/summary.json", "w", encoding="utf-8") as f:
    json.dump(summary, f, indent=2, ensure_ascii=False)
PY
cat > "$summary_md" <<MD
# kage CI filesystem proof summary

**Classification:** ${classification}  
**Terminal classification:** ${terminal_classification}
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
