/**
 * Minimal line-level diff utility.
 * Uses a simple LCS (Longest Common Subsequence) approach to produce
 * side-by-side diff lines with change markers.
 */

export type DiffLineKind = 'equal' | 'added' | 'removed';

export interface DiffLine {
  kind: DiffLineKind;
  /** Line content (without newline) */
  text: string;
  /** 1-based line number in the original (base) text, or null if added */
  baseLine: number | null;
  /** 1-based line number in the new (head) text, or null if removed */
  headLine: number | null;
}

/**
 * The largest LCS table this will build, in cells.
 *
 * The table is quadratic in both inputs, so a whole *file* — which the details
 * pane now diffs when the reader clicks a file node — can ask for a matrix
 * that costs more to allocate than the answer is worth. 4M cells is roughly
 * two 2,000-line sides, which covers real source files; the trimming below
 * means only genuinely dissimilar text ever gets near it.
 */
export const LCS_BUDGET = 4_000_000;

/** Whether these two texts will be aligned line-by-line, or shown as one
 *  replaced block. Callers render a note when it's the latter — a cap that
 *  silently changes what the diff means would read as a wrong diff. */
export function exceedsLcsBudget(base: string, head: string): boolean {
  const b = base.split('\n');
  const h = head.split('\n');
  const [pre, suf] = commonEnds(b, h);
  return (b.length - pre - suf) * (h.length - pre - suf) > LCS_BUDGET;
}

/** How many lines the two sides share at the start and at the end. Almost
 *  every real edit is a small middle between two long identical ends, so
 *  trimming them is what keeps the quadratic part small. */
function commonEnds(b: string[], h: string[]): [number, number] {
  let pre = 0;
  while (pre < b.length && pre < h.length && b[pre] === h[pre]) pre++;
  let suf = 0;
  while (suf < b.length - pre && suf < h.length - pre && b[b.length - 1 - suf] === h[h.length - 1 - suf]) {
    suf++;
  }
  return [pre, suf];
}

/**
 * Compute a line-level diff between two strings.
 * Returns an array of DiffLine entries suitable for rendering.
 */
export function computeLineDiff(base: string, head: string): DiffLine[] {
  const allBase = base.split('\n');
  const allHead = head.split('\n');
  const [pre, suf] = commonEnds(allBase, allHead);
  const baseLines = allBase.slice(pre, allBase.length - suf);
  const headLines = allHead.slice(pre, allHead.length - suf);

  // Over budget, the middle is reported as one block replaced by another.
  // That is what a diff degrades to rather than a different kind of answer,
  // and `exceedsLcsBudget` lets the caller say so.
  const lcs = baseLines.length * headLines.length > LCS_BUDGET
    ? []
    : longestCommonSubsequence(baseLines, headLines);

  const result: DiffLine[] = [];
  for (let i = 0; i < pre; i++) {
    result.push({ kind: 'equal', text: allBase[i], baseLine: i + 1, headLine: i + 1 });
  }
  const emit = (l: DiffLine): DiffLine => ({
    ...l,
    baseLine: l.baseLine === null ? null : l.baseLine + pre,
    headLine: l.headLine === null ? null : l.headLine + pre,
  });
  let bi = 0;
  let hi = 0;

  for (const [bIdx, hIdx] of lcs) {
    // Emit removed lines (in base but before this LCS match)
    while (bi < bIdx) {
      result.push(emit({ kind: 'removed', text: baseLines[bi], baseLine: bi + 1, headLine: null }));
      bi++;
    }
    // Emit added lines (in head but before this LCS match)
    while (hi < hIdx) {
      result.push(emit({ kind: 'added', text: headLines[hi], baseLine: null, headLine: hi + 1 }));
      hi++;
    }
    // Emit equal line
    result.push(emit({ kind: 'equal', text: baseLines[bi], baseLine: bi + 1, headLine: hi + 1 }));
    bi++;
    hi++;
  }

  // Remaining lines after last LCS match
  while (bi < baseLines.length) {
    result.push(emit({ kind: 'removed', text: baseLines[bi], baseLine: bi + 1, headLine: null }));
    bi++;
  }
  while (hi < headLines.length) {
    result.push(emit({ kind: 'added', text: headLines[hi], baseLine: null, headLine: hi + 1 }));
    hi++;
  }

  // The shared tail, numbered from where each side actually ends.
  for (let i = 0; i < suf; i++) {
    const b = allBase.length - suf + i;
    const h = allHead.length - suf + i;
    result.push({ kind: 'equal', text: allBase[b], baseLine: b + 1, headLine: h + 1 });
  }

  return result;
}

/** How many lines the diff adds and removes. */
export function changeCounts(lines: DiffLine[]): { added: number; removed: number } {
  let added = 0;
  let removed = 0;
  for (const l of lines) {
    if (l.kind === 'added') added++;
    else if (l.kind === 'removed') removed++;
  }
  return { added, removed };
}

/** One row of a side-by-side view: the before cell, the after cell, or both.
 *  A `null` cell is padding — the other side has a line here and this one
 *  doesn't. */
export interface SplitRow {
  base: DiffLine | null;
  head: DiffLine | null;
}

/**
 * Fold a unified diff into side-by-side rows.
 *
 * A run of removals followed by a run of additions is the shape of an edited
 * block, so the two runs are zipped: the first removed line sits opposite the
 * first added one. Zipping is what makes a one-word change on line 12 land on
 * one row instead of two screens apart, and it is the only reason a split
 * view beats a unified one for reading.
 */
export function toSplitRows(lines: DiffLine[]): SplitRow[] {
  const rows: SplitRow[] = [];
  let i = 0;
  while (i < lines.length) {
    if (lines[i].kind === 'equal') {
      rows.push({ base: lines[i], head: lines[i] });
      i++;
      continue;
    }
    const removed: DiffLine[] = [];
    const added: DiffLine[] = [];
    while (i < lines.length && lines[i].kind === 'removed') removed.push(lines[i++]);
    while (i < lines.length && lines[i].kind === 'added') added.push(lines[i++]);
    for (let j = 0; j < Math.max(removed.length, added.length); j++) {
      rows.push({ base: removed[j] ?? null, head: added[j] ?? null });
    }
  }
  return rows;
}

/** A stretch of the diff worth rendering, or a note about what was skipped. */
export type DiffChunk =
  | { kind: 'lines'; lines: DiffLine[] }
  | { kind: 'gap'; hidden: number };

/**
 * Drop the unchanged stretches that are more than `context` lines away from
 * any change, the way `git diff` does — a 200-line function with a two-line
 * edit is otherwise a wall the change hides inside.
 *
 * A gap is only worth taking if it saves more lines than the header that
 * announces it, so runs of 3 or fewer hidden lines stay in place.
 */
export function chunkDiff(lines: DiffLine[], context = 3): DiffChunk[] {
  const changed = lines.some((l) => l.kind !== 'equal');
  // Nothing moved: there is no change for context to be context *to*, and a
  // lone "42 lines hidden" note would be a diff view claiming to have folded
  // something away.
  if (!changed) return [{ kind: 'lines', lines }];

  const keep: boolean[] = new Array(lines.length).fill(false);
  lines.forEach((l, i) => {
    if (l.kind === 'equal') return;
    for (let j = Math.max(0, i - context); j <= Math.min(lines.length - 1, i + context); j++) {
      keep[j] = true;
    }
  });
  // A gap of three or fewer lines costs more to announce than to show.
  for (let i = 0; i < keep.length; ) {
    if (keep[i]) { i++; continue; }
    const start = i;
    while (i < keep.length && !keep[i]) i++;
    if (i - start <= 3) keep.fill(true, start, i);
  }

  const chunks: DiffChunk[] = [];
  for (let i = 0; i < lines.length; ) {
    if (keep[i]) {
      const start = i;
      while (i < lines.length && keep[i]) i++;
      chunks.push({ kind: 'lines', lines: lines.slice(start, i) });
    } else {
      const start = i;
      while (i < lines.length && !keep[i]) i++;
      chunks.push({ kind: 'gap', hidden: i - start });
    }
  }
  return chunks;
}

/**
 * Compute LCS of two string arrays.
 * Returns array of [baseIndex, headIndex] pairs for matching lines.
 * Uses standard DP approach — O(n*m) time and space.
 * For source files under ~1000 lines this is fast enough.
 */
function longestCommonSubsequence(a: string[], b: string[]): [number, number][] {
  const n = a.length;
  const m = b.length;

  // DP table
  const dp: number[][] = Array.from({ length: n + 1 }, () => new Array(m + 1).fill(0));

  for (let i = 1; i <= n; i++) {
    for (let j = 1; j <= m; j++) {
      if (a[i - 1] === b[j - 1]) {
        dp[i][j] = dp[i - 1][j - 1] + 1;
      } else {
        dp[i][j] = Math.max(dp[i - 1][j], dp[i][j - 1]);
      }
    }
  }

  // Backtrack to get the actual subsequence indices
  const result: [number, number][] = [];
  let i = n;
  let j = m;
  while (i > 0 && j > 0) {
    if (a[i - 1] === b[j - 1]) {
      result.push([i - 1, j - 1]);
      i--;
      j--;
    } else if (dp[i - 1][j] >= dp[i][j - 1]) {
      i--;
    } else {
      j--;
    }
  }

  result.reverse();
  return result;
}
