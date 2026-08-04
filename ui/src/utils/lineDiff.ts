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
 * Compute a line-level diff between two strings.
 * Returns an array of DiffLine entries suitable for rendering.
 */
export function computeLineDiff(base: string, head: string): DiffLine[] {
  const baseLines = base.split('\n');
  const headLines = head.split('\n');
  const lcs = longestCommonSubsequence(baseLines, headLines);

  const result: DiffLine[] = [];
  let bi = 0;
  let hi = 0;

  for (const [bIdx, hIdx] of lcs) {
    // Emit removed lines (in base but before this LCS match)
    while (bi < bIdx) {
      result.push({ kind: 'removed', text: baseLines[bi], baseLine: bi + 1, headLine: null });
      bi++;
    }
    // Emit added lines (in head but before this LCS match)
    while (hi < hIdx) {
      result.push({ kind: 'added', text: headLines[hi], baseLine: null, headLine: hi + 1 });
      hi++;
    }
    // Emit equal line
    result.push({ kind: 'equal', text: baseLines[bi], baseLine: bi + 1, headLine: hi + 1 });
    bi++;
    hi++;
  }

  // Remaining lines after last LCS match
  while (bi < baseLines.length) {
    result.push({ kind: 'removed', text: baseLines[bi], baseLine: bi + 1, headLine: null });
    bi++;
  }
  while (hi < headLines.length) {
    result.push({ kind: 'added', text: headLines[hi], baseLine: null, headLine: hi + 1 });
    hi++;
  }

  return result;
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
