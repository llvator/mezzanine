/**
 * The Parsed Languages selection rules — which languages exist, and when a
 * staged selection may be collapsed to `null`.
 *
 * Pure set arithmetic, extracted from `stores/analysisScope.ts` for the same
 * reason `refPaths.ts` was: the store imports `vscodeAdapter`, so importing it
 * from a test drags in the browser adapter and fails to resolve. The rules
 * below are the part worth testing and none of them need a store.
 *
 * The rule that matters is [`isServerDefault`]. Getting it wrong is invisible:
 * the request succeeds, the analysis re-runs, and the thing you ticked is
 * missing from the result.
 */

/** Every language the backend understands. Mirrors the `Language` enum in
 *  `src/models/file_info.rs` (sans `Unknown`), and each entry must be a name
 *  `Language::from_name` accepts — an unknown string is rejected by the
 *  server, so a typo here is a checkbox that errors.
 *
 *  Hardcoded, and it has drifted every time the enum grew. The Rust test
 *  `the_ui_language_list_matches_the_enum` now fails when they disagree. */
export const ALL_ANALYSIS_LANGUAGES: readonly string[] = [
  'rust',
  'python',
  'javascript',
  'typescript',
  'java',
  'go',
  'csharp',
  'cpp',
  'c',
  'ruby',
  'swift',
  'kotlin',
  'scala',
  'php',
  'groovy',
  'impex',
  'svelte',
  'elevator',
  'ansible',
  'sql',
  'markdown',
] as const;

/** Languages the backend will not analyze unless *named*
 *  (`Language::is_opt_in`). Markdown is the only one: a repo's `.md` files
 *  outnumber its source in many checkouts, so "no filter" leaves them out. */
export const OPT_IN_ANALYSIS_LANGUAGES: readonly string[] = ['markdown'] as const;

/** What `null` actually analyzes: every language except the opt-in ones. */
export const DEFAULT_ANALYSIS_LANGUAGES: readonly string[] =
  ALL_ANALYSIS_LANGUAGES.filter((l) => !OPT_IN_ANALYSIS_LANGUAGES.includes(l));

/**
 * Whether a staged set is exactly what `null` would produce.
 *
 * `null` and "every box ticked" are **different requests**, and conflating
 * them is the bug this function exists to prevent: the set collapsed to
 * `null`, the server read that as its default, and the default excludes
 * Markdown — so the checkbox ticked and nothing changed. Measured on a live
 * server: `null` gave 0 notes where the explicit list gave 497.
 */
export function isServerDefault(staged: ReadonlySet<string>): boolean {
  return (
    staged.size === DEFAULT_ANALYSIS_LANGUAGES.length &&
    DEFAULT_ANALYSIS_LANGUAGES.every((l) => staged.has(l))
  );
}

/**
 * The staged value after ticking or unticking one language.
 *
 * `null` in means "currently the server default"; `null` out means the
 * selection landed back on it and should be sent as no filter at all.
 */
export function nextStagedLanguages(
  current: ReadonlySet<string> | null,
  lang: string,
  checked: boolean,
): Set<string> | null {
  const next = current === null ? new Set(DEFAULT_ANALYSIS_LANGUAGES) : new Set(current);
  if (checked) next.add(lang);
  else next.delete(lang);
  return isServerDefault(next) ? null : next;
}

/**
 * The staged value for the master toggle.
 *
 * "All" is an explicit full set, never `null` — a toggle labelled *all* that
 * quietly analyzed less than everything would be the same lie in a louder
 * place. "None" is an empty set, which the panel refuses to apply.
 */
export function allOrNoneStaged(checked: boolean): Set<string> {
  return new Set(checked ? ALL_ANALYSIS_LANGUAGES : []);
}
