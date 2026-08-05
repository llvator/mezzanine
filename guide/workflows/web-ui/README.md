# Visualizer workflows

Driven on a canvas: the VS Code extension panel and the browser UI, which are
the same Svelte build talking to the same engine. Output is a pattern you
notice rather than a value you read.

| Workflow | Answers |
| --- | --- |
| [The altitude descent](altitude-descent.md) | I am new here — what is this system, from the domain down to the function? |
| [Quality scatter](quality-scatter.md) | What *kind* of trouble is this code in, and is it local or spread? |
| [Follow selection](follow-selection.md) | What is around the code I am editing, without going to look? |
| [Diff review](diff-review.md) | Did this change the shape of the system, not just its text? |
| [Cohesion audit](cohesion-audit.md) | Are these folders real boundaries or just names? |
| [The docs graph](docs-graph.md) | What does the documentation cover, contradict, and no longer reach? |
| [Scope decoupling](scope-decoupling.md) | What does this subsystem depend on, with fan-in still telling the truth? |
| [Canvas to agent](canvas-to-agent.md) | I found the problem on the canvas — now fix it. |

## Getting a canvas up

Either surface works for every workflow here.

```bash
# VS Code: Command Palette → "Nao: Open Code Visualizer"

# Browser, engine serves the UI:
nao watch . --port 3200        # then open http://localhost:3200

# Browser, UI served separately:
nao watch . --port 3200 --allow-origin http://localhost:4173
```

## The keymap

Press `?` on any pane for the live list — it is generated from the same table
the handlers dispatch on, so it cannot drift from what the keys actually do.
Panes are focused by digit: `0` sidebar, `1` graph, `2` view, `3` details,
`4` description.

| Scope | Keys |
| --- | --- |
| Global | `?` help · `l` freeze hover · `mod+k` search |
| Sidebar | `f` filters · `q` quality · `s` settings · `/` search · `c` collapse |
| Graph | `p` pin hovered · `x` clear · `f` fit · `w` fit width · `+`/`-` zoom · `r` reset · `t` tree/graph · `n` labels |
| View | `e`/`f`/`m` entity/file/module level · `t` tree/graph · `a` auto-fit · `s` spacing · `d` highlight depth · `o` hover mode · `n`/`k`/`b` node/kind/link labels |
| Details | `p` pin/unpin · `x` unpin |
| Description | `h` follow hover |

## What the canvas is not for

Decide this up front or the visualizer becomes a thing you open and close
without it changing any decision:

- **Precise blast radius** — use `impact`. The exact-vs-heuristic edge markers
  only exist in the text tools, and the difference matters.
- **Anything you need to paste** into a ticket, review, or commit message —
  use the text tools, which are already prose.
- **Exhaustive enumeration** — force layouts hide nodes behind nodes. "I did
  not see it" is not "it is not there."
- **Readings that depend on exact position** — the layout is seeded, but a
  force simulation settles differently enough that position is a hint, not a
  measurement.
