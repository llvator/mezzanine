# The MCP warm cache: lazy invalidation with a generation counter

How `nao mcp` serves repeat tool calls in milliseconds without ever
returning a stale answer. This doc explains the general mechanisms
first, then walks the concrete implementation in `src/mcp/`.

## The problem being solved

Every MCP tool answers questions about the same artifact: the analyzed
dependency graph. Building that graph costs seconds (parse every file,
resolve names, run metric passes). An agent session calls tools in
bursts — `map`, then `context`, then `impact`, then `assess_change` —
against a tree that usually *hasn't changed between calls*. Paying full
analysis per call is pure waste; but serving yesterday's graph after an
edit would be worse than slow — it would be wrong.

So the requirements are:

1. Repeat calls on an unchanged tree must be near-free.
2. Any call after a change must reflect the change.
3. A change must never produce a *partially* updated answer.

## Mechanism 1: lazy invalidation ("invalidate cheap, rebuild on demand")

There are two classic ways to keep a cache fresh:

- **Eager (write-through):** when the source changes, rebuild the cache
  immediately, so it's always ready.
- **Lazy (invalidate-then-recompute-on-demand):** when the source
  changes, only *mark the cache stale* — a cheap operation. The
  expensive rebuild happens on the next read that actually needs it.

Nao chooses lazy, and the reason is the workload's shape. An agent
editing code produces *bursts of writes* (ten file saves in a minute)
followed by *occasional reads* (one `assess_change` at the end). Eager
rebuilding would re-analyze ten times and use one result. Lazy
invalidation marks stale ten times (nanoseconds each) and re-analyzes
once. The general rule: **eager favors read-heavy workloads, lazy
favors write-heavy ones.** Code editing is write-heavy between reads.

The cost of lazy is that the *first* read after a change pays the full
rebuild latency. That's the right place to pay it: the caller asking
the question is the one who needs the fresh answer.

## Mechanism 2: the generation counter ("epoch-based staleness")

How do you "mark stale" cheaply when there are many cache entries?
You don't touch the entries at all. You keep one number:

```
generation: AtomicU64      // the world's current version
```

Every cache entry remembers the generation it was built at:

```
CachedGraph { graph: Arc<DependencyGraph>, gen: u64 }
```

- **Invalidation** = `generation += 1`. One atomic increment
  invalidates every entry simultaneously, without iterating anything.
- **Lookup** = hit only if `entry.gen == generation`. An entry from an
  older generation is simply ignored (a miss), rebuilt, re-inserted
  with the current generation.

This is the *epoch* (or *version-stamp*) pattern. Its beauty is that
invalidation cost is O(1) regardless of cache size, and there is no
"forgot to invalidate entry X" class of bug — staleness is decided at
read time by comparing two integers, not at write time by hunting down
entries.

In the code: the counter lives on `McpServer.generation`
([src/mcp/mod.rs](../src/mcp/mod.rs)), the check lives at the top of
`analyze_with_tests` ([src/mcp/tools.rs](../src/mcp/tools.rs)):

```rust
let gen_at_start = server.generation.load(Ordering::Acquire);
if let Some(hit) = server.graph_cache.lock().unwrap().get(&key) {
    if hit.gen == gen_at_start {
        return Ok(hit.graph.clone());       // warm: milliseconds
    }
}
// ...full analysis...                       // cold: seconds
```

## Mechanism 3: the watcher as a pure signal

A dedicated thread (`spawn_invalidation_watcher` in
[src/mcp/mod.rs](../src/mcp/mod.rs)) watches the project root
recursively. Its only job is to turn filesystem events into generation
bumps. It never analyzes, never touches the cache maps, never blocks a
tool call. Three filters keep it honest:

- **Debounce (300ms).** Editors and agents write in flurries — a save
  can produce several events. The debouncer coalesces a flurry into one
  batch, so one logical edit is one generation bump, not five.
- **Extension filter.** Only files whose extension maps to a parseable
  `Language` can change the graph, so only those invalidate. Editing
  this markdown file leaves the cache warm. The rule is derived from
  `Language::from_extension` — the same source of truth the analyzer
  uses — so a newly supported language starts invalidating by
  construction, with no second list to maintain.
- **Directory filter.** `target/`, `node_modules/`, `.git/`, `dist/`,
  `build/` are excluded. This matters more than it looks: cargo writes
  *generated `.rs` files* under `target/` during every build. Without
  the filter, `cargo build` would invalidate the cache even though no
  source truth changed.

Failure handling follows "degrade, don't break": if the watcher can't
start (permissions, fd limits), it logs to stderr and the server simply
behaves like the old cold-per-call version. A cache is an optimization;
its absence must never be an error.

## Mechanism 4: the torn-snapshot guard (read the clock twice)

There's a race hiding in lazy caching. Suppose:

```
t0   call arrives, generation = 5
t1   analysis starts reading files...
t2   agent saves a file  →  generation = 6
t3   analysis finishes — but it read SOME files before t2 and some after
```

The graph built at t3 is a *torn snapshot*: it might mix pre-edit and
post-edit file contents. Caching it under generation 6 would serve a
wrong graph until the next edit. The guard is to read the generation
**before starting** and again **after finishing**, and only cache when
they match:

```rust
let gen_at_start = generation.load();      // t0: 5
// ...analysis (seconds)...
if generation.load() == gen_at_start {     // t3: 6 ≠ 5 → don't cache
    cache.insert(key, CachedGraph { graph, gen: gen_at_start });
}
```

The torn result is still *returned* to the caller — it's the best
available answer, and the caller asked mid-edit — but it's never
*remembered*. The next call sees a miss and rebuilds cleanly. This is
the same validate-after pattern used in optimistic concurrency control
(and in seqlocks): do the work without locking the world, then check
whether the world moved, and discard the work's *side effect* if it did.

## Why determinism is the load-bearing prerequisite

None of this is safe unless one property holds: **identical trees
produce identical graphs** (ticket AN-002). Before that fix, two
analyses of the same code could resolve ambiguous names differently
(HashMap iteration order), flip borderline smells, and shift fan-in.
A cache doesn't create staleness bugs — it *amplifies* nondeterminism
bugs, because a cached graph freezes whichever roll of the dice it was
built from, and callers would see it disagree with a fresh run.

With determinism, the cache's worst failure mode collapses to
**stale-old** (you see the tree as of the last generation — bounded by
the debounce window) and never **stale-wrong** (a graph no analysis of
any tree version would produce).

## The second cache: immutable keys need no invalidation at all

`assess_change` compares the working tree against a git ref. The
working-tree side flows through the generational cache above. The base
side gets a separate, simpler cache with a stronger property: the key
is the **resolved commit SHA**, and a SHA *pins its content forever*.
An immutable key means:

- no generation check — the entry can't go stale, ever;
- the expensive setup (materialize a git worktree, analyze it, remove
  it) runs once per SHA, then repeat self-reviews skip it entirely.

This is the "content-addressed cache" end of the spectrum: when the key
cryptographically identifies the value's inputs, invalidation logic
disappears. Measured effect: repeat `assess_change` went from 2.66s to
231ms — the residue being the diff computation itself, which compares
two already-cached graphs.

One subtlety: the cached base graph's entity paths point into a
worktree directory that has since been deleted. That's fine because
`compute_diff` uses the directory only to *strip prefixes* from path
strings — a string operation, not a filesystem access. The cache stores
the directory alongside the graph so the stripping stays consistent.

## Sharing without copying: `Arc` and the lock discipline

Graphs are large. The cache hands them out as `Arc<DependencyGraph>` —
a cache hit is a reference-count bump, not a copy, and an entry evicted
mid-use stays alive until its last user drops it (no use-after-free by
construction, no lifetime gymnastics).

Concurrency is deliberately minimal: the MCP loop is single-threaded
(stdio in, stdio out); the watcher is the only other thread, and the
only state they share is the atomic counter and two `Mutex<HashMap>`s
held for microseconds around lookups/inserts. No lock is ever held
across an analysis. When the cheapest correct tool is a mutex and an
integer, use a mutex and an integer.

## Eviction: crude on purpose

The graph cache holds at most 8 entries, the base cache 4; on overflow
they **clear entirely** rather than LRU-evict. With keys being analysis
roots (usually one) and base SHAs (usually one or two per session), the
caches simply don't see enough distinct keys for eviction policy to
matter. An LRU here would be complexity with no observable benefit —
the bound exists only so a pathological caller can't grow memory
without limit.

## The mechanisms, named

| In this codebase | The general pattern |
| --- | --- |
| Bump counter, rebuild on next call | Lazy / on-demand cache invalidation |
| `generation` + per-entry `gen` | Epoch / version-stamp invalidation |
| 300ms watcher coalescing | Debouncing |
| Check gen before & after building | Optimistic concurrency (validate-after) |
| Returning but not caching a torn result | Best-effort read, guarded side effect |
| SHA-keyed base graphs | Content-addressed / immutable-key caching |
| `Arc<DependencyGraph>` handout | Shared ownership instead of copying |
| Watcher failure → cold calls | Graceful degradation |
| AN-002 as prerequisite | Determinism before caching |

## Current limits, and the next step

Invalidation is *coarse*: one changed file discards whole-tree graphs,
and the next call re-parses everything (~0.6s on this repo, growing
with size). Correct, simple — and the obvious refinement is
**incremental re-analysis**: re-parse only the changed files and patch
the graph. That's a much deeper change (the resolver's name maps and
every metric pass currently assume a full rebuild) and is tracked on
the [roadmap](capabilities-and-roadmap.md) as the remaining step-up.
