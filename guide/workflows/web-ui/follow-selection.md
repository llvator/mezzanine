# Follow selection

> What is around the code I am editing, without going to look?

Every other workflow here is something you *go and do*. This one is ambient:
the graph re-centres as your cursor moves, and you catch structural surprises
while doing something else entirely.

Of everything in this folder, it has the best effort-to-payoff ratio, because
after the initial setup it costs nothing per use.

## Setup

In VS Code:

1. Open the Mezzanine panel — Command Palette → **Mezzanine: Open Code Visualizer**.
2. **View Options** → turn on **Follow selection to editor**.
3. Settings → enable `mezz.autoVisualize`.

The sync is bidirectional. Moving the cursor re-centres the graph; clicking a
node reveals the source. `mezz watch` re-analyzes on save, so the graph tracks
the code you are actually writing rather than the code you opened with.

For a compact single-file view, **Mezzanine: Visualize Current File** focuses on the
entity at the cursor rather than the whole project.

## Why ambient beats intentional

A graph you have to decide to open is a graph you consult when you already
suspect something. That is exactly the case where you least need it — you
have a hypothesis, so `impact` or `trace` will answer faster and more
precisely.

The value of the always-on version is the opposite case: the surprises you
had no reason to go looking for.

- "Why does this file reach into `server/`?"
- "This function has eleven callers — I assumed two."
- "That helper I am about to modify sits in a cycle."

None of those are questions you would have thought to ask. They arrive
because the shape was in your peripheral vision while you were doing
something else.

## Using it without it being a distraction

Three keys make the difference between ambient and annoying:

| Key | Pane | Effect |
| --- | --- | --- |
| `l` | global | **Freeze hover** — stops the panel chasing the pointer so you can read what is under it |
| `p` | graph | **Pin** the hovered node — the graph stops following the cursor and stays where you put it |
| `x` | graph | Clear the selection and resume following |

The pattern that works: leave it following, and the moment something looks
odd, `p` to pin and investigate. Without pinning, the thing you noticed
disappears the instant you move the mouse toward it.

`o` (View pane) cycles hover mode, and `d` sets highlight depth — how many
hops out from the hovered node get emphasised. Depth 1 keeps it calm; higher
depths are for the moment you have already spotted something.

## Pairing it with the Description pane

Focus the Description pane (`4`) and turn on **follow hover** (`h`). Now
hovering narrates: the node's description followed by its ancestors', so you
get intent alongside structure. Clicking a rung of the ancestor chain pins
that ancestor and re-roots the chain there — often the first place a parent
entity becomes visible at all.

This pane carries no metrics, no source, no fields. That restraint is the
point: it is for reading, and a second details panel would just be noise.

## When to turn it off

- **Deep single-file work** where the surrounding structure is already known
  and re-analysis on every save is wasted motion.
- **Presenting or pairing**, where a moving graph pulls attention off the
  code being discussed.

## Limits

- Re-analysis happens on save. Between saves the graph describes the last
  saved state, not the buffer.
- On a large repo the first analysis after opening the panel is the slow one;
  after that the warm cache and the content-hashed parse store make saves
  cheap — one edited file re-parses exactly that file.
