#!/usr/bin/env bash
set -euo pipefail
root="$(git rev-parse --show-toplevel)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
cd "$tmp"
mkdir -p target/ci-proof target/ci-logs scripts/ci
cp "$root/scripts/ci/write-proof-summary.sh" scripts/ci/write-proof-summary.sh
cat > target/ci-proof/results.env <<'ENV'
allow_skip_used=false
strict_rofs_sudo_status=0
strict_overlay_sudo_status=0
strict_combined_sudo_status=1
strict_runtime_sudo_status=1
ENV
cat > target/ci-logs/strict_rofs_sudo.log <<'LOG'
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 9 filtered out
LOG
cat > target/ci-logs/strict_overlay_sudo.log <<'LOG'
running 1 test
test overlayfs_backend_tree_matches_fallback_tree_when_enabled ... ok
LOG
cat > target/ci-logs/strict_combined_sudo.log <<'LOG'
fuser backend selected but the fuser crate is not available in this offline CI workspace; error_kind=fuser_backend_unavailable
LOG
cat > target/ci-logs/strict_runtime_sudo.log <<'LOG'
fuser backend selected but the fuser crate is not available in this offline CI workspace; error_kind=fuser_backend_unavailable
LOG
set +e
bash scripts/ci/write-proof-summary.sh >/tmp/write-proof-summary-zero-tests.out
status=$?
set -e
if [ "$status" -eq 0 ]; then
  echo "expected write-proof-summary to exit non-zero for harness false positive" >&2
  exit 1
fi
python3 - <<'PY'
import json
s=json.load(open('target/ci-proof/summary.json'))
assert s['strict_rofs_passed'] is False, s
assert s['proof_level_reached'] == 1, s
assert s['terminal_classification'] != 'LEVEL3_OVERLAY_AND_LOWER_PROVEN_BUT_RUNTIME_NOT_PROVEN', s
assert 'strict_rofs_sudo_zero_tests' in s['failing_tests'], s['failing_tests']
PY
