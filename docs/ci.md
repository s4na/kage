# kage CI proof levels

GitHub Actions uses only free standard GitHub-hosted Linux runners. The main runner label is `ubuntu-24.04`; `ubuntu-slim` is intentionally not used because it is an unprivileged container and cannot truthfully probe low-level FUSE/overlay mount behavior.

## What default GHA proves

- **Level 0:** formatting, clippy, default non-kernel tests, and `git diff --check`.
- **Level 1:** focused mount-free rofs/protocol/lifecycle tests.
- **Level 2:** strict real kage-rofs FUSE mount with `KAGE_TEST_ROFS=1`.
- **Level 3:** strict real overlayfs mount with `KAGE_TEST_OVERLAY=1`.
- **Level 4:** strict rofs + overlay + commit-back/runtime smoke with `KAGE_TEST_ROFS=1 KAGE_TEST_OVERLAY=1 KAGE_TEST_RUNTIME=1`.
- **Level 5:** managed Linux VM / Apple Silicon workflow. This is documented future work, not implemented in free GHA.

PR/push CI runs Level 0, Level 1, and a Level 2-4 capability probe. The probe runs strict tests without allow-skip and classifies failures. A green workflow can mean either Level 4 was proven or the GitHub-hosted environment was cleanly classified as unsupported. Read the proof artifacts before claiming runtime readiness.

## Allow-skip policy

`KAGE_TEST_ROFS_ALLOW_SKIP=1` and `KAGE_TEST_OVERLAY_ALLOW_SKIP=1` are diagnostic-only. CI proof jobs do not use them. If an allow-skip variable is present in a strict proof job, the proof summary is invalid and the job fails.

## Artifacts

The filesystem probe uploads `kage-ci-proof-fs-capability-probe` containing:

- `target/ci-proof/summary.json`
- `target/ci-proof/summary.md`
- `target/ci-logs/fs-probe.log`
- `target/ci-logs/strict_rofs.log`
- `target/ci-logs/strict_overlay.log`
- `target/ci-logs/strict_combined.log`
- `target/ci-logs/strict_runtime.log`

`summary.json` is machine-readable and contains runner metadata, `/dev/fuse` state, overlay availability, strict command exit codes, proof level reached, terminal classification, allow-skip status, attempted/passed/skipped booleans for every strict proof command, environment blockers, failing tests, artifact paths, and the workflow run URL when GitHub exposes it.

## Interpreting outcomes

- **Level 1 only:** default and mount-free tests passed; runtime mount proof did not pass. Inspect classification.
- **Level 2 passed, Level 3 failed:** rofs FUSE mount works; overlayfs mount is blocked or broken.
- **Level 3 passed, Level 2 failed:** overlayfs mount works; rofs FUSE mount is blocked or broken.
- **Level 4 passed:** GitHub-hosted runner proved rofs + overlay + commit-back runtime smoke without allow-skip.
- **environment_unsupported:** runner lacks `/dev/fuse`, mount capability, or overlay permission. CI is green only because the limitation was classified.
- **setup_defect:** dependency installation or setup failed in a way that prevented classification.
- **implementation_failure:** capabilities appeared available or failure was ambiguous; Codex should inspect and fix code/tests/CI.

## Manual dispatch

`workflow_dispatch` inputs:

- `run_strict_fs`: run or skip the filesystem probe.
- `fail_on_environment_limit`: if `true`, fail unless strict proof reaches Level 4.
- `runner_label`: runner label, default `ubuntu-24.04`.

No self-hosted, larger, paid, or secret-backed runners are required by default.

## Human runbook when local `gh` cannot run Actions

From a checkout on the branch under review:

```bash
git push -u origin "$(git branch --show-current)"
gh workflow run ci.yml --ref "$(git branch --show-current)" \
  -f run_strict_fs=true \
  -f fail_on_environment_limit=false \
  -f runner_label=ubuntu-24.04
gh run watch
run_id="$(gh run list --workflow ci.yml --branch "$(git branch --show-current)" --limit 1 --json databaseId --jq '.[0].databaseId')"
mkdir -p gha-artifacts
gh run download "$run_id" --dir gha-artifacts
find gha-artifacts -maxdepth 4 -type f | sort
```

Paste back the workflow run URL plus `target/ci-proof/summary.json`, `target/ci-proof/summary.md`, `target/ci-logs/fs-probe.log`, and any failing strict logs from the downloaded `kage-ci-proof-fs-capability-probe` artifact.
