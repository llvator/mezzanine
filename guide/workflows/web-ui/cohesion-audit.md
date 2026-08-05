# Cohesion audit

> Are these folders real boundaries, or just names?

A directory tree asserts that things inside it belong together. Nothing
checks that assertion. This workflow makes it visible in about two seconds,
because the force layout pulls each node toward its folder tree — so a folder
that means something settles into a region, and one that does not scatters.

## The reading

Open the canvas at file or entity level, scope to a subtree, and let the
simulation settle.

| What you see | What it means |
| --- | --- |
| A folder's nodes settle into one region | The folder is a real boundary. Its members talk to each other more than to outsiders |
| A folder's nodes scatter across the canvas | **The folder is a name, not a module.** Its members' real relationships are elsewhere |
| Two sibling folders converge into one blob | They may want to be one module — or the boundary between them is in the wrong place |
| One node marooned far from its folder | The clearest single finding here: that file is misfiled, and the graph is telling you where it actually belongs |

The scattered case is the valuable one, and it is genuinely hard to reach any
other way. You would need to define a cohesion metric, implement it, decide a
threshold, and generate a report — and you would still be looking at a number
rather than at *which* members went where.

## Why the pull is ancestor-aware

A node's pull is distributed over its **whole ancestor chain**, not just its
immediate parent. This matters more than it sounds.

With a flat grouping — immediate parent directory only — `ui/src/stores` and
`ui/src/components` are exactly as unrelated to the simulation as
`ui/src/stores` and `docs/adr`. The hierarchy exists in the system but not in
the layout, so the force washes out over the settle whatever the initial
seeding put there. Siblings converge on the parent they share only if the
simulation knows they *have* a shared parent.

Two design details keep the reading honest:

- **Tiers are priced by what they exclude.** A tier holding every node in the
  drawn set contributes exactly zero, so a common ancestor cannot act as a
  centering force that quietly loosens the leaf folders whose pull it took.
  The common-ancestor case falls out of the arithmetic rather than being
  special-cased.
- **Weights are normalised per node**, so the off/low/med/high ladder means
  the same thing however many tiers a node happens to have. A file five
  levels deep is not pulled harder than one at two levels.

The node's own folder is exempt from tier scaling: a scope narrowed to a
single folder has to keep cohering the way it did before nesting existed.

## Calibrating what you see

Step the cohesion control through its levels and watch what moves:

- Structure that holds at **every** level is real.
- Structure that only appears at **high** is the force asserting the folder
  tree, not the code agreeing with it.

That is the control question, and it is worth asking every time — otherwise
you are reading the layout's premise back to yourself.

Be aware of one confound: raising cohesion also contracts each leaf group
into a ball whose members cannot approach past the collision radius. So
comparing "off" against "high" conflates grouping strength with leaf
contraction. Compare adjacent levels instead.

## What to do with a finding

A scattered folder is a prompt, not a verdict. The useful follow-ups:

- Open [scope decoupling](scope-decoupling.md) and look at where its members'
  edges actually go.
- If the members cluster around a *different* folder, that is a move, and
  `impact` on each will tell you the cost.
- If they cluster around nothing, the folder may be a grab-bag — the `utils`
  problem — and the fix is naming the real groupings, not moving files.

## Limits

- **This is a layout, not a metric.** It is evidence for a conversation, not
  a number for a dashboard. Two settles differ.
- Cohesion competes with every other force in the simulation. A node with
  very high fan-out will sit near its callers regardless of where it lives,
  and that is correct behaviour, not a bug in the reading.
- The visual grouping tiers (hulls, region labels, per-region colour) are in
  active development — see `UI-070` through `UI-073`. The force itself is
  shipped and is what this workflow rests on.
