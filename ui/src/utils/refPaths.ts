/**
 * Path arithmetic shared by every code-reference question.
 *
 * A `cr:` declares a path ("this Feature lives in `src/parser/`", "this note
 * is about `src/analyzer/mod.rs`") and two questions get asked of it: does it
 * still point at real code, and which files does it claim. Both are string
 * comparisons against the `file_path`s in the graph, and both were previously
 * written inline in `stores/codeRefs.ts` — where they could not be tested
 * without booting the whole Svelte store graph.
 *
 * They live here for the same reason `scopeRules.ts` does: the interesting
 * properties are ordering and normalization ones, and a click-through in the
 * browser cannot demonstrate them. The bug that prompted the extraction is
 * pinned in `scripts/ref-paths.test.ts` — see [`normalizeRefPath`].
 */

/**
 * Canonical form for comparing a declared path against a `file_path`.
 *
 * Strips leading `./` and trailing `/`, so `./src/parser/` and `src/parser`
 * compare equal. Authors write both.
 *
 * **This must be applied to both sides of every comparison.** It originally
 * was not: refs went through it and `file_path`s did not. The renderer writes
 * `file_path` relative to the analysis root, so a bare `mezz watch` — whose
 * root is `.` — emitted `./src/parser/mod.rs` while the ref beside it said
 * `src/parser`. Nothing matched, and the UI reported every reference in the
 * graph as drift while looking entirely healthy about it.
 */
export function normalizeRefPath(path: string): string {
  let p = (path ?? '').trim();
  while (p.startsWith('./')) p = p.slice(2);
  while (p.endsWith('/')) p = p.slice(0, -1);
  return p;
}

/**
 * Every path a `cr:` could legitimately name: each file, plus each of its
 * ancestor directories, so a ref naming a folder resolves against the files
 * inside it. Precomputing the ancestors turns resolution into a set lookup
 * instead of a prefix scan over every file per reference.
 *
 * Ghost nodes (external/stdlib refs, empty `file_path`) contribute nothing —
 * they aren't files in this tree.
 */
export function buildPathUniverse(filePaths: Iterable<string>): Set<string> {
  const universe = new Set<string>();
  for (const raw of filePaths) {
    const path = normalizeRefPath(raw ?? '');
    if (!path) continue;
    universe.add(path);
    let cut = path.lastIndexOf('/');
    while (cut > 0) {
      universe.add(path.slice(0, cut));
      cut = path.lastIndexOf('/', cut - 1);
    }
  }
  return universe;
}

/**
 * Whether a declared path claims a file — either by naming it exactly or by
 * naming a directory above it.
 *
 * A plain `startsWith` would be wrong: `src/parser` must not claim
 * `src/parser_old/x.rs`. The separator is part of the test.
 */
export function pathClaims(declared: string, filePath: string): boolean {
  const subject = normalizeRefPath(filePath);
  const claim = normalizeRefPath(declared);
  if (!subject || !claim) return false;
  return subject === claim || subject.startsWith(claim + '/');
}

/**
 * Whether *any* of `declared` claims `filePath`.
 *
 * The plural of [`pathClaims`], and the call the split view's cross-filter
 * makes per node per recompute — a spec entity claims a *set* of paths (its
 * own `cr:` plus everything its subtree declares), and the question asked of
 * that set is always "does it cover this file".
 *
 * An empty list claims nothing. That is the whole distinction the cross-filter
 * rests on: "this entity declares no code" must draw an empty canvas, not an
 * unfiltered one.
 */
export function pathsClaim(declared: readonly string[], filePath: string): boolean {
  if (declared.length === 0 || !filePath) return false;
  return declared.some((path) => pathClaims(path, filePath));
}

/**
 * Declared paths ordered most-specific first, so a file claimed by both
 * `src/parser/rust/inference.rs` and `src/parser/` hears about the precise
 * claim before the folder-wide one.
 */
export function bySpecificity(paths: Iterable<string>): string[] {
  return [...paths].sort((a, b) => b.length - a.length || a.localeCompare(b));
}
