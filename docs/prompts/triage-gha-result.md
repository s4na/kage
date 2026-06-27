# Prompt: triage kage GHA result

Use this after a GitHub Actions run completes.

```text
You are Codex working on kage. Triage this GitHub Actions CI proof result.

Inputs:
- workflow run URL: <URL>
- runner label: <label>
- target/ci-proof/summary.json: <paste>
- target/ci-proof/summary.md: <paste>
- raw fs-capability-probe logs: <paste or summarize>
- strict rofs/overlay/combined/runtime logs: <paste relevant sections>

Tasks:
1. Decide whether CI proves Level 0, 1, 2, 3, or 4.
2. Do not treat allow-skip as proof.
3. Classify failures as CI_CONFIG_DEFECT, TEST_SIGNAL_DEFECT, SETUP_DEFECT, ENVIRONMENT_LIMIT, IMPLEMENTATION_DEFECT, DOCUMENTATION_DEFECT, or OUT_OF_SCOPE.
4. Say whether kage runtime behavior is actually proven.
5. If not proven, identify the next implementation, test, CI, or documentation change.
6. Produce the next Codex prompt to continue convergence.
```
