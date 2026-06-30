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
1. First extract the exact failing test/example name, file path, seed/order, retry count, and failure message from artifacts or logs. If the pasted log only shows an aggregate failure count, say the failure is not yet triageable and request/download the result artifact before calling it flaky.
2. Decide whether CI proves Level 0, 1, 2, 3, or 4.
3. Do not treat allow-skip as proof.
4. Classify failures as CI_CONFIG_DEFECT, TEST_SIGNAL_DEFECT, SETUP_DEFECT, ENVIRONMENT_LIMIT, IMPLEMENTATION_DEFECT, DOCUMENTATION_DEFECT, or OUT_OF_SCOPE.
5. Say whether kage runtime behavior is actually proven.
6. For application test failures, distinguish these cases:
   - `FLAKE_SUSPECTED`: the same test has passed on retry or failed intermittently across runs with the same code.
   - `DETERMINISTIC_TEST_FAILURE`: a named test/example fails reproducibly.
   - `INSUFFICIENT_TEST_DETAIL`: only aggregate counts or artifact URLs are available, with no failing example details.
7. If not proven, identify the next implementation, test, CI, or documentation change.
8. Produce the next Codex prompt to continue convergence.
```
