/**
 * Pick the best few matches without sorting the rest.
 *
 * Search boxes here rank a whole corpus and then draw a couple of hundred
 * rows. Sorting the full match set to find them is work proportional to what
 * was thrown away, and the queries with the most matches — the one-letter
 * prefixes typed on the way to a real query — are exactly the ones a full
 * sort punishes hardest.
 *
 * A leaf module: no stores, no Svelte, nothing at runtime, so the node test
 * runner can load it directly.
 */

/** The two fields ranking reads. `path` is the tiebreak, so an unstable
 *  corpus iteration order can't reshuffle equally-scored rows. */
export interface Ranked {
  score: number;
  path: string;
}

/** Does `a` sort ahead of `b`? Strongest score first, ties by path. */
export function ranksBefore(a: Ranked, b: Ranked): boolean {
  if (a.score !== b.score) return a.score > b.score;
  return a.path.localeCompare(b.path) < 0;
}

/**
 * The best `k`, in rank order.
 *
 * A bounded insertion sort: each candidate is compared against the weakest
 * survivor first and discarded outright when it can't beat it, which after
 * the opening `k` is nearly all of them. Only a candidate good enough to make
 * the list pays for the binary search and the shift.
 *
 * Ties are resolved the same way a full sort would resolve them, so the
 * result is identical to `[...matches].sort(...).slice(0, k)` — this is only
 * a cheaper route to it, never a different answer.
 */
export function bestMatches<T extends Ranked>(matches: T[], k: number): T[] {
  if (k <= 0) return [];

  const out: T[] = [];
  for (const m of matches) {
    if (out.length === k && !ranksBefore(m, out[k - 1])) continue;

    let lo = 0;
    let hi = out.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (ranksBefore(m, out[mid])) hi = mid;
      else lo = mid + 1;
    }

    out.splice(lo, 0, m);
    if (out.length > k) out.pop();
  }
  return out;
}
