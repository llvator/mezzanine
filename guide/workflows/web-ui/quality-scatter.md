# Quality scatter

> What *kind* of trouble is this code in, and is it local or spread?

A ranked table tells you `activate` is bad. The scatter tells you what kind
of bad — and then hands you off to the canvas, which tells you whether the
mess is contained or spread across the system. That handoff is the workflow.

## Getting there

Sidebar (`0`), Quality tab (`q`). Two scatter plots:

| Plot | Axes | Reads as |
| --- | --- | --- |
| Coupling | fan-in × fan-out | How this entity sits in the dependency web |
| Size vs complexity | loc × cyclomatic | Whether length or branching is the problem |

Points are coloured by the same severity encoding the canvas uses — one
composite score, consistent everywhere — and points sitting in a dependency
cycle are marked. Every point has a `<title>` naming the entity and its
values, and every point is clickable.

## Reading the coupling quadrants

This is the plot that pays, because the quadrants mean genuinely different
things and call for different responses:

| Quadrant | Shape | What it is | Response |
| --- | --- | --- | --- |
| High in, high out | Hub / dispatcher | Everything routes through it | The highest-value refactor and the riskiest — check `impact` first |
| High in, low out | A contract | Many depend on it, it depends on little | Often *correct*. Change carefully, it is load-bearing |
| Low in, high out | Orchestrator | Pulls a lot together, few callers | Fine if it is an entry point; a smell if it is buried |
| Low in, low out | Leaf | Isolated | Ignore, or check whether it is dead |
| The outlier corner | — | Your refactor candidate | Start here |

The mistake to avoid is treating "high fan-in" as bad. A widely used, stable
contract is what good factoring *looks like*. What deserves attention is high
fan-in combined with high fan-out — that entity is a junction, and junctions
are where change amplifies.

## The click-through

Click a point. It selects the node on the canvas, and now you can see the
thing the scatter could not tell you: **is this mess local or spread?**

- A high-pressure node whose neighbours are all in its own folder is a
  contained problem. Refactor it in place; the blast radius is a directory.
- A high-pressure node with edges fanning across four subsystems is an
  architectural problem wearing a function's clothes. Refactoring the
  function will not fix it.

That distinction changes what you do next, and it is invisible in any ranked
list. It is the reason to use the canvas here at all rather than reading
`quality` output in a terminal.

## Cycles

Points marked as sitting in a dependency cycle deserve a look even at modest
complexity. A cycle means the two entities cannot be understood, tested, or
extracted independently — a structural cost that neither complexity number
captures. `nao cycles <path>` prints the chains if you want them as text.

## The severity encoding

Fill colour on both the scatter and the canvas comes from `composite_score`,
the same number the Quality list ranks by. One source, three render sites —
the canvas has two node-join sites and the legend a third, and each carrying
its own copy is how a theme bug shipped once already.

Check the **Legend** in the Filter panel: it describes the *active* encoding
rather than a fixed palette, and it owns the size-channel and colour-channel
selectors. If you have changed what size means, the legend changed with it.

## Limits

- Thresholds behind the severity colours are absolute constants, not
  per-repo percentiles. Calibrate your own reading of what "red" means for
  this codebase.
- The scatter plots the entities in the **current scope**. Narrow the scope
  and the axes rescale — two sessions are not comparable unless the scope
  matches.
- Points overlap. A dense cluster near the origin is many entities, not one;
  the scatter is for finding outliers, not for counting.
