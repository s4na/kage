# CODEX CI Convergence

## Initial CI gap list

1. Existing workflow: `.github/workflows/ci.yml` with Rust checks, GHA lint, and zizmor.
2. Required default CI existed, but Rust tests were a single job and did not distinguish proof levels.
3. No strict filesystem capability probe job existed for free GitHub-hosted runners.
4. Allow-skip modes were not used in required proof jobs, but strict commands were also not run by CI.
5. Strict rofs/overlay/runtime commands were available in scripts/tests but not in GHA.
6. Logs/proof summaries were not uploaded as artifacts.
7. The workflow did not distinguish environment unsupported from implementation failure.
8. The workflow did not use `ubuntu-slim`; this remains correct.
9. README had verification levels, but no GHA-specific proof artifact/triage instructions.
10. There was no copy-pasteable CI result prompt for review.

## Cycle 1

- Failing or missing CI signal: default CI did not separate Level 0 from Level 1 mount-free verification.
- Classification: CI_CONFIG_DEFECT.
- Files inspected: `.github/workflows/ci.yml`, `scripts/ci-test-summary.sh`, `crates/kage-rofs/src/lib.rs`, `crates/kage-cli/src/main.rs`.
- Change made: split workflow into `rust-default` and `rofs-mount-free` jobs on `ubuntu-24.04`.
- Local command run: `bash scripts/ci/run-default-checks.sh` and focused cargo tests through the workflow-equivalent commands.
- Expected GitHub Actions behavior: Level 0 and Level 1 fail normally on implementation/test errors.
- Remaining risk: strict kernel behavior still needs classified probe.

## Cycle 2

- Failing or missing CI signal: no machine-readable strict filesystem capability proof summary.
- Classification: TEST_SIGNAL_DEFECT.
- Files inspected: `scripts/verify-rofs.sh`, `scripts/verify-overlayfs.sh`, strict test gates.
- Change made: added `scripts/ci/probe-github-hosted-fs.sh`, `scripts/ci/run-strict-fs-tests.sh`, and `scripts/ci/write-proof-summary.sh` to create `target/ci-proof/summary.json` and `summary.md` plus raw logs.
- Local command run: `bash scripts/ci/probe-github-hosted-fs.sh`; `bash scripts/ci/run-strict-fs-tests.sh || true`; `bash scripts/ci/write-proof-summary.sh`.
- Expected GitHub Actions behavior: strict commands run without allow-skip, failures are classified as proof, environment unsupported, setup defect, or implementation failure.
- Remaining risk: classifier may need refinement after real GHA artifacts are available.

## Cycle 3

- Failing or missing CI signal: workflow had no artifact upload or manual strict mode controls.
- Classification: CI_CONFIG_DEFECT.
- Files inspected: `.github/workflows/ci.yml`.
- Change made: added `workflow_dispatch` inputs, `fs-capability-probe` job, pinned `actions/upload-artifact`, artifact upload for `target/ci-proof/**` and `target/ci-logs/**`, and `fail_on_environment_limit` behavior.
- Local command run: YAML parse validation and shell syntax checks.
- Expected GitHub Actions behavior: PR/push CI stays green for cleanly classified GitHub-hosted limitations; manual dispatch can fail if Level 4 is not proven.
- Remaining risk: cannot execute GHA locally.

## Cycle 4

- Failing or missing CI signal: docs did not explain GHA artifacts and what users should paste back.
- Classification: DOCUMENTATION_DEFECT.
- Files inspected: `README.md`, `docs/`.
- Change made: added `docs/ci.md`, `docs/ci-result-triage.md`, and `docs/prompts/triage-gha-result.md`.
- Local command run: `git diff --check` and doc file inspection.
- Expected GitHub Actions behavior: reviewers can inspect `summary.json`, `summary.md`, and logs and paste a complete result back for the next convergence task.
- Remaining risk: docs may need updates after first live GHA run.
