#!/usr/bin/env bash
set -euo pipefail

mkdir -p target/ci-proof target/ci-logs
summary_json=target/ci-proof/summary.json
summary_md=target/ci-proof/summary.md
combined_env=target/ci-proof/combined.env
classification_env=target/ci-proof/classification.env
: > "$combined_env"
[ -f target/ci-proof/env.sh ] && cat target/ci-proof/env.sh >> "$combined_env"
[ -f target/ci-proof/container-host.env ] && cat target/ci-proof/container-host.env >> "$combined_env"
[ -f target/ci-proof/container.env ] && cat target/ci-proof/container.env >> "$combined_env"
if [ -f target/ci-proof/results.env ]; then
  sed '/^[^=]*_command=/d' target/ci-proof/results.env >> "$combined_env"
fi
# shellcheck disable=SC1090
set -a
source "$combined_env" || true
set +a

python3 - <<'PY'
import json, os, re, shlex
from pathlib import Path

def getenv(name, default=""):
    return os.environ.get(name, default)

def as_int(name, default=999):
    try:
        return int(os.environ.get(name, str(default)))
    except ValueError:
        return default

def as_bool(name):
    return str(os.environ.get(name, "")).lower() == "true"

def status(name):
    return as_int(f"{name}_status")

def attempted(name):
    return status(name) != 999

def skipped(_name):
    return as_bool("allow_skip_used")

def passed(name):
    return attempted(name) and status(name) == 0 and not skipped(name)

def any_pass(*names):
    return any(passed(name) for name in names)

def any_attempt(*names):
    return any(attempted(name) for name in names)

def log_text():
    chunks = []
    for path in Path("target/ci-logs").glob("*.log"):
        chunks.append(path.read_text(encoding="utf-8", errors="replace"))
    return "\n".join(chunks)

text = log_text()
allow_skip_used = as_bool("allow_skip_used")
strict_rofs_passed = any_pass("strict_rofs_nonsudo", "strict_rofs_sudo", "strict_rofs")
strict_overlay_passed = any_pass("strict_overlay_nonsudo", "strict_overlay_sudo", "strict_overlay")
strict_combined_passed = any_pass("strict_combined_nonsudo", "strict_combined_sudo", "strict_combined")
strict_runtime_passed = any_pass("strict_runtime_nonsudo", "strict_runtime_sudo", "strict_runtime")
strict_rofs_attempted = any_attempt("strict_rofs_nonsudo", "strict_rofs_sudo", "strict_rofs")
strict_overlay_attempted = any_attempt("strict_overlay_nonsudo", "strict_overlay_sudo", "strict_overlay")
strict_combined_attempted = any_attempt("strict_combined_nonsudo", "strict_combined_sudo", "strict_combined")
strict_runtime_attempted = any_attempt("strict_runtime_nonsudo", "strict_runtime_sudo", "strict_runtime")
containerized_strict_attempted = as_bool("containerized_strict_attempted")
containerized_rofs_attempted = as_bool("containerized_rofs_attempted") or as_int("containerized_rofs_status") != 999
containerized_rofs_passed = containerized_rofs_attempted and as_int("containerized_rofs_status") == 0 and not allow_skip_used
containerized_overlay_attempted = as_bool("containerized_overlay_attempted") or as_int("containerized_overlay_status") != 999
containerized_overlay_passed = containerized_overlay_attempted and as_int("containerized_overlay_status") == 0 and not allow_skip_used
containerized_combined_attempted = as_bool("containerized_combined_attempted") or as_int("containerized_combined_status") != 999
containerized_combined_passed = containerized_combined_attempted and as_int("containerized_combined_status") == 0 and not allow_skip_used
containerized_runtime_attempted = as_bool("containerized_runtime_attempted") or as_int("containerized_runtime_status") != 999
containerized_runtime_passed = containerized_runtime_attempted and as_int("containerized_runtime_status") == 0 and not allow_skip_used
containerized_any_attempted = any([containerized_rofs_attempted, containerized_overlay_attempted, containerized_combined_attempted, containerized_runtime_attempted])
containerized_any_failed = any([
    containerized_rofs_attempted and not containerized_rofs_passed,
    containerized_overlay_attempted and not containerized_overlay_passed,
    containerized_combined_attempted and not containerized_combined_passed,
    containerized_runtime_attempted and not containerized_runtime_passed,
])

privileged_route_attempted = any_attempt(
    "strict_rofs_sudo", "strict_overlay_sudo", "strict_combined_sudo", "strict_runtime_sudo"
)
helper_or_priv_probe_attempted = any(
    as_int(name) != 999 for name in [
        "overlay_sudo_mount_status",
        "fuse_overlayfs_rootless_status",
        "fuse_overlayfs_sudo_status",
        "kage_rofs_non_sudo_mount_status",
        "kage_rofs_sudo_mount_status",
    ]
)

failing_tests = []
for name in [
    "strict_rofs_nonsudo", "strict_rofs_sudo", "strict_rofs",
    "strict_overlay_nonsudo", "strict_overlay_sudo", "strict_overlay",
    "strict_combined_nonsudo", "strict_combined_sudo", "strict_combined",
    "strict_runtime_nonsudo", "strict_runtime_sudo", "strict_runtime",
]:
    if attempted(name) and status(name) != 0:
        failing_tests.append(name)
for name in ["rofs", "overlay", "combined", "runtime"]:
    if as_bool(f"containerized_{name}_attempted") and as_int(f"containerized_{name}_status") != 0:
        failing_tests.append(f"containerized_{name}")

environment_blockers = []
if getenv("dev_fuse_exists") == "false":
    environment_blockers.append("missing /dev/fuse")
if getenv("dev_fuse_rw") == "false":
    environment_blockers.append("/dev/fuse is not readable/writable")
if getenv("overlay_listed") == "false":
    environment_blockers.append("overlay is not listed in /proc/filesystems")
for field, label in [
    ("overlay_non_sudo_mount_status", "non-sudo overlay mount"),
    ("overlay_sudo_mount_status", "sudo overlay mount"),
    ("fuse_overlayfs_rootless_status", "rootless fuse-overlayfs"),
    ("fuse_overlayfs_sudo_status", "sudo fuse-overlayfs"),
]:
    value = as_int(field)
    if value in (0, 999):
        continue
    if field == "overlay_non_sudo_mount_status" and as_int("overlay_sudo_mount_status") == 0:
        continue
    if field == "fuse_overlayfs_rootless_status" and as_int("fuse_overlayfs_sudo_status") == 0:
        continue
    environment_blockers.append(f"{label} exited {value}")
if getenv("dev_fuse_exists") == "true" and getenv("sudo_available") == "true" and not privileged_route_attempted:
    environment_blockers.append("strict privileged/helper mount route not yet exercised")
if getenv("fuse_overlayfs_available") == "true" and as_int("fuse_overlayfs_rootless_status") == 999 and as_int("fuse_overlayfs_sudo_status") == 999:
    environment_blockers.append("fuse-overlayfs helper route not yet exercised")

permission_re = re.compile(r"Operation not permitted|permission denied|must be superuser|CAP_SYS_ADMIN|mount failed", re.I)
setup_re = re.compile(r"sudo: .*cargo: command not found|No such file or directory.*cargo|rustup: command not found", re.I)
git_index_conflict_re = re.compile(r"appears as both a file and as a directory|cannot add to the index|git update-index", re.I)
tree_mismatch_re = re.compile(r"assertion.*left.*right|tree hash mismatch|fallback tree hash|overlay tree hash", re.I | re.S)
timeout_re = re.compile(r"timed out|timeout|has been running for over 60 seconds|status: 143|Command exited with non-zero status 124", re.I)
rofs_einval_re = re.compile(r"kage-rofs fuse mount failed:.*Invalid argument|direct FUSE mount returned EINVAL|os error 22", re.I)
capable_overlay_failed = as_int("overlay_sudo_mount_status") == 0 and attempted("strict_overlay_sudo") and not passed("strict_overlay_sudo")
capable_rofs_failed = (
    getenv("dev_fuse_exists") == "true"
    and getenv("dev_fuse_rw") == "true"
    and (getenv("sudo_has_cap_sys_admin") == "true" or getenv("fusermount3_available") == "true")
    and attempted("strict_rofs_sudo")
    and not passed("strict_rofs_sudo")
)

classification = "implementation_failure"
classification_detail = "strict tests failed for an unclassified reason"
terminal_classification = "IMPLEMENTATION_BUG_WITH_REPRO"
proof_level = 1
strict_proof_obtained = False
proof_route_scope = getenv("KAGE_PROOF_ROUTE", "auto")

if allow_skip_used:
    classification_detail = "allow-skip variable was set in a strict proof job"
elif strict_runtime_passed or containerized_runtime_passed:
    classification = "proof_passed"
    classification_detail = "Level 4 runtime smoke passed without allow-skip"
    terminal_classification = "LEVEL4_RUNTIME_PROVEN"
    proof_level = 4
    strict_proof_obtained = True
elif strict_combined_passed or containerized_combined_passed or (strict_rofs_passed and strict_overlay_passed) or (containerized_rofs_passed and containerized_overlay_passed):
    classification = "proof_passed"
    classification_detail = "Level 3 rofs/overlay substrate passed without full runtime smoke"
    terminal_classification = "LEVEL3_OVERLAY_AND_LOWER_PROVEN_BUT_RUNTIME_NOT_PROVEN"
    proof_level = 3
    strict_proof_obtained = True
elif strict_rofs_passed or containerized_rofs_passed:
    classification = "proof_passed"
    classification_detail = "Level 2 rofs FUSE mount passed; overlay/runtime did not"
    terminal_classification = "LEVEL2_ROFS_PROVEN_BUT_OVERLAY_OR_RUNTIME_NOT_PROVEN"
    proof_level = 2
    strict_proof_obtained = True
elif getenv("dev_fuse_exists") == "true" and getenv("sudo_available") == "true" and not privileged_route_attempted:
    classification = "environment_unsupported"
    classification_detail = "Level 1 only; /dev/fuse and sudo are present but strict privileged/helper routes were not attempted"
    terminal_classification = "LEVEL1_MOUNT_FREE_ONLY_PROVEN"
elif as_int("containerized_image_build_status") not in (0, 999):
    classification = "setup_defect"
    classification_detail = "containerized strict image build failed before strict tests could run"
    terminal_classification = "CI_REVIEWABLE_PENDING_GHA_RUN"
elif not any_attempt("strict_rofs_nonsudo", "strict_rofs_sudo", "strict_rofs", "strict_overlay_nonsudo", "strict_overlay_sudo", "strict_overlay", "strict_combined_nonsudo", "strict_combined_sudo", "strict_combined", "strict_runtime_nonsudo", "strict_runtime_sudo", "strict_runtime") and as_int("apt_status") == 999:
    classification = "not_run"
    classification_detail = "proof summary generated without probe or strict-result inputs"
    terminal_classification = "CI_REVIEWABLE_PENDING_GHA_RUN"
elif setup_re.search(text):
    classification = "setup_defect"
    classification_detail = "sudo strict route could not run cargo/toolchain; CI harness needs setup fixes"
    terminal_classification = "CI_REVIEWABLE_PENDING_GHA_RUN"
elif capable_overlay_failed and git_index_conflict_re.search(text):
    classification = "implementation_failure"
    classification_detail = "sudo overlay substrate passed, but strict overlay failed with a Git index file/directory conflict"
    terminal_classification = "IMPLEMENTATION_BUG_WITH_REPRO"
elif capable_overlay_failed and tree_mismatch_re.search(text):
    classification = "implementation_failure"
    classification_detail = "sudo overlay substrate passed, but strict overlay tree output did not match fallback tree output"
    terminal_classification = "IMPLEMENTATION_BUG_WITH_REPRO"
elif capable_rofs_failed and timeout_re.search(text):
    classification = "implementation_failure"
    classification_detail = "usable /dev/fuse plus fusermount3/sudo route existed, but strict kage-rofs mount timed out"
    terminal_classification = "IMPLEMENTATION_BUG_WITH_REPRO"
elif capable_rofs_failed and rofs_einval_re.search(text):
    classification = "implementation_failure"
    classification_detail = "sudo/helper rofs route was available, but direct kage-rofs FUSE mount returned EINVAL"
    terminal_classification = "IMPLEMENTATION_BUG_WITH_REPRO"
elif containerized_any_failed and (timeout_re.search(text) or tree_mismatch_re.search(text) or rofs_einval_re.search(text) or getenv("containerized_dev_fuse_exists") == "true" or getenv("containerized_overlay_mount_status") == "0"):
    classification = "implementation_failure"
    classification_detail = "containerized privileged strict route was attempted but did not produce a strict proof"
    terminal_classification = "IMPLEMENTATION_BUG_WITH_REPRO"
elif permission_re.search(text) and (privileged_route_attempted or helper_or_priv_probe_attempted) and not (capable_rofs_failed or capable_overlay_failed):
    classification = "environment_unsupported"
    classification_detail = "strict filesystem proof failed after non-sudo and privileged/helper routes were attempted"
    terminal_classification = "STRONG_ENVIRONMENT_BLOCKED"
elif as_int("apt_status") != 0 and not permission_re.search(text):
    classification = "setup_defect"
    classification_detail = "filesystem prerequisite installation failed and strict failure was not otherwise classified"
    terminal_classification = "CI_REVIEWABLE_PENDING_GHA_RUN"

workflow_run_url = ""
if getenv("GITHUB_SERVER_URL") and getenv("GITHUB_REPOSITORY") and getenv("GITHUB_RUN_ID"):
    workflow_run_url = f"{getenv('GITHUB_SERVER_URL')}/{getenv('GITHUB_REPOSITORY')}/actions/runs/{getenv('GITHUB_RUN_ID')}"

artifact_paths = [
    "target/ci-proof/summary.json",
    "target/ci-proof/summary.md",
    "target/ci-logs/fs-probe.log",
    "target/ci-logs/strict_rofs_nonsudo.log",
    "target/ci-logs/strict_rofs_sudo.log",
    "target/ci-logs/strict_overlay_nonsudo.log",
    "target/ci-logs/strict_overlay_sudo.log",
    "target/ci-logs/strict_combined_nonsudo.log",
    "target/ci-logs/strict_combined_sudo.log",
    "target/ci-logs/strict_runtime_nonsudo.log",
    "target/ci-logs/strict_runtime_sudo.log",
    "target/ci-logs/container_probe.log",
    "target/ci-logs/container_strict_rofs.log",
    "target/ci-logs/container_strict_overlay.log",
    "target/ci-logs/container_strict_combined.log",
    "target/ci-logs/container_strict_runtime.log",
]
summary = {
    "runner": {
        "os": getenv("runner_os"),
        "arch": getenv("runner_arch"),
        "name": getenv("runner_name"),
        "image_label": getenv("image_label"),
    },
    "kernel": getenv("kernel"),
    "os_release": getenv("os_release"),
    "id": getenv("id"),
    "sudo_available": getenv("sudo_available", "unknown"),
    "sudo_n_status": as_int("sudo_n_status"),
    "sudo_id": getenv("sudo_id"),
    "dev_fuse_exists": getenv("dev_fuse_exists", "unknown"),
    "dev_fuse_readable_writable": getenv("dev_fuse_rw", "unknown"),
    "overlay_listed": getenv("overlay_listed", "unknown"),
    "mount_path": getenv("mount_path"),
    "findmnt_path": getenv("findmnt_path"),
    "fusermount3_available": getenv("fusermount3_available", "unknown"),
    "fusermount3_path": getenv("fusermount3_path"),
    "fusermount3_setuid_or_usable": getenv("fusermount3_setuid_or_usable", "unknown"),
    "fuse_overlayfs_available": getenv("fuse_overlayfs_available", "unknown"),
    "fuse_overlayfs_path": getenv("fuse_overlayfs_path"),
    "fuse3_installed": getenv("fuse3_installed", "unknown"),
    "fuse_overlayfs_installed": getenv("fuse_overlayfs_installed", "unknown"),
    "fuse_conf_present": getenv("fuse_conf_present", "unknown"),
    "fuse_conf_user_allow_other": getenv("fuse_conf_user_allow_other", "unknown"),
    "runner_has_cap_sys_admin": getenv("runner_has_cap_sys_admin", "unknown"),
    "sudo_has_cap_sys_admin": getenv("sudo_has_cap_sys_admin", "unknown"),
    "apt_status": as_int("apt_status"),
    "overlay_mount_probe_status": as_int("overlay_mount_probe_status"),
    "overlay_non_sudo_mount_status": as_int("overlay_non_sudo_mount_status"),
    "overlay_sudo_mount_status": as_int("overlay_sudo_mount_status"),
    "fuse_overlayfs_rootless_status": as_int("fuse_overlayfs_rootless_status"),
    "fuse_overlayfs_sudo_status": as_int("fuse_overlayfs_sudo_status"),
    "kage_rofs_non_sudo_mount_status": as_int("kage_rofs_non_sudo_mount_status"),
    "kage_rofs_non_sudo_mount_error_kind": getenv("kage_rofs_non_sudo_mount_error_kind"),
    "kage_rofs_sudo_mount_status": as_int("kage_rofs_sudo_mount_status"),
    "kage_rofs_sudo_mount_error_kind": getenv("kage_rofs_sudo_mount_error_kind"),
    "strict_results": {
        "rofs": as_int("strict_rofs_status"),
        "overlay": as_int("strict_overlay_status"),
        "combined": as_int("strict_combined_status"),
        "runtime": as_int("strict_runtime_status"),
    },
    "proof_level_reached": proof_level,
    "terminal_classification": terminal_classification,
    "proof_route_scope": proof_route_scope,
    "allow_skip_used": allow_skip_used,
    "strict_proof_obtained": strict_proof_obtained,
    "strict_rofs_attempted": strict_rofs_attempted,
    "strict_rofs_passed": strict_rofs_passed,
    "strict_rofs_skipped": skipped("strict_rofs"),
    "strict_rofs_nonsudo_attempted": attempted("strict_rofs_nonsudo"),
    "strict_rofs_nonsudo_passed": passed("strict_rofs_nonsudo"),
    "strict_rofs_sudo_attempted": attempted("strict_rofs_sudo"),
    "strict_rofs_sudo_passed": passed("strict_rofs_sudo"),
    "strict_overlay_attempted": strict_overlay_attempted,
    "strict_overlay_passed": strict_overlay_passed,
    "strict_overlay_skipped": skipped("strict_overlay"),
    "strict_overlay_nonsudo_attempted": attempted("strict_overlay_nonsudo"),
    "strict_overlay_nonsudo_passed": passed("strict_overlay_nonsudo"),
    "strict_overlay_sudo_attempted": attempted("strict_overlay_sudo"),
    "strict_overlay_sudo_passed": passed("strict_overlay_sudo"),
    "strict_combined_attempted": strict_combined_attempted,
    "strict_combined_passed": strict_combined_passed,
    "strict_combined_skipped": skipped("strict_combined"),
    "strict_combined_nonsudo_attempted": attempted("strict_combined_nonsudo"),
    "strict_combined_nonsudo_passed": passed("strict_combined_nonsudo"),
    "strict_combined_sudo_attempted": attempted("strict_combined_sudo"),
    "strict_combined_sudo_passed": passed("strict_combined_sudo"),
    "strict_runtime_attempted": strict_runtime_attempted,
    "strict_runtime_passed": strict_runtime_passed,
    "strict_runtime_skipped": skipped("strict_runtime"),
    "strict_runtime_nonsudo_attempted": attempted("strict_runtime_nonsudo"),
    "strict_runtime_nonsudo_passed": passed("strict_runtime_nonsudo"),
    "strict_runtime_sudo_attempted": attempted("strict_runtime_sudo"),
    "strict_runtime_sudo_passed": passed("strict_runtime_sudo"),
    "environment_blockers": environment_blockers,
    "failing_tests": failing_tests,
    "artifact_paths": artifact_paths,
    "workflow_run_url": workflow_run_url,
    "proof_routes": [route for route, present in [("host", strict_rofs_attempted or strict_overlay_attempted or strict_combined_attempted or strict_runtime_attempted), ("containerized", containerized_strict_attempted)] if present],
    "containerized_strict_attempted": containerized_strict_attempted,
    "containerized_strict_image": getenv("containerized_strict_image"),
    "containerized_privileged_attempted": as_bool("containerized_privileged_attempted"),
    "containerized_docker_available": getenv("containerized_docker_available", "unknown"),
    "containerized_image_build_status": as_int("containerized_image_build_status"),
    "containerized_apparmor_unconfined_status": as_int("containerized_apparmor_unconfined_status"),
    "containerized_docker_run_status": as_int("containerized_docker_run_status"),
    "containerized_dev_fuse_exists": getenv("containerized_dev_fuse_exists", "unknown"),
    "containerized_dev_fuse_readable_writable": getenv("containerized_dev_fuse_readable_writable", "unknown"),
    "containerized_fusermount3_available": getenv("containerized_fusermount3_available", "unknown"),
    "containerized_fuse_overlayfs_available": getenv("containerized_fuse_overlayfs_available", "unknown"),
    "containerized_overlay_available": getenv("containerized_overlay_available", "unknown"),
    "containerized_overlay_mount_status": as_int("containerized_overlay_mount_status"),
    "containerized_fuse_overlayfs_status": as_int("containerized_fuse_overlayfs_status"),
    "containerized_rofs_attempted": containerized_rofs_attempted,
    "containerized_rofs_passed": containerized_rofs_passed,
    "containerized_rofs_error_kind": getenv("containerized_rofs_error_kind"),
    "containerized_overlay_attempted": containerized_overlay_attempted,
    "containerized_overlay_passed": containerized_overlay_passed,
    "containerized_overlay_error_kind": getenv("containerized_overlay_error_kind"),
    "containerized_combined_attempted": containerized_combined_attempted,
    "containerized_combined_passed": containerized_combined_passed,
    "containerized_combined_error_kind": getenv("containerized_combined_error_kind"),
    "containerized_runtime_attempted": containerized_runtime_attempted,
    "containerized_runtime_passed": containerized_runtime_passed,
    "containerized_runtime_error_kind": getenv("containerized_runtime_error_kind"),
    "classification": classification,
    "classification_detail": classification_detail,
}
Path("target/ci-proof/summary.json").write_text(json.dumps(summary, indent=2, ensure_ascii=False), encoding="utf-8")
Path("target/ci-proof/classification.env").write_text(
    "\n".join([
        f"classification={shlex.quote(classification)}",
        f"classification_detail={shlex.quote(classification_detail)}",
        f"terminal_classification={shlex.quote(terminal_classification)}",
        f"proof_level={proof_level}",
        f"strict_proof_obtained={'true' if strict_proof_obtained else 'false'}",
        f"final_status={'2' if os.environ.get('FAIL_ON_ENVIRONMENT_LIMIT') == 'true' and classification == 'environment_unsupported' else '1' if classification in ('implementation_failure', 'setup_defect') else '0'}",
    ]) + "\n",
    encoding="utf-8",
)
PY
# shellcheck disable=SC1091
source "$classification_env"

cat > "$summary_md" <<MD
# kage CI filesystem proof summary

**Classification:** ${classification}
**Terminal classification:** ${terminal_classification}
**Detail:** ${classification_detail}
**Route scope:** ${KAGE_PROOF_ROUTE:-auto}
**Exit behavior:** implementation and setup defects intentionally exit non-zero after writing artifacts.
**Proof level reached:** Level ${proof_level}
**Allow-skip used:** ${allow_skip_used:-false}
**Strict proof obtained:** ${strict_proof_obtained}

| Probe | Result |
| --- | --- |
| Runner | ${runner_os:-unknown} / ${runner_arch:-unknown} / ${runner_name:-unknown} |
| Kernel | ${kernel:-unknown} |
| /dev/fuse exists | ${dev_fuse_exists:-unknown} |
| /dev/fuse read/write | ${dev_fuse_rw:-unknown} |
| fusermount3 available | ${fusermount3_available:-unknown} |
| fuse-overlayfs available | ${fuse_overlayfs_available:-unknown} |
| sudo available | ${sudo_available:-unknown} |
| runner CAP_SYS_ADMIN | ${runner_has_cap_sys_admin:-unknown} |
| sudo CAP_SYS_ADMIN | ${sudo_has_cap_sys_admin:-unknown} |
| overlay in /proc/filesystems | ${overlay_listed:-unknown} |
| apt status | ${apt_status:-999} |
| overlay non-sudo mount status | ${overlay_non_sudo_mount_status:-999} |
| overlay sudo mount status | ${overlay_sudo_mount_status:-999} |
| fuse-overlayfs rootless status | ${fuse_overlayfs_rootless_status:-999} |
| fuse-overlayfs sudo status | ${fuse_overlayfs_sudo_status:-999} |
| kage-rofs non-sudo mount status | ${kage_rofs_non_sudo_mount_status:-999} (${kage_rofs_non_sudo_mount_error_kind:-unknown}) |
| kage-rofs sudo mount status | ${kage_rofs_sudo_mount_status:-999} (${kage_rofs_sudo_mount_error_kind:-unknown}) |
| strict rofs status | ${strict_rofs_status:-999} |
| strict overlay status | ${strict_overlay_status:-999} |
| strict combined status | ${strict_combined_status:-999} |
| strict runtime status | ${strict_runtime_status:-999} |
| containerized strict image | ${containerized_strict_image:-unknown} |
| containerized docker available | ${containerized_docker_available:-unknown} |
| containerized image build status | ${containerized_image_build_status:-999} |
| containerized apparmor=unconfined status | ${containerized_apparmor_unconfined_status:-999} |
| containerized docker run status | ${containerized_docker_run_status:-999} |
| containerized /dev/fuse exists | ${containerized_dev_fuse_exists:-unknown} |
| containerized /dev/fuse read/write | ${containerized_dev_fuse_readable_writable:-unknown} |
| containerized overlay mount status | ${containerized_overlay_mount_status:-999} |
| containerized fuse-overlayfs status | ${containerized_fuse_overlayfs_status:-999} |
| containerized rofs status | ${containerized_rofs_status:-999} (${containerized_rofs_error_kind:-unknown}) |
| containerized overlay status | ${containerized_overlay_status:-999} (${containerized_overlay_error_kind:-unknown}) |
| containerized combined status | ${containerized_combined_status:-999} (${containerized_combined_error_kind:-unknown}) |
| containerized runtime status | ${containerized_runtime_status:-999} (${containerized_runtime_error_kind:-unknown}) |

If this is the host-route artifact, containerized fields are expected to be unknown/999; inspect the separate kage-ci-proof-fs-capability-probe-containerized artifact for the privileged-container route.

Artifacts to inspect:

- target/ci-proof/summary.json
- target/ci-proof/summary.md
- target/ci-logs/fs-probe.log
- target/ci-logs/strict_rofs_nonsudo.log
- target/ci-logs/strict_rofs_sudo.log
- target/ci-logs/strict_overlay_nonsudo.log
- target/ci-logs/strict_overlay_sudo.log
- target/ci-logs/strict_combined_nonsudo.log
- target/ci-logs/strict_combined_sudo.log
- target/ci-logs/strict_runtime_nonsudo.log
- target/ci-logs/strict_runtime_sudo.log
- target/ci-logs/container_probe.log
- target/ci-logs/container_strict_rofs.log
- target/ci-logs/container_strict_overlay.log
- target/ci-logs/container_strict_combined.log
- target/ci-logs/container_strict_runtime.log
MD

cat "$summary_md"
if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
  cat "$summary_md" >> "$GITHUB_STEP_SUMMARY"
fi
exit "$final_status"
