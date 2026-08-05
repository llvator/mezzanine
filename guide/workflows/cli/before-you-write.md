# Before you write

> Does this already exist, and is any of this still used?

Two cheap questions that prevent two expensive outcomes: a fourth helper that
does what three others already do, and a cleanup that deletes something still
reachable.

## Does it already exist — `similar`

Call it before implementing anything named. One call, answered in
milliseconds from the warm cache.

```
similar { "query": "resolve git ref to sha" }
```

```
- [0.72] function `resolve_git_ref(repo_root: &Path, git_ref: &str) -> AnyhowResult<String>`
    — src/diff.rs:307 (matched: git, ref, resolve)
- [0.48] function `git_sha(path)` — scripts/call_edge_recall.py:277 (matched: git, sha)
```

### Reading the score

The score is the fraction of your query's token *weight* the entity matches,
discounted when the match falls outside the entity's own name. A match
covering the whole query in the name scores 1.00.

- **Common English words carry almost no weight.** `to`, `of`, `for` are
  worth 0.15 of a content word, and an entity whose *every* matched token is
  a function word is dropped outright — `to` must never lift `toRef` into the
  results for "resolve git ref to sha".
- **Below 0.40 is dropped**, even when `top` is unfilled. The floor is
  absolute, not a fraction of the best hit, because a relative floor always
  returns *something* — and the tool's contract is that an empty result means
  implementing fresh is reasonable.
- **`top` is a ceiling, not a quota.** Two results when you asked for ten is
  the tool working.
- The `matched:` trace names the responsible tokens, so a surprising rank
  stays debuggable rather than mysterious.

### The CLI cousin

```bash
nao find compute_diff src
```

```
function compute_diff_blocking in .../src/server/diff_handler.rs [L85]
function compute_diff in src/diff.rs [L174]
```

`find` is exact-ish name matching; `similar` is semantic-ish over identifier
and signature vocabulary. Use `find` when you know the name, `similar` when
you only know the intent.

### Limit

`similar` v1 matches **identifiers and signatures**, so it finds *namable*
duplication. Two functions implementing the same algorithm with entirely
different vocabulary will not surface. Structural similarity (token shingles
or AST shapes) is roadmap, not shipped.

## Is any of this still used — `dead_code`

```
dead_code { "path": "src/mcp" }
```

```
# Dead-code candidates in src/mcp — 0 in 0 files
_Also fan-in 0, not reported: 17 test, 0 entry point, 0 public API,
 0 mentioned elsewhere in the source._
```

### Why the header matters

Fan-in 0 alone is not a finding, and a tool that treated it as one would be
useless. Four populations are removed first, and **the header reports each
count** so you can watch the list narrow rather than wonder what was hidden:

| Gate | Why "no dependent" is not evidence of death |
| --- | --- |
| **Tests** | Tests are forced into the graph, so a function whose only caller is a test has fan-in 1 and never becomes a candidate at all |
| **Entry points** | `main`, Python dunders, and contract members (trait impls, `override`, an outgoing `Implements` edge) are reached through an abstraction, not by name |
| **Public API** | A published `pub` contract has callers you cannot see. Only applied for languages whose parser reads visibility off an explicit modifier |
| **Mentioned elsewhere** | An identifier scan over the same files, skipping the name's own definition span — this catches macro bodies, reflection, and string dispatch |

That last gate does the heaviest lifting. A Rust macro body reaches the
parser as an opaque token tree, so a helper called only from inside a
`format!` has fan-in 0. Without the scan, **11 of the first 14 results on
`src/mcp` were false positives.** The gate is one-directional by design: it
only ever *suppresses*, so a name appearing in a comment costs you a true
positive, but nothing can invent one.

### Two flags worth knowing

- **`include_public: true`** — turn the public-API gate off for an
  application or binary, where `pub` is convenience rather than a published
  contract. Leave it off for a library.
- **`path` narrows the report, not the analysis.** Fan-in is always computed
  over the whole project. A graph built from one subdirectory cannot see the
  callers outside it and would report that folder's entire surface as dead.

### Verification

The default view was checked against `rustc`'s own `dead_code` lint on this
repo: it returns exactly that lint's findings which survive the four gates.
The tool errs toward under-reporting — still verify a candidate before
deleting it, because no static graph sees FFI, dynamic dispatch by string, or
a name assembled at runtime.

## Also useful before a cleanup

```bash
nao cycles src      # circular dependencies, as chains
nao stats src       # entity and relationship counts, most-connected entities
```

`nao stats`' "most connected" list includes unresolved external names
(`Some`, `String`, `Node`) alongside your own types — read past them to the
first entity you recognise.
