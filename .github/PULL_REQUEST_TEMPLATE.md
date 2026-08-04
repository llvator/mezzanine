<!--
Read CONTRIBUTING.md before opening this PR.

PR title rules (CI-enforced):
  - <= 70 characters
  - No prefix ('feat:', 'fix(api):', 'AREA:', etc. are rejected)
  - Starts with a capital letter
  - No trailing period
  - Imperative mood, e.g. "Add walrus operator support" — not auto-checked
    but reviewers will catch tense/mood drift.
-->

## Affected files

<!--
One line per modified file, with a short clause explaining what changed there.
Reviewers read this and predict the diff.
-->

- path/to/file.rs — what changed and why

## Summary

<!-- 2-3 sentences. The "why", not the "what" — the diff shows what. -->

## Self-check

<!-- Tick each box once you've verified it. CI re-runs these but local feedback is faster. -->

- [ ] `cargo test` passes
- [ ] `cargo build` is clean (no new warnings)
- [ ] No new function exceeds the complexity ceiling (cyclo ≤ 15, cognitive ≤ 22, max_nesting ≤ 4) — see [CONTRIBUTING.md](../CONTRIBUTING.md)
- [ ] Existing functions touched by this PR did not get worse
- [ ] New behavior has tests
- [ ] No unrelated changes mixed in

## Notes for the reviewer

<!-- Optional. Anything subtle, surprising, or worth a closer look. Skip if there's nothing. -->
