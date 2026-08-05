/**
 * Analysis-scope (parent filter): controls which languages the backend
 * even bothers to parse. Distinct from the visual Languages filter,
 * which only hides nodes that already exist in the dataset — this one
 * shrinks the dataset itself, by re-running the analyzer with a new
 * language set on Apply.
 *
 * Wire format: `POST /api/analysis/scope` with
 *   `{ languages: ["rust", "python", ...] | null }`
 * where `null` (or `[]`) means "no filter — analyze every supported
 * language", which is the startup default.
 *
 * The backend cancels any in-flight analysis when this endpoint fires,
 * so toggling Apply mid-run is safe.
 */

import { writable, derived, get } from 'svelte/store';
import { apiUrl } from '../vscodeAdapter';
import { nextStagedLanguages, allOrNoneStaged } from '../utils/languageScope';

/** The language lists and the staging rules live in
 *  `utils/languageScope.ts` so they can be tested without importing this
 *  store — which pulls in `vscodeAdapter` and, with it, the browser.
 *  Re-exported here because this store is the public face of the panel. */
export {
  ALL_ANALYSIS_LANGUAGES,
  OPT_IN_ANALYSIS_LANGUAGES,
  DEFAULT_ANALYSIS_LANGUAGES,
} from '../utils/languageScope';

/** What's *currently* analyzed. `null` means "no filter — everything". */
export const appliedAnalysisLanguages = writable<Set<string> | null>(null);

/** What the user has staged but not applied yet. Mirrors `applied` on
 *  load and after a successful apply. Apply button checks `staged !==
 *  applied`. */
export const stagedAnalysisLanguages = writable<Set<string> | null>(null);

/** Whether Markdown is currently analyzed — the widening switch, the same
 *  one `--include-docs` and `nao.includeDocs` set. Distinct from ticking
 *  `markdown` in the language list, which *restricts* to a set that happens
 *  to contain it; this one survives whatever the list says. */
export const appliedIncludeDocs = writable<boolean>(false);
export const stagedIncludeDocs = writable<boolean>(false);

/** Seed both from the server. Without this the panel assumes "no filter,
 *  no docs" — and against a server started with `--include-docs` the first
 *  Apply would send `include_docs: false` and turn docs off for real. */
export async function loadAnalysisScope(): Promise<void> {
  try {
    const resp = await fetch(apiUrl('/api/analysis/scope'));
    if (!resp.ok) return;
    const data = (await resp.json()) as {
      languages: string[] | null;
      include_docs: boolean;
    };
    const langs = data.languages === null ? null : new Set(data.languages);
    appliedAnalysisLanguages.set(langs);
    stagedAnalysisLanguages.set(langs);
    appliedIncludeDocs.set(!!data.include_docs);
    stagedIncludeDocs.set(!!data.include_docs);
  } catch {
    // A server too old to answer leaves the defaults in place. Nothing to
    // report — the panel is still usable, it just starts from an assumption.
  }
}

/** Toggle the docs switch. */
export function setStagedIncludeDocs(checked: boolean): void {
  stagedIncludeDocs.set(checked);
}

/** True while an apply is in flight. The button shows a spinner and is
 *  disabled until this clears. */
export const analysisScopeApplying = writable<boolean>(false);

/** Last error message from the apply call, or null if the last apply
 *  succeeded (or none has happened yet). */
export const analysisScopeError = writable<string | null>(null);

/** True when the staged set differs from the applied set. The Apply
 *  button is enabled iff this is true. */
export const analysisScopeDirty = derived(
  [stagedAnalysisLanguages, appliedAnalysisLanguages, stagedIncludeDocs, appliedIncludeDocs],
  ([$staged, $applied, $stagedDocs, $appliedDocs]) =>
    !setsEqual($staged, $applied) || $stagedDocs !== $appliedDocs,
);

function setsEqual(a: Set<string> | null, b: Set<string> | null): boolean {
  if (a === null && b === null) return true;
  if (a === null || b === null) return false;
  if (a.size !== b.size) return false;
  for (const v of a) if (!b.has(v)) return false;
  return true;
}

/** Toggle a language in the staged set. `null` (no filter) collapses
 *  to a fresh Set on first interaction so the user can opt into a
 *  narrower scope without separately "starting" the filter. */
export function toggleStagedLanguage(lang: string, checked: boolean): void {
  stagedAnalysisLanguages.update((s) => nextStagedLanguages(s, lang, checked));
}

/** Stage every language, or none of them.
 *
 *  "Every" is an explicit full Set, *not* `null`. `null` means the server
 *  default, which excludes the opt-in languages — so sending it for a
 *  master-toggle labelled "all" would quietly analyze less than it claims.
 *
 *  "None" is an empty Set, which is not a valid thing to apply — the backend
 *  reads `[]` as "no filter" — so the panel disables Apply while the staged
 *  set is empty; it exists as the fast way to get to "just this one language"
 *  without unticking twenty boxes. */
export function setAllStagedLanguages(checked: boolean): void {
  stagedAnalysisLanguages.set(allOrNoneStaged(checked));
}

/** True when the user has staged an empty language set. Nothing can be
 *  analyzed in that state, so Apply stays disabled. */
export const stagedScopeEmpty = derived(
  stagedAnalysisLanguages,
  ($staged) => $staged !== null && $staged.size === 0,
);

/** Discard staged changes. Used by the Cancel button. */
export function resetStaged(): void {
  stagedAnalysisLanguages.set(get(appliedAnalysisLanguages));
  stagedIncludeDocs.set(get(appliedIncludeDocs));
  analysisScopeError.set(null);
}

/** Apply staged changes: POST to the backend. Returns true on success. */
export async function applyAnalysisScope(): Promise<boolean> {
  const staged = get(stagedAnalysisLanguages);
  const stagedDocs = get(stagedIncludeDocs);
  analysisScopeApplying.set(true);
  analysisScopeError.set(null);
  try {
    const body = JSON.stringify({
      languages: staged === null ? null : [...staged],
      include_docs: stagedDocs,
    });
    const resp = await fetch(apiUrl('/api/analysis/scope'), {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body,
    });
    if (!resp.ok) {
      const text = await resp.text();
      throw new Error(text || `HTTP ${resp.status}`);
    }
    const data = (await resp.json()) as {
      success: boolean;
      message?: string;
    };
    if (!data.success) {
      analysisScopeError.set(data.message ?? 'Failed to apply analysis scope');
      return false;
    }
    // The SSE 'reload' event will refresh the graph; we just commit
    // the applied state here so the UI can stop showing "dirty".
    appliedAnalysisLanguages.set(staged);
    appliedIncludeDocs.set(stagedDocs);
    return true;
  } catch (e) {
    analysisScopeError.set(`Failed to apply: ${e}`);
    return false;
  } finally {
    analysisScopeApplying.set(false);
  }
}
