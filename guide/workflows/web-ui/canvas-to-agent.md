# Canvas to agent

> I found the problem on the canvas — now fix it.

The visualizer is a noticing tool. Claude Code is a doing tool. This closes
the gap between them, so a finding does not have to survive being retyped as
a path and a description.

## Turning it on

Off by default. It has to be asked for:

```bash
nao watch . --port 3200 --allow-agent-spawn
```

Then click a node and refactor it — the UI opens a Claude Code terminal on
this machine, scoped to that entity.

| Variable | Purpose |
| --- | --- |
| `NAO_TERMINAL` | Which terminal to open |
| `NAO_CLAUDE_BIN` | Claude Code binary, when it is not on `PATH` |

## Why it is off by default

This is the one route that **runs code rather than serving data**, so it does
not exist unless you ask for it — and it **always requires the pairing
token**, including from loopback, where reading the graph does not.

That asymmetry is deliberate. Binding to loopback is not a security boundary
against a browser: any page you happen to have open is on the same machine as
the server. For reading the graph, the origin allowlist is enough. For
spawning a process, it is not.

If you are running the UI on a non-loopback origin, you already need
`--allow-origin` plus the token to read anything at all; see
[getting-started.md](../../getting-started.md).

## The workflow

1. Notice something — a red node in the [quality scatter](quality-scatter.md),
   a folder that scattered in the [cohesion audit](cohesion-audit.md), an
   unexpected crossing under [scope decoupling](scope-decoupling.md).
2. Select the node.
3. Spawn the agent on it.
4. The agent works with the MCP tools available — so its first moves are
   `context` and `impact` on the entity you pointed at.

The handoff is the value: you spent your attention on *noticing*, which is
what a human with a canvas is good at, and the agent spends its context on
*mechanism*, which is what it is good at. Neither has to do the other's job,
and the entity identity passes between them without you transcribing
anything.

## Pairing it with self-review

If [push review](../cli/push-review.md) is wired up, the spawned agent's work
gets structurally reviewed on stop without anyone asking. Notice → delegate →
verify, with a human decision at each end and neither middle step depending
on someone remembering.

## Limits

- **One machine.** The route spawns a local process; it is not a remote
  execution path and should never become one.
- The agent inherits your repository state, including uncommitted work.
  Commit or stash first if you want a clean base for `assess_change`.
- Pointing an agent at a node is not a specification. "Refactor this" gets
  you a plausible refactor of the thing you clicked — if the finding was that
  the *boundary* is wrong, say so, because the canvas showed you that and the
  node alone does not carry it.
