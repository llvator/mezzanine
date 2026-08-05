# The agent loop

> How should a coding agent spend its context budget on an unfamiliar task?

An agent has a specific economics: every token costs, and the global view
never fits in the window. The MCP tools are shaped for that — compact ranked
text, capped with explicit truncation notes, filtered of noise (ghosts,
parameters, imports), and every tool `readOnlyHint: true`.

This is the loop those tools were designed around. Each step is one call.

## The eight steps

| # | Step | Tool | Instead of |
| --- | --- | --- | --- |
| 1 | Ground | `overview` | Reading the README and guessing |
| 2 | Orient | `map` | Reading ten files to learn a folder |
| 3 | Locate | `similar` | Writing a helper that already exists |
| 4 | Prepare | `context` | Five file reads to assemble one edit |
| 5 | Check reach | `impact`, `trace` | Hoping nothing else calls it |
| 6 | Edit | LSP / plain edits | — |
| 7 | Verify | `tests_for` | Running the whole suite |
| 8 | Self-review | `assess_change` | Presenting and hoping |

### 1. Ground — `overview`

When the project has an Elevator (`.elv`) layer, this is the cheapest
orientation available: the whole domain in ~60 lines, with `cr:` pointers
into the implementing code.

```
overview {}
overview { "focus": "f.parsers" }
```

Skip when there is no `.elv` layer — the tool says so rather than guessing.

### 2. Orient — `map`

```
map { "path": "src/mcp", "depth": 2 }
```

Files and entities with loc, complexity, and coupling. Cheaper and more
complete than reading the files, and it tells you which ones are worth
opening.

### 3. Locate — `similar`

```
similar { "query": "resolve git ref to sha" }
```

```
- [0.72] function `resolve_git_ref(repo_root: &Path, git_ref: &str) -> AnyhowResult<String>`
    — src/diff.rs:307 (matched: git, ref, resolve)
```

Results below 0.40 are dropped even when `top` is unfilled. **An empty result
is the answer**, not a failure — it means implementing fresh is reasonable.
That contract is why the floor is absolute rather than relative.

### 4. Prepare — `context`

```
context { "entity": "render_change_report" }
```

The target's full source plus the *signatures* of everything it uses and
everything using it. One call replaces the file-hopping that would otherwise
pull whole neighbouring files into the window for one function each.

### 5. Check reach — `impact` and `trace`

`impact` for who breaks. `trace` for how control arrives here:

```
trace { "from": "main", "to": "compute_diff" }
```

Up to three shortest chains with the relationship kind of every hop; it
checks the reverse direction when no forward path exists.

### 7. Verify — `tests_for`

```
tests_for { "entity": "compute_diff" }
```

Run what this returns. Tests are found even when the session excludes test
files from analysis.

### 8. Self-review — `assess_change`

Before presenting the work:

```
assess_change { "base_ref": "HEAD" }
```

Metric deltas, added/removed entities, smell churn, and a **Spec claims**
section naming the spec entities whose descriptions your change may have
invalidated.

## Why the loop is trustworthy

Two properties, both load-bearing:

- **Analysis is deterministic** (AN-002). Identical trees produce
  byte-identical graphs, so every delta `assess_change` reports is real
  signal rather than reordering noise. It is also what makes the warm cache
  safe: stale-old is possible, stale-wrong is not.
- **Type usage is exact** where parsers emit `UsesType` edges — Rust,
  TypeScript/Svelte, Java, Kotlin, Python, Groovy. Elsewhere `impact` falls
  back to a "used via members" approximation, and says so.

## Cost

The first call after a source change pays a full re-analysis; subsequent
calls come from the warm cache in milliseconds. Underneath sits a
content-hashed on-disk parse store, so a fresh `nao` process on an
already-analyzed repo is warm from call one — editing one file re-parses
exactly that file.

In practice on this repo: two full 11.7k-entity graphs, 275/275 parse-store
hits, **1.5 seconds**.

## Registration

`.mcp.json` is checked in, so Claude Code picks the server up automatically
here. For other projects:

```bash
claude mcp add nao -- nao mcp
```

## Limits

- An empty `impact` result is strong evidence, not proof — see
  [refactor-targeting.md](refactor-targeting.md#3-what-breaks--impact).
- `map` and `quality` currently return empty for a **single file** path —
  pass directories.
- `similar` v1 is identifier- and signature-based: it finds *namable*
  duplication, not structural clones that share no vocabulary.
