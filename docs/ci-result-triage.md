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
