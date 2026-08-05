# Workflows

Nao is one analysis engine behind several surfaces. This folder collects the
working patterns each surface is actually good at, so you can pick the one
that fits the moment rather than reaching for whichever surface you opened
last.

The split is by **how you drive it**:

| Folder | You are | The interaction |
| --- | --- | --- |
| [`cli/`](cli/) | in a terminal | You type a command, or your coding agent calls a tool. Output is text you can pipe, diff, paste, and commit. |
| [`web-ui/`](web-ui/) | in the visualizer | You look at a canvas. Output is a pattern you notice — shape, clustering, asymmetry, absence. |

`cli/` covers both binaries (`nao`, `elevator`) and the MCP tools your agent
calls in the same session, because both are terminal-side and answer the same
way. `web-ui/` covers the VS Code extension panel and the browser UI, which
are the same Svelte build against the same engine.

## Choosing between them

The distinction that matters is not "visual vs textual" — it is **what kind
of question you have**.

- Text tools answer **questions you know how to ask**. You have a name, a
  path, or a ref, and you want a precise answer about it.
- The canvas is for **noticing what you didn't know to ask about**. You have
  a feeling that an area is tangled, or you have never seen this subsystem
  before, and you need its shape before you can form a question.

If you already know the entity's name, `impact` beats squinting at edges. If
you don't know what you're looking for yet, no amount of ranked text will
tell you the folder structure stopped meaning anything.

## The workflows

### Terminal — [`cli/`](cli/)

| Workflow | Answers |
| --- | --- |
| [Refactor targeting](cli/refactor-targeting.md) | Where does refactoring pay, what exactly is wrong there, what breaks, how do I verify it? |
| [The agent loop](cli/agent-loop.md) | How should a coding agent spend its context budget on an unfamiliar task? |
| [Push review](cli/push-review.md) | How do I get structural feedback without remembering to ask for it? |
| [Spec-first tasks](cli/spec-first-tasks.md) | Which part of the domain does this task belong to, and what did it claim before I started? |
| [Spec health](cli/spec-health.md) | Is the spec still telling the truth about the code? |
| [Before you write](cli/before-you-write.md) | Does this already exist, and is any of this still used? |

### Visualizer — [`web-ui/`](web-ui/)

| Workflow | Answers |
| --- | --- |
| [The altitude descent](web-ui/altitude-descent.md) | I am new here — what is this system, from the domain down to the function? |
| [Quality scatter](web-ui/quality-scatter.md) | What *kind* of trouble is this code in, and is it local or spread? |
| [Follow selection](web-ui/follow-selection.md) | What is around the code I am editing, without going to look? |
| [Diff review](web-ui/diff-review.md) | Did this change the shape of the system, not just its text? |
| [Cohesion audit](web-ui/cohesion-audit.md) | Are these folders real boundaries or just names? |
| [The docs graph](web-ui/docs-graph.md) | What does the documentation cover, contradict, and no longer reach? |
| [Scope decoupling](web-ui/scope-decoupling.md) | What does this subsystem depend on, with fan-in still telling the truth? |
| [Canvas to agent](web-ui/canvas-to-agent.md) | I found the problem on the canvas — now fix it. |

## Where to start

If you adopt exactly one thing from this folder, make it
[Follow selection](web-ui/follow-selection.md) — it costs nothing per use and
runs in the background of work you were doing anyway.

If you adopt two, add [Push review](cli/push-review.md), because it is the
only workflow here that does not depend on anyone remembering to run it.

## Prerequisites

All of these assume `nao` and `elevator` are installed and, for the agent
tools, that the MCP server is registered. See
[getting-started.md](../getting-started.md).
