# Scope decoupling

> What does this subsystem depend on, with fan-in still telling the truth?

Nao has two scope controls, and the difference between them is the most
commonly misused thing in the UI:

| Panel | Controls | Effect |
| --- | --- | --- |
| **Analysis Scope** | What gets **analyzed** | Determines the graph, and therefore every metric |
| **Visual Scopes** | What gets **drawn** | Filters the canvas only |

Narrowing both is the intuitive move and the wrong one.

## Why narrowing analysis lies to you

Fan-in is "how many things depend on this." A graph built from one
subdirectory **cannot see the callers outside it**. Every entity in that
folder then appears to have few or no dependents — so the folder's entire
surface looks either dead or safe to change.

The numbers are not merely incomplete; they are confidently wrong, in the
direction that makes you brave.

## The correct setup

**Analysis Scope: wide** (the whole project, or at least everything that
could plausibly call in). **Visual Scopes: narrow** (the subsystem you care
about).

Now:

- Fan-in and fan-out are computed against the whole truth.
- The canvas shows only your subsystem.
- Edges leaving the drawn set are **visible as edges leaving**, rather than
  silently absent.

That last property is the whole point. You are looking at your module's real
contract with the rest of the system — every dependency it has and every
dependency on it — without the other 90% of the repo drawn on top.

## What to look for

- **Edges leaving in an unexpected direction.** A parser reaching into the
  server layer is an architectural violation you can see before you can name.
- **A dense inbound bundle onto one entity.** That is your module's real
  public API, whatever its declared visibility says.
- **Edges you cannot explain.** Select the node and use `impact` for the
  precise answer — the canvas found it, the text tool confirms it.
- **A module with almost no crossings.** Genuinely independent, and therefore
  genuinely extractable. Rare and worth knowing about.

## The same principle in the text tools

`dead_code` applies this rule internally: `path` narrows the *report* while
fan-in is always computed over the whole project, precisely because the
subdirectory graph would report that folder's entire surface as dead. The
MCP tools bake the correct behaviour in; the UI gives you the two dials and
trusts you.

## Elevator entities behave differently on purpose

Under visual scoping, Elevator entities close over their transitive
`Contains` descendants and attached Concepts **across `.elv` file
boundaries** — a Category never renders without the Features it declares
elsewhere. Code entities keep strict file scoping.

The asymmetry is deliberate: a domain grouping split across spec files is
still one grouping, and rendering half of it would misrepresent the domain.
Descendants only, so narrowing to one branch still narrows.

## Cheap version

**Nao: Visualize Current File** gives you a compact view focused on the
entity at your cursor, with the project-wide analysis intact behind it. When
the question is just "what does this file touch," that is one command instead
of two panels.

## Limits

- Wide analysis costs more on the first run. The warm cache and the
  content-hashed parse store make subsequent runs cheap, so pay it once.
- "Edges leaving the drawn set" still depends on those edges having been
  resolved — call resolution runs at 93.7% recall on this repo, so a missing
  crossing is possible. Absence is evidence, not proof.
