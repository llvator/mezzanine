# The altitude descent

> I am new here — what is this system, from the domain down to the function?

This is the one thing Nao can do that nothing else can, because it holds two
altitudes in the same renderer: the domain layer authored in Elevator
(`.elv`) and the code graph derived from source. You arrive at a function
already knowing which capability it serves.

## The descent

### 1. Start at the domain

Open the visualizer on the spec. Elevator entities render differently on
purpose — `isElevatorGraph` is a majority test on the loaded graph, and when
it passes, node size drops to a kind-based radius and the kind palette takes
over. Domain kinds carry no code metrics, so sizing them by "complexity"
would be inventing a number.

What you are reading: Categories as the ground floor, Features as the
capabilities inside them, Functionalities as the verbs on those Features, and
Concepts as the cross-cutting things several Features pull in.

Hover to read descriptions in the **Description** pane (`4`, then `h` to
follow the pointer). Skimming the canvas narrates the system.

### 2. Pick your Category and read its neighbourhood

Click a Category. Its Features close over their transitive `Contains`
descendants and attached Concepts **across `.elv` file boundaries** — a
Category never renders without the Features it declares elsewhere. That is a
deliberate exception to strict file scoping, because a domain grouping split
across files is still one grouping.

The Concepts touching your branch are the important part. A Concept exists
precisely because it spans three or more unrelated points in the Feature
tree, so it is the thing you would never discover by reading down a
hierarchy.

### 3. Cross to the code

Each spec entity carries `cr:` paths — the code that implements it. Re-scope
the graph to those paths and you are now in the code graph, standing exactly
where the capability lives.

### 4. Step down the aggregation levels

With the **View** pane focused (`2`):

| Key | Level | You see |
| --- | --- | --- |
| `m` | Module | Subsystems and the traffic between them |
| `f` | File | Files and their dependencies |
| `e` | Entity | Functions, classes, methods |

Descend one at a time. Jumping straight to entity level on an unfamiliar area
gives you a hairball; arriving there after two intermediate steps gives you a
neighbourhood you already have a mental frame for.

`t` toggles tree/graph mode at any level — tree for hierarchy, force layout
for connectivity.

### 4b. Descend on a relationship, not on the whole view

The level toggle steps the *whole canvas* down, and past a few hundred
entities the render budget will refuse — which is the right refusal, and it
leaves you looking at a file-level edge you cannot open.

The marked set is the targeted version. At file level, ⌘-click (Ctrl-click
elsewhere) both ends of the edge you care about — or `m` over each with the
graph focused (`1`). Each picked node takes a dashed ring; nothing else on
screen changes, so the neighbourhood that made the edge interesting stays
drawn while you build the set.

Then **Drill into N marked ↓** in the toolbar, or `d`. The scope narrows to
just those paths and the level is re-picked against the smaller graph, so a
handful of files opens at entity level on its own. You are now looking at the
functions and classes behind that one dependency, and nothing else.

The button says the entity count before you press it. If the set is too big
to draw as entities it says *opens at file level* rather than quietly handing
back another file view — mark fewer, or drill again from where you land.

Marks are made on paths, not on nodes, so a file marked here stays marked
through a level change; `x` drops them along with the selection.

### 5. Land in the editor

With **Follow selection** on, clicking a node reveals it in the editor —
or use **Go to Source** in the Selection panel. See
[follow-selection.md](follow-selection.md) for making this ambient.

## Why the order is the point

Each step down answers the question the previous step raised, and the
descriptions come from a human who understood the *why*. Reading source
bottom-up gives you mechanism without intent; you learn what a function does
long before you learn why anyone wanted it.

The claim this compresses "the first two weeks of reading around" into hours
rests on the spec existing and being reasonably fresh — see
[spec health](../cli/spec-health.md) for keeping it that way.

## When there is no spec

Skip to step 4 and start at module level. You lose the intent layer, but the
descent through aggregation levels still beats opening files alphabetically.

## Limits

- The domain layer is **authored, not derived**. Descriptions and `cr:` paths
  can lag the code — treat them as strong leads, not ground truth, and verify
  mechanism details against the `cr:` target before relying on them.
- A spec built sketch-first is deliberately uneven: breadth everywhere, depth
  only where work has happened. A one-line Feature is not a gap in the tool,
  it is an area nobody has needed to describe yet.
