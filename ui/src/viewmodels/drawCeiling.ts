/**
 * drawCeiling — the canvas render gate, measured on what is drawn.
 *
 * The gate this replaces (UI-061) read `scopeCounts(rules, index)`: the
 * entity total for the selected paths, evaluated in `applySelection` and
 * consumed by `displayPlan` *before* it called `compute()`. Every filter the
 * user can reach — entity types, relationship types, languages, hidden
 * files, ghosts, level overrides, diff, search — is applied inside
 * `compute()`, so no filter could ever affect that decision. Narrowing the
 * view left the "Scope too large to render" overlay exactly where it was,
 * and the only lever left was to shrink the analysis scope, which also
 * shrank Quality, Summary, Diff and Context. A visualization limit was
 * forcing an analysis compromise.
 *
 * So the decision moved downstream of `compute()`, onto the one number that
 * actually predicts render cost: how many nodes reach the DOM.
 *
 * Kept in its own module, with only type imports, for two reasons: the
 * decision is pure and worth testing without a browser (see
 * `scripts/draw-ceiling.test.ts`), and `displayPlan.ts` is already the
 * largest viewmodel in the tree.
 */

import type { DisplayPlan } from './displayPlan';

/**
 * Most nodes the canvas will draw before it declines.
 *
 * 2,000 deliberately restates the number the app already tolerated, now
 * measured on the right quantity. Before UI-061 a scope could be pinned to
 * entity level with `autoLevel` off and draw every one of up to 2,000
 * entities — unguarded, because the old gate stopped at the same figure on
 * the *scope* count and nothing checked afterwards. So 2,000 drawn nodes is
 * not a guess: it is the load this canvas has been shipping.
 *
 * Distinct from `RENDER_BUDGET` (400) in `stores/scope.ts`, which is a
 * target `pickLevel` collapses *toward* by choosing entity / file / module
 * aggregation. A ceiling set at 400 would fire on any view that lands
 * slightly above the target — a level pinned by hand, ghosts switched on,
 * or the parameter and class-field nodes `compute()` injects after
 * collapse. The 5× gap is that headroom.
 *
 * Whether one number can serve both the drawing and the plan computation it
 * feeds is UI-063's question, not this module's.
 */
export const DRAW_CEILING = 2000;

/** Why the canvas declined, and what it would take to change its mind.
 *  `drawn` is live: it moves as filters change, which is what makes the
 *  overlay's advice checkable rather than decorative. */
export interface DrawOverflow {
  /** Nodes the plan wanted on screen. */
  drawn: number;
  /** The ceiling `drawn` exceeded. */
  ceiling: number;
}

/**
 * Gate a computed plan on its own drawn-node count.
 *
 * Under the ceiling the plan passes through untouched. Over it, everything
 * the View draws from is emptied and `overflow` records the two numbers —
 * so the canvas goes quiet exactly as it did before, but now every filter
 * is upstream of the decision and can therefore lift it.
 *
 * Note what this does *not* do: `graphData` still reaches `GraphView`, which
 * builds its d3 simulation and SVG from that store rather than from the
 * plan. Gating the DOM build is a separate defect with its own ticket; the
 * old gate did not do it either.
 */
/**
 * Every node id the canvas will put in the DOM for this plan.
 *
 * Not just `visibleNodeIds`. A dimmed node is drawn — faintly, as the diff
 * and search context the dimming exists to provide — so it costs a DOM
 * element and a simulation body like any other. Counting only the visible
 * set let a diff-filtered view report 50 while drawing ten thousand, which
 * is the same mistake as the gate this module replaced: measuring one
 * quantity and charging another.
 *
 * The exception is `dimOpacity === 0`, where `applyDisplayPlan` hides the
 * dimmed set outright rather than fading it. Then it genuinely is not drawn.
 *
 * One definition, used by both the gate and the View's build set, so the
 * number the ceiling is compared against is the number that gets built.
 */
export function drawnIdsOf(plan: DisplayPlan): Set<string> {
  const ids = new Set(plan.visibleNodeIds);
  if (plan.dimOpacity > 0) {
    for (const id of plan.dimmedNodeIds) ids.add(id);
  }
  return ids;
}

export function gateByDrawCeiling(
  plan: DisplayPlan,
  ceiling: number = DRAW_CEILING,
): DisplayPlan {
  const drawn = drawnIdsOf(plan).size;
  if (drawn <= ceiling) return plan;
  return {
    ...plan,
    visibleNodeIds: new Set(),
    visibleLinkKeys: new Set(),
    dimmedNodeIds: new Set(),
    contextNodeIds: new Set(),
    treePositions: new Map(),
    nodeDistances: null,
    selectedId: null,
    dimOpacity: 0,
    contextOpacity: 1,
    overflow: { drawn, ceiling },
  };
}
