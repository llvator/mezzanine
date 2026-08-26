# Contributing to Mezzanine

Thanks for your interest. This document is everything you need to get a
change merged. It is also what CI enforces, so following it is the fastest
path to a green build.

New to the project? Start with
[guide/getting-started.md](guide/getting-started.md) for a 5-minute
install-and-run of the `mezz` CLI, the `elevator` CLI, and the VS Code
extension.

## Before you start

For anything beyond a small fix, **open an issue first**. It's cheaper to
agree on the approach in an issue than to discover a mismatch in review.
Small, obviously-correct fixes (typos, a clear bug with a clear fix) can go
straight to a PR.

## One-time setup

```sh
git clone https://github.com/llvator/mezzanine.git
cd mezz
cargo build
bash scripts/install_hooks.sh
```

That last step symlinks `.git/hooks/pre-commit` to
[scripts/hooks/pre-commit](scripts/hooks/pre-commit), which regenerates the
**Repo health** table in the README whenever you stage changes under `src/`,
so the published metrics never drift from the code. It exits in milliseconds
when no `src/*.rs` files are staged.

Two other scripts are worth knowing about:

- `./scripts/build.sh` — recompile in place without installing, for
  iteration. Note `ui/` has two build targets: `--ui` writes `ui/dist` (the
  browser UI `mezz watch` serves) and `--webview` writes `webview-dist`
  (bundled into the `.vsix`).
- `./scripts/install.sh` — full build *and* install of both binaries plus
  the VS Code extension. See
  [guide/getting-started.md](guide/getting-started.md).

## The four-step self-check

Run these before you push. Most of what CI checks is reproducible locally in
about 30 seconds.

```sh
# 1. Build is clean
cargo build

# 2. All tests pass
cargo test

# 3. No new clippy warnings introduced by your change
#    (`-D warnings` is not enforced in CI — the codebase has pre-existing
#    lints. Aim not to add new ones.)
cargo clippy --all-targets

# 4. New code is below the complexity ceiling
cargo run --quiet --bin mezz -- analyze -f json -l rust src/ 2>/dev/null \
  | jq -r '.entities[]
      | select(.metrics.cyclomatic != null)
      | select(.metrics.cyclomatic > 15 or .metrics.cognitive_complexity > 22 or .metrics.max_nesting > 4)
      | "\(.metrics.cyclomatic)\t\(.metrics.cognitive_complexity)\t\(.metrics.max_nesting)\t\(.name)\t\(.file_path)"' \
  | sort -k1 -n -r
```

If your PR touches `ui/`, also run:

```sh
cd ui
npm run check             # type errors
npm test                  # matcher + scope-rule unit tests
npm run build             # bundles cleanly
```

**Run `npm run check`, not a bare `npx svelte-check`.** Without
`--tsconfig ./tsconfig.app.json` — which is what the script passes —
svelte-check falls back to a default project and sees about 100 of the 170
files. It reports a confident zero while three type errors sit in a component
you just wrote. The baseline for `npm run check` is **10 errors**: nine d3
generics in `GraphView.svelte` and one in `collapseGraph.ts`. Anything above
that is yours.

There's no automated UI complexity gate. Apply judgement: components above
~300 lines or with deeply nested template logic should be split.

### Colours

Component colours come from the theme's CSS custom properties — `--text`,
`--text-secondary`, `--text-muted`, `--text-dim`, `--bg-surface`, `--accent`
and friends. Don't write a literal, and don't invent a property name: a
`var(--bg-secondary, #2a2a2a)` fallback looks defensive but silently pins the
component to one theme, because nothing defines `--bg-secondary`. Forty-two
such references had accumulated.

Two kinds of colour are deliberately *not* theme-derived: per-entity-kind and
per-language palettes, and the diff add/remove/modify colours. Those identify
things and must stay stable across themes. They belong on a swatch next to
themed text (see `ColorChip.svelte`), not as the text colour itself — a hue
picked to separate nodes on a dark canvas is not a legible text colour on a
light panel.

`node ui/scripts/ux-probe.mjs ui-023` audits every rendered text/background
pair in all four themes and separates component literals from palette tokens.

### Layout probes

If your change touches the browser UI's layout, panels, toolbar, themes or
the graph canvas, run the probe harness as well:

```sh
mezz watch . --port 3000                           # terminal 1
cd ui && npm run dev -- --port 5199 --strictPort  # terminal 2
node ui/scripts/ux-probe.mjs --all                # terminal 3
```

[ui/scripts/ux-probe.mjs](ui/scripts/ux-probe.mjs) asserts the things
`svelte-check` cannot: panel widths, wrapped toolbar rows, scroll
oversubscription, clipped graph nodes, control overlap at narrow viewports,
and label contrast across all four themes. It talks CDP to an installed
Chrome over Node's built-in `WebSocket` — no dependencies and nothing added
to `package.json`. Exit code is the number of failed checks.

`--all` is slow (the theme suite reloads the app four times); pass suite
names to narrow it. `node ui/scripts/ux-probe.mjs` with no arguments lists
them.

Many checks fail on `main` today — they were written against a UX review and
describe the target state, not the current one. Run the suites relevant to
your change and compare against the baseline recorded in the corresponding
issue rather than expecting a clean sweep.

If you add a check, make it fail before it passes. Assert on transitions
rather than static state, drive toggles to a known value instead of clicking
blindly, and prefer an explicit `data-probe="…"` hook over a regex across a
panel — loose patterns match unrelated text and pass against broken UI. A
false pass is worse than no check; if something can't be asserted honestly,
leave it as a manual step.

## Complexity ceiling

Mezzanine analyzes its own source, and CI enforces a ceiling on every PR:

| Metric | Ceiling |
|---|---|
| Cyclomatic complexity | ≤ 15 |
| Cognitive complexity | ≤ 22 |
| Max nesting depth | ≤ 4 |

These are the p90 of the codebase at the time the gate was introduced.

The gate is **diff-only**: it fails on functions you *add* above the ceiling,
or existing functions your change pushes *higher*. Functions that were
already over when the gate landed are grandfathered — don't fix them as a
drive-by; open an issue instead.

Step 4 above is deliberately conservative and lists grandfathered code too.
Each line reads `cyclo  cognitive  nesting  function_name  file_path`:

```
54  186  10  extract_calls            parser/python/calls.rs
33   36   3  compute_composite_score  models/scope_metrics.rs
```

To reproduce the exact CI gate, compare two snapshots:

```sh
cargo build --release --bin mezz

git stash
./target/release/mezz analyze -f json -l rust src/ > /tmp/baseline.json
git stash pop

./target/release/mezz analyze -f json -l rust src/ > /tmp/current.json

python3 .github/scripts/check_complexity.py /tmp/baseline.json /tmp/current.json
```

It exits 0 if the gate passes, 1 with diagnostics if not. Most over-budget
functions are over on *cognitive* complexity, which usually means too many
nesting levels, too many `match` arms in one place, or a control-flow walker
that should be split per node-kind.

## PR title rules

The PR title becomes the merge commit message, so it shows up in `git log`
regardless of merge strategy. CI checks the mechanical parts:

- **Imperative mood, present tense.** "Add", not "Added" or "Adding".
- **No prefix.** No `feat:`, `fix:`, `refactor:`, no scope brackets.
- **Under 70 characters.** It has to fit in `git log --oneline`.
- **Starts with a capital letter, no trailing period.**
- **Concrete.** Name the behavior change a user or reviewer would notice,
  not the mechanics.

From the existing log:

```
Disambiguate same-name entity lookups by file locality
Surface more Python structure and tune ghost classification
Auto-collapse graph to file/module level above render budget
```

## Commit messages

The git log is read more often than any other documentation here. A coherent
log lets you `git blame` a line and immediately understand the why.

- One blank line after the title, then prose wrapped at 72 chars.
- **Why, not what.** The diff shows what. The body explains the constraint,
  the prior failure mode, or the tradeoff.
- Reference the issue it resolves, if there is one.
- If an AI agent contributed, end with a `Co-Authored-By:` line.

**Don't:** bundle unrelated changes (if your description has "and" or "also",
split it); commit generated files (`target/`, `node_modules/`); commit
`.env`, credentials, or local config; or skip hooks with `--no-verify`.

## How many commits per PR

One is the default — a clean implementation usually fits in one commit. Two
or three is fine when there's a clear ordering. More than five suggests the
PR is too big or the commits aren't logically grouped.

Each commit should leave the codebase in a working state. No "WIP" or
"fixup" commits in merged history.

## A note on issue references in comments

Comments cite issue codes like `AN-006` or `UI-014`:

```rust
// AN-006. Rust module paths: a free function in `src/diff.rs` is called
```

They record *why* a decision was made and point into a maintainer-internal
tracker you won't have. Treat them as provenance, not as required reading —
the comment beside the code should stand on its own, and if it doesn't, that
is a bug in the comment worth reporting.

Use one in your own comments only if you're citing an issue you can link to.
Prose is better than an unresolvable code.

## License and the CLA

Mezzanine is dual-licensed: [AGPL v3](LICENSE) for everyone, and a
[commercial licence](COMMERCIAL.md) for organisations that cannot accept the
AGPL. Offering both requires holding rights broad enough to grant both, which
the AGPL alone does not provide — so contributions are covered by a
[Contributor Licence Agreement](CLA.md).

Read it once; it is short and the reasoning is stated in it. In summary: you
keep ownership of everything you write and may reuse it anywhere, and you
grant the maintainers a licence broad enough to offer it under both licences.
It is not a copyright assignment.

To accept it, sign off your commits and say so in your first PR:

```sh
git commit -s        # adds a Signed-off-by: line
```

**If you are contributing work written in the course of employment**, your
employer may own it. Section 4 of the CLA covers what to do; sort it out
before opening the PR rather than after review.
