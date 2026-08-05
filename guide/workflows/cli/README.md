# Terminal workflows

Driven from a shell: the `nao` and `elevator` binaries, plus the MCP tools a
coding agent calls in the same session. Output is text — pipeable, diffable,
and quotable in a commit message or a review.

| Workflow | Answers |
| --- | --- |
| [Refactor targeting](refactor-targeting.md) | Where does refactoring pay, what exactly is wrong there, what breaks, how do I verify it? |
| [The agent loop](agent-loop.md) | How should a coding agent spend its context budget on an unfamiliar task? |
| [Push review](push-review.md) | How do I get structural feedback without remembering to ask for it? |
| [Spec-first tasks](spec-first-tasks.md) | Which part of the domain does this task belong to, and what did it claim before I started? |
| [Spec health](spec-health.md) | Is the spec still telling the truth about the code? |
| [Before you write](before-you-write.md) | Does this already exist, and is any of this still used? |

## Two surfaces, one engine

Some capabilities are CLI subcommands, some are MCP tools, and the split is
not arbitrary — the CLI predates the agent surface and grew from different
questions. It is worth knowing which is which before you go looking for a
command that does not exist:

| Capability | CLI | MCP tool |
| --- | --- | --- |
| Whole-graph analysis, any output format | `nao analyze` | — |
| Dependencies of a file/entity | `nao deps` | `impact`, `trace` |
| Find an entity by name | `nao find` | `similar` |
| Circular dependencies | `nao cycles` | part of `quality` |
| Counts and totals | `nao stats` | part of `map` |
| Structural diff of two commits | `nao diff` | `assess_change` (working tree vs a ref) |
| Churn × complexity risk | — | `hotspots` |
| Smells and refactor pressure | — | `quality` |
| Folder shape with metrics | — | `map` |
| Tests reaching an entity | — | `tests_for` |
| Edit context pack | — | `context` |
| Unreferenced entities | — | `dead_code` |
| Domain map from `.elv` | `elevator` | `overview`, `spec_slice` |

The MCP-only tools are the ones built for an agent's economics: ranked,
capped, and filtered of noise. Nothing stops you reading them yourself — ask
your agent to run one and show you the output.

See [../../../docs/agents/mcp-server.md](../../../docs/agents/mcp-server.md)
for the full tool reference.
