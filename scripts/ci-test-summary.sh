#!/usr/bin/env bash
set -uo pipefail

log_path="${1:-target/ci/cargo-test.log}"
mkdir -p "$(dirname "$log_path")"

set +e
cargo test --workspace --all-features 2>&1 | tee "$log_path"
cargo_status=${PIPESTATUS[0]}
set -e

passed=0
failed=0
ignored=0
measured=0
filtered=0
suites=0

while IFS= read -r line; do
  if [[ "$line" =~ test\ result:\ [A-Za-z]+\.\ ([0-9]+)\ passed\;\ ([0-9]+)\ failed\;\ ([0-9]+)\ ignored\;\ ([0-9]+)\ measured\;\ ([0-9]+)\ filtered\ out ]]; then
    passed=$((passed + BASH_REMATCH[1]))
    failed=$((failed + BASH_REMATCH[2]))
    ignored=$((ignored + BASH_REMATCH[3]))
    measured=$((measured + BASH_REMATCH[4]))
    filtered=$((filtered + BASH_REMATCH[5]))
    suites=$((suites + 1))
  fi
done < "$log_path"

total=$((passed + failed + ignored + measured))
summary_target="${GITHUB_STEP_SUMMARY:-}"

write_summary() {
  {
    echo "### Rust test summary"
    echo
    echo "**Failed / total:** ${failed} / ${total}"
    echo
    echo "| Metric | Count |"
    echo "| --- | ---: |"
    echo "| Suites | ${suites} |"
    echo "| Passed | ${passed} |"
    echo "| Failed | ${failed} |"
    echo "| Ignored | ${ignored} |"
    echo "| Measured | ${measured} |"
    echo "| Filtered out | ${filtered} |"
    echo
    echo "<details><summary>Last 200 lines of cargo test output</summary>"
    echo
    echo '```text'
    tail -n 200 "$log_path"
    echo '```'
    echo
    echo "</details>"
  }
}

if [[ -n "$summary_target" ]]; then
  write_summary >> "$summary_target"
else
  write_summary
fi

echo "Rust tests: ${failed}/${total} failed (${passed} passed, ${ignored} ignored, ${suites} suites)"
exit "$cargo_status"
