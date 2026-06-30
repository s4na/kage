# CI result triage

After GitHub Actions runs, download the `kage-ci-proof-fs-capability-probe` artifact and paste the following to a reviewer or ChatGPT.

```text
Please triage this kage GitHub Actions filesystem proof result.

Workflow run URL:
<URL>

Runner label:
<ubuntu-24.04 or other label>

summary.json:
<paste target/ci-proof/summary.json>

summary.md:
<paste target/ci-proof/summary.md>

Raw fs-capability-probe logs:
<paste target/ci-logs/fs-probe.log or attach artifact>

Strict test logs:
- strict_rofs.log:
<paste relevant failure section>
- strict_overlay.log:
<paste relevant failure section>
- strict_combined.log:
<paste relevant failure section>
- strict_runtime.log:
<paste relevant failure section>

Failed command, if any:
<command>

Proof level reached:
<level>

Exact classification:
<classification and detail>

Was allow-skip used?
<true/false>

Question: Does this CI run prove kage runtime behavior, show a GitHub-hosted environment limitation, or reveal an implementation/test/CI defect? What should Codex fix next?
```

## RSpec/application-test failures

When a CI log only reports aggregate counts such as `Examples=2091, Failures=1`
or uploads a result artifact without printing the failed example, do not classify the
run as flaky yet. First inspect the uploaded result JSON/XML or rerun the single
failed example so the triage includes:

- failed example description and spec file/line;
- random seed, parallel node, and worker id;
- failure message/backtrace;
- whether a retry or a later run on the same commit passed.

Use `INSUFFICIENT_TEST_DETAIL` for summary-only logs, `DETERMINISTIC_TEST_FAILURE`
for a reproducible named failure, and `FLAKE_SUSPECTED` only after there is
evidence of intermittent behavior on the same code.
