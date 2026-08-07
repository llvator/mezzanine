/**
 * UI-091 — telling an unmeasured scope apart from a decoupled one.
 *
 * Scope fan-in and fan-out are counted over *dependency* edges. A document
 * graph has none: a Markdown link, an Elevator or Impex reference and a
 * folded SQL foreign key are all `References`, which
 * `RelationshipKind::is_dependency` excludes. So the folder the canvas has
 * just drawn three arrows out of reports a fan-out of `0`.
 *
 * Zero is the flattering end of that scale. Left alone, a scope nobody
 * measured reads as a scope with nothing to answer for — the one reading a
 * reader is least likely to double-check. The backend now tallies reference
 * edges alongside (never inside) the dependency counts; this turns that pair
 * of numbers into what a cell should say.
 *
 * Pure and store-free, so both the detail panel and the Quality table can ask
 * the same question and get the same answer.
 */

/** What one fan-in / fan-out cell should render. */
export interface CouplingCell {
  /** The number, or an em dash when nothing was measured. */
  text: string;
  /** True when the `0` would have been a claim the graph cannot support. */
  unmeasured: boolean;
  /** Hover text. Empty when the number speaks for itself. */
  tip: string;
}

/** Which side of the scope the references sit on. */
export type Direction = 'in' | 'out';

/**
 * Decide how to render one coupling count.
 *
 * `fan` is the dependency-edge count as measured; `refs` is the number of
 * distinct scopes on the same side reached by reference edges instead.
 *
 * A scope only reads as unmeasured when it has *no* dependency edges in that
 * direction and *does* have references there. A scope with real coupling
 * shows its real number even if it also has references — the count is then a
 * measurement, and annotating it would be noise. A scope with neither shows
 * `0`, which is a genuine, earned zero and must stay distinguishable.
 */
export function couplingCell(fan: number, refs: number, dir: Direction): CouplingCell {
  if (fan > 0 || refs <= 0) {
    return { text: String(fan), unmeasured: false, tip: '' };
  }
  const plural = refs === 1 ? 'link' : 'links';
  const side = dir === 'in' ? 'pointing at this scope' : 'leaving this scope';
  return {
    text: '—',
    unmeasured: true,
    tip:
      `Not measured — not zero. Coupling counts dependency edges (imports, calls, ` +
      `type uses). This scope has ${refs} reference ${plural} ${side} — links between ` +
      `documents or specs — which are reported but never counted as coupling.`,
  };
}
