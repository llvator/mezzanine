/**
 * What a region's relationships do — the second half of UI-071.
 *
 * The outline says where a folder is and the card says what it is called.
 * Neither answers the question worth asking about a group: **is this a
 * subsystem, or a directory someone filed things in?** The graph knows.
 * Relationships that stay inside the boundary against relationships that
 * cross it is exactly that distinction, and it is already derivable from the
 * link set the canvas has drawn.
 *
 * ## This is not the Quality panel's cohesion, and does not pretend to be
 *
 * `ScopeMetrics.cohesion` is a repo-level property: every dependency edge,
 * unfiltered, unaggregated, whatever the reader happens to be looking at. It
 * belongs where it already is, in the Quality table, as a percentage against
 * a threshold.
 *
 * These counts describe **the picture on screen** — after the scope, the
 * filters and the aggregation level. Two numbers under one name would be
 * worse than one, so this deals in plain counts, never a ratio and never the
 * word: what it reports, the reader can verify by looking at the canvas,
 * which is the whole reason for putting it there.
 *
 * ## Nesting makes crossing relative, on purpose
 *
 * A link from `ui/src/stores` to `ui/src/viewmodels` **crosses** the `stores`
 * boundary and stays **inside** `ui/src`. Both are true, and the pair is the
 * useful part: it says at which level the coupling is contained. The card
 * shows the trail, so a reader who wants the parent's answer moves the
 * pointer to a part of the parent no child covers — the same affordance
 * UI-089 gives the focus gesture.
 *
 * Pure and store-free (`npm run test:traffic`), so the counting rules can be
 * argued with in a test rather than through a browser.
 */

/** One region's account of itself, in the units the canvas is drawing. */
export interface RegionTraffic {
  /** Members currently on the canvas. */
  drawn: number;
  /** Members the current scope holds at this level, drawn or not. The
   *  comparison a sparse region needs: without it a folder half-hidden by a
   *  filter is indistinguishable from a folder that is genuinely small. */
  total: number;
  /** Drawn relationships with both ends inside this region. */
  inside: number;
  /** Drawn relationships with exactly one end inside it. */
  crossing: number;
}

export function emptyTraffic(): RegionTraffic {
  return { drawn: 0, total: 0, inside: 0, crossing: 0 };
}

/** A drawn relationship, reduced to the only thing this needs from it. */
export interface TrafficLink {
  source: string;
  target: string;
}

export interface TrafficInput {
  /**
   * Every node the current scope holds at the level being drawn — *not* only
   * the visible ones. Same collection the drawn set is a subset of, so
   * `drawn` and `total` are in the same units and the comparison means
   * something. Comparing against the index's entity totals would put a file
   * count over an entity count, which is the mistake `StatsBar` documents.
   */
  candidates: readonly { id: string }[];
  /** Which of them reached the canvas. */
  isDrawn: (id: string) => boolean;
  /** The relationships actually drawn. */
  links: readonly TrafficLink[];
  /**
   * A node's regions, innermost first — the same ancestor chain the hulls
   * were built from, so membership here and the outline on screen cannot
   * disagree. A node in no region (a ghost, an external symbol) returns `[]`.
   */
  keysOf: (id: string) => readonly string[];
}

/**
 * Count what each region holds and what its relationships do.
 *
 * One pass over the nodes and one over the links, so this is affordable
 * wherever the drawn set is rebuilt — and it must not be recomputed on a
 * simulation tick, because none of it depends on where anything landed.
 *
 * **A link with an end in no region at all is skipped entirely.** Crossing is
 * meant to say "this goes to another part of the repo", and a call into the
 * standard library says nothing about whether the folder is a subsystem. If
 * ghosts counted, every folder that imports anything would look leaky, and
 * the number would stop separating the case it exists to separate.
 */
export function regionTraffic(input: TrafficInput): Map<string, RegionTraffic> {
  const { candidates, isDrawn, links, keysOf } = input;
  const out = new Map<string, RegionTraffic>();
  const of = (path: string): RegionTraffic => {
    let t = out.get(path);
    if (!t) { t = emptyTraffic(); out.set(path, t); }
    return t;
  };

  for (const node of candidates) {
    const drawn = isDrawn(node.id);
    for (const path of keysOf(node.id)) {
      const t = of(path);
      t.total++;
      if (drawn) t.drawn++;
    }
  }

  for (const link of links) {
    const from = keysOf(link.source);
    const to = keysOf(link.target);
    if (from.length === 0 || to.length === 0) continue;
    const inTo = new Set(to);
    const inFrom = new Set(from);
    // The union, because a region is entitled to an opinion about a link with
    // either end in it. Counting only the source's regions would leave a
    // folder blind to everything pointing at it.
    for (const path of new Set([...from, ...to])) {
      const t = of(path);
      if (inFrom.has(path) && inTo.has(path)) t.inside++;
      else t.crossing++;
    }
  }

  return out;
}

/**
 * The sentence the card puts under the trail.
 *
 * Counts and a name, never a percentage: a ratio here would invite comparison
 * with the Quality panel's cohesion, which is measured over a different
 * universe and would legitimately disagree.
 */
export function trafficSentence(label: string, t: RegionTraffic | undefined): string {
  if (!t) return '';
  const total = t.inside + t.crossing;
  if (total === 0) return `No relationships drawn inside or out of ${label}`;
  if (t.crossing === 0) return `All ${total} relationships here stay inside ${label}`;
  if (t.inside === 0) return `All ${total} relationships here cross out of ${label}`;
  return `${t.inside} of ${total} relationships stay inside ${label}`;
}

/**
 * How a row states its membership.
 *
 * Silent when nothing is hidden — `19` rather than `19 of 19`. The comparison
 * is worth the reader's attention only when the two differ, and a ratio on
 * every row of a three-deep trail is noise that hides the one row where it
 * matters.
 */
export function membershipText(t: RegionTraffic | undefined, fallback: number): string {
  if (!t) return String(fallback);
  return t.drawn === t.total ? String(t.total) : `${t.drawn} of ${t.total}`;
}

/**
 * Why a row's membership reads the way it does.
 *
 * The noun is the caller's, because a region is a folder or a file depending
 * on the grain (UI-103) and a tooltip that says "folder" over a file's row is
 * a claim about the tree that the tree does not make. Defaulted rather than
 * required: every existing caller means a folder, and the parameter exists so
 * the file-grain caller cannot silently inherit the wrong word.
 */
export function membershipTitle(t: RegionTraffic | undefined, noun = 'folder'): string {
  if (!t) return `Nodes from this ${noun} currently drawn`;
  if (t.drawn === t.total) return `All ${t.total} of this ${noun}'s nodes in scope are drawn`;
  const hidden = t.total - t.drawn;
  return `${t.drawn} drawn, ${hidden} hidden by a filter — this region is filtered, not sparse`;
}
