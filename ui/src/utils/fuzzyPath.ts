/**
 * Fuzzy matching for paths and entity names — the one matcher the sidebar
 * uses everywhere it asks "does this string match what the user typed".
 *
 * Before this, every filter box in the app was `String.includes()` on a
 * lowercased string with results in whatever order the underlying structure
 * happened to be in. That meant `uicmpsv` found nothing, two words found
 * nothing, "everything except tests" was inexpressible, and when a query did
 * match forty rows the best one was wherever the tree put it.
 *
 * The model is `fzf`'s: whitespace-separated terms, ANDed, each of which can
 * be a fuzzy subsequence or an operator-prefixed literal.
 *
 *   foo     fuzzy subsequence, scored
 *   'foo    literal substring, unscored
 *   !foo    negated — reject the candidate if this matches (literal)
 *   ^foo    anchored to the start
 *   foo$    anchored to the end
 *
 * Case handling is smart-case, as `fzf` and `rg` both do it: a term typed in
 * all-lowercase matches case-insensitively, and a term containing an
 * uppercase character matches case-sensitively. Typing `Graph` when you mean
 * the type and `graph` when you don't care costs nothing to learn and is the
 * behaviour anyone arriving from a terminal already expects.
 *
 * Deliberately regex-free. Regex is a different mental model from fuzzy
 * scoring and the two compose badly in one box; the four operators above
 * cover the cases that actually come up in a path filter.
 *
 * This module is a **leaf**: no store imports, no Svelte. `stores/scope.ts`
 * imports `filterViewModel`, so anything reachable from both trees has to
 * sit below that edge or it closes a cycle — and the module-level
 * `derived(...)` calls make cycle-time evaluation order load-bearing rather
 * than merely untidy.
 */

export interface Term {
  /** The text to match, with operator characters stripped. */
  needle: string;
  /** `'foo` — plain substring rather than a subsequence. Always set for a
   *  negated term; see `buildTerm`. */
  literal: boolean;
  /** `!foo` — the candidate is rejected when this term matches. */
  negated: boolean;
  /** `^foo` */
  anchorStart: boolean;
  /** `foo$` */
  anchorEnd: boolean;
  /**
   * Smart-case: true when the term itself carries an uppercase character.
   *
   * Invariant, relied on by `scoreTerm`: when this is false the `needle` is
   * already lowercase, because that is precisely how `buildTerm` decides it.
   * Build terms through `parseQuery` and it holds for free.
   */
  caseSensitive: boolean;
}

/** Score awarded to a literal or anchored term, which is binary — it either
 *  matches or it doesn't, and there is no "how well" to measure. Sits above
 *  a typical fuzzy score so an exact request outranks an incidental
 *  subsequence when both are in the same query. */
const LITERAL_SCORE = 80;

// Bonus weights. Relative magnitudes matter, absolute ones don't.
const BONUS_CONSECUTIVE = 8;
const BONUS_BOUNDARY = 9;
const BONUS_CAMEL = 7;
const BONUS_FIRST_CHAR = 10;
const BONUS_BASENAME = 3;
const PENALTY_GAP = 1;
/** Caps the gap penalty so one long unmatched stretch can't sink a
 *  candidate below a short one that matches nothing interesting. */
const MAX_GAP_PENALTY = 12;
/** Nudges shorter candidates ahead when everything else ties — `graph.ts`
 *  should beat `graphviz_renderer_helpers.ts` for the query `graph`. */
const LENGTH_TIEBREAK = 0.05;

const SEPARATORS = new Set(['/', '\\', '_', '-', '.', ' ', ':']);

function isBoundary(candidate: string, i: number): boolean {
  if (i === 0) return true;
  return SEPARATORS.has(candidate[i - 1]);
}

/** A lowercase letter followed by an uppercase one — the camelCase hump
 *  that makes `dP` a sensible way to ask for `displayPlan`. */
function isCamelHump(candidate: string, i: number): boolean {
  if (i === 0) return false;
  const prev = candidate[i - 1];
  const cur = candidate[i];
  return prev >= 'a' && prev <= 'z' && cur >= 'A' && cur <= 'Z';
}

/**
 * Tightest subsequence match of `needle` in `haystack`, as match positions.
 *
 * Two passes, which is how `fzf` does it and why: a forward pass finds the
 * earliest position where the needle is complete, and a backward pass from
 * there pulls every match as late as it can go. A single greedy forward pass
 * would match `sv` in `ui/src/components/Sidebar.svelte` at the `s` of `src`
 * and score the result as if the match were scattered, when the tight match
 * in `svelte` is right there.
 *
 * Returns `null` when the needle is not a subsequence at all.
 */
function tightestMatch(haystack: string, needle: string): number[] | null {
  let end = -1;
  let n = 0;
  for (let i = 0; i < haystack.length && n < needle.length; i++) {
    if (haystack[i] === needle[n]) {
      n++;
      end = i;
    }
  }
  if (n < needle.length) return null;

  const positions: number[] = new Array(needle.length);
  let k = needle.length - 1;
  for (let i = end; i >= 0 && k >= 0; i--) {
    if (haystack[i] === needle[k]) {
      positions[k] = i;
      k--;
    }
  }
  return positions;
}

/** Score a set of match positions. Higher is better. */
function scorePositions(original: string, positions: number[]): number {
  const lastSlash = original.lastIndexOf('/');
  let score = 0;
  let gapPenalty = 0;

  for (let k = 0; k < positions.length; k++) {
    const i = positions[k];
    score += 1;
    if (i === 0) score += BONUS_FIRST_CHAR;
    else if (isBoundary(original, i)) score += BONUS_BOUNDARY;
    else if (isCamelHump(original, i)) score += BONUS_CAMEL;
    if (k > 0) {
      const gap = i - positions[k - 1] - 1;
      if (gap === 0) score += BONUS_CONSECUTIVE;
      else gapPenalty += Math.min(gap * PENALTY_GAP, MAX_GAP_PENALTY);
    }
    if (i > lastSlash) score += BONUS_BASENAME;
  }

  return score - gapPenalty - original.length * LENGTH_TIEBREAK;
}

/**
 * Score one term against one candidate. `null` means "does not match".
 *
 * `lowered` lets a caller that scores a fixed corpus hand in the folded form
 * it already holds. Both halves of the scope search do: a repo's paths and
 * entity names don't change between keystrokes, so folding them per query —
 * per *term*, in fact — was allocating a throwaway string for every candidate
 * in the repo on each character typed.
 *
 * Positions are still scored against `candidate`, never the folded form:
 * `scorePositions` reads boundaries and camelCase humps out of it, and both
 * are gone from a lowercased string.
 */
function scoreTerm(term: Term, candidate: string, lowered?: string): number | null {
  const hay = term.caseSensitive ? candidate : (lowered ?? candidate.toLowerCase());
  // No fold needed on the needle: a term is case-insensitive exactly when it
  // has no uppercase in it, so it is already its own lowercase form.
  const needle = term.needle;

  if (term.anchorStart && term.anchorEnd) return hay === needle ? LITERAL_SCORE : null;
  if (term.anchorStart) return hay.startsWith(needle) ? LITERAL_SCORE : null;
  if (term.anchorEnd) return hay.endsWith(needle) ? LITERAL_SCORE : null;
  if (term.literal) return hay.includes(needle) ? LITERAL_SCORE : null;

  const positions = tightestMatch(hay, needle);
  if (!positions) return null;
  return scorePositions(candidate, positions);
}

function buildTerm(raw: string): Term | null {
  let s = raw;
  let negated = false;
  let literal = false;
  let anchorStart = false;
  let anchorEnd = false;

  if (s.startsWith('!')) { negated = true; s = s.slice(1); }
  if (s.startsWith("'")) { literal = true; s = s.slice(1); }
  // Negation is literal by default, as it is in fzf, and for the same
  // reason: as a *subsequence*, `test` occurs in nearly every path with a
  // `t`, an `e` and an `s` in it — `ui/src/componen[t]s/Scop[e]Tre[e].[s]vel[t]e`
  // is a match. Fuzzy negation therefore rejects almost the entire corpus,
  // which is never what someone typing `!test` wants. `!'x` and `!x` mean
  // the same thing; the quote is allowed but redundant.
  if (negated) literal = true;
  if (s.startsWith('^')) { anchorStart = true; s = s.slice(1); }
  if (s.length > 1 && s.endsWith('$')) { anchorEnd = true; s = s.slice(0, -1); }
  if (!s) return null;

  return {
    needle: s,
    literal,
    negated,
    anchorStart,
    anchorEnd,
    caseSensitive: s !== s.toLowerCase(),
  };
}

// One-entry memo. `parseQuery` is called from several derived stores that
// all recompute on the same keystroke, and the parse is pure — caching the
// last result keeps every consumer of a given query on one allocation
// without any of them having to coordinate.
let cachedRaw: string | null = null;
let cachedTerms: Term[] = [];

/** Parse a raw query into terms. Empty input yields an empty list, which
 *  every caller reads as "no query", not "matches nothing". */
export function parseQuery(raw: string): Term[] {
  if (raw === cachedRaw) return cachedTerms;
  const terms: Term[] = [];
  for (const piece of raw.trim().split(/\s+/)) {
    if (!piece) continue;
    const term = buildTerm(piece);
    if (term) terms.push(term);
  }
  cachedRaw = raw;
  cachedTerms = terms;
  return terms;
}

/**
 * Score a candidate against every term. All non-negated terms must match;
 * any negated term that matches rejects the candidate outright.
 *
 * Returns `null` for no match, so `0` stays a legitimate score.
 *
 * `lowered` is `candidate.toLowerCase()`, when the caller keeps a folded copy
 * of its corpus — see `scoreTerm`. Omitting it costs a fold per term and is
 * the right call for one-off comparisons.
 */
export function scoreQuery(terms: Term[], candidate: string, lowered?: string): number | null {
  if (terms.length === 0) return null;
  let total = 0;
  for (const term of terms) {
    const score = scoreTerm(term, candidate, lowered);
    if (term.negated) {
      if (score !== null) return null;
      continue;
    }
    if (score === null) return null;
    total += score;
  }
  return total;
}

/** Boolean form, for callers that filter without ranking. */
export function matchesQuery(terms: Term[], candidate: string): boolean {
  return scoreQuery(terms, candidate) !== null;
}

/**
 * True when any negated term matches `candidate`.
 *
 * For callers that score one string but commit another — the scope query
 * ranks an entity on its *name* and then scopes to the *file* declaring it.
 * Checking only the name let `^src !parser` return `src/parser/…`, because
 * an entity happened to be called `src` and that name contains no "parser".
 * Negation is exclusion, and exclusion has to be about what ends up in
 * scope, not about the string that got it there.
 */
export function violatesNegation(terms: Term[], candidate: string): boolean {
  for (const term of terms) {
    if (!term.negated) continue;
    if (scoreTerm(term, candidate) !== null) return true;
  }
  return false;
}

/** True when the query consists only of negated terms (`!test`), which every
 *  candidate that avoids them satisfies. Callers that would otherwise show
 *  the entire corpus can use this to decide whether that's what the user
 *  asked for. */
export function isPurelyNegative(terms: Term[]): boolean {
  return terms.length > 0 && terms.every((t) => t.negated);
}
