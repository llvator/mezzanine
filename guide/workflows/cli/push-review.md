# Push review

> How do I get structural feedback without remembering to ask for it?

Every other workflow here is **pull**: it works only when someone thinks to
run it. An agent that forgets to call `assess_change` ships regressions
silently, and so does a human. Push mode inverts that — mezz's structural
signal arrives without being asked.

Two legs. The PR leg is advisory. The Stop-hook leg is advisory by default
and blocking under `--block`, which is what makes it a loop rather than a
broadcast — see below.

## Leg 1: the Stop hook

Wire `mezz hook self-review` into a Claude Code Stop or PostToolUse hook. It
reports structural regressions of the working tree against a git ref.

Set it up with one command:

```bash
mezz init --hooks        # merges into an existing .claude/settings.json
mezz init --hooks --force  # re-write the legs mezz wrote, leaving yours alone
```

which writes `.claude/settings.json`:

```json
{
  "hooks": {
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "o=$(mezz hook self-review --block 2>/dev/null); r=$?; [ -n \"$o\" ] && echo \"$o\" >&2; [ $r -eq 2 ] && exit 2; exit 0",
            "timeout": 120,
            "statusMessage": "mezz self-review"
          }
        ]
      }
    ]
  }
}
```

It follows the **LSP-diagnostics token contract**, and every clause of that
contract is there to stop the hook becoming noise you learn to ignore:

| Property | Why |
| --- | --- |
| Silent when clean | A hook that always prints is a hook you stop reading |
| Each finding surfaced once per session | Fingerprint state file; repetition trains dismissal |
| Severity floor (`--min-severity`) | A combined cyclomatic + nesting rise below 3.0 is metric noise, never a finding |
| Hard line cap (`--cap`, default 10) | Beyond that it points you at `assess_change` instead of flooding |

Determinism (AN-002) is what makes the fingerprints stable across runs — the
same unchanged regression hashes the same way, so "once per session" actually
holds.

**Cost.** On this repo the hook analyzes two full graphs — the base worktree
and the working tree, ~11.7k entities each — in **1.5 seconds**, with 275/275
parse-store hits. That is what makes it viable on every stop rather than a
thing you disable after a week.

Options worth knowing:

```bash
mezz hook self-review --base-ref main       # compare against a branch point
mezz hook self-review --min-severity medium # raise the floor
mezz hook self-review --cap 20              # raise the cap
mezz hook self-review --block               # block the stop on a regression
```

The state file defaults to a temp-dir path keyed by repo + base SHA, so it
resets naturally when a new commit moves the base.

### Why `--block`, and why the shell wrapper

Without `--block` the hook exits 0, and a `Stop` hook that exits 0 has its
stdout written to the **debug log only** — not the transcript, and never the
agent's context. The findings are computed, rendered, and dropped. Only three
events (`UserPromptSubmit`, `UserPromptExpansion`, `SessionStart`) feed plain
stdout to Claude; `Stop` is not one of them.

`--block` exits **2** instead, which is the one channel `Stop` has: Claude Code
blocks the stop, hands the agent the hook's stderr, and the turn continues so
the agent can fix what it just broke.

The wrapper exists because findings and the analyzer's progress output cannot
share a stream. Findings go to stdout; the `Analyzing base ...` chatter goes to
stderr. The wrapper drops the chatter, re-emits the findings on stderr where a
blocked stop will read them, and propagates only exit 2 — so a missing or
broken `mezz` can never wedge a session.

**Nothing blocks twice.** A finding enters the session state the moment it is
emitted, so the run after a block no longer counts it as new. Each regression
costs at most one extra turn. Resolved findings never block at all: stopping an
agent to tell it the tree improved spends a turn and teaches it the channel is
noise.

## Leg 2: the PR comment

`mezz pr-report` renders the same assessment as a PR comment body — the full
report when the change is structural, a one-liner when it is not.

```bash
mezz pr-report --base-ref "$(git merge-base origin/main HEAD)" > report.md
```

It **always exits 0**, so a CI job wiring it in stays non-blocking. That is
deliberate: mezz is a signal, not a gate. A reference GitHub Actions workflow
lives in [`.github/workflows/`](../../../.github/workflows/) — it posts one
comment and edits it in place on every push, found by the
`<!-- mezz-pr-report -->` marker, so a long-running PR accumulates one comment
rather than thirty.

## Reading the output

Both legs report the same three things:

- **New smells** — a God Class or Dispatcher that was not there at the base.
- **Cycles** — a dependency cycle the change introduced.
- **Above-floor complexity growth** — with the before and after numbers.

The value is that these arrive attached to the diff that caused them, while
the reasoning is still in your head, instead of surfacing three months later
in a quality report nobody commissioned.

## Limits

- Both legs run in a fresh process and do not use the long-lived MCP server's
  warm cache — they rely on the on-disk parse store instead. A genuinely cold
  store makes the first run slow.
- `--base-ref` defaults to `HEAD`, which means "uncommitted work only." For a
  branch's whole story, pass the merge-base.
- The hook is advisory by construction. If you want a gate, that is the
  complexity ceiling in [CONTRIBUTING.md](../../../CONTRIBUTING.md), which is
  a separate diff-only CI check.
