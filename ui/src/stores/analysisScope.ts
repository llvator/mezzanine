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
import { isServeMode } from './serveMode';
import { settingsReport, type SettingsReport } from './settingsReport';
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

/** Where the Elevator spec lives, when it isn't "every `.elv` under the
 *  root" — the default, written here as the empty string because this is a
 *  text field and `null` would need special-casing at every use.
 *
 *  A session override, not a saved setting: the durable answer is `spec_dir`
 *  in the repo's `.nao/settings.json`, and this field is seeded from it. The
 *  browser deliberately does not write that file back — a page should not be
 *  able to edit the file that decides which directories nao reads. */
export const appliedSpecDir = writable<string>('');
export const stagedSpecDir = writable<string>('');

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
      spec_dir?: string | null;
    };
    const langs = data.languages === null ? null : new Set(data.languages);
    appliedAnalysisLanguages.set(langs);
    stagedAnalysisLanguages.set(langs);
    appliedIncludeDocs.set(!!data.include_docs);
    stagedIncludeDocs.set(!!data.include_docs);
    appliedSpecDir.set(data.spec_dir ?? '');
    stagedSpecDir.set(data.spec_dir ?? '');
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
  [
    stagedAnalysisLanguages,
    appliedAnalysisLanguages,
    stagedIncludeDocs,
    appliedIncludeDocs,
    stagedSpecDir,
    appliedSpecDir,
  ],
  ([$staged, $applied, $stagedDocs, $appliedDocs, $stagedSpec, $appliedSpec]) =>
    !setsEqual($staged, $applied) ||
    $stagedDocs !== $appliedDocs ||
    $stagedSpec.trim() !== $appliedSpec.trim(),
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
  stagedSpecDir.set(get(appliedSpecDir));
  analysisScopeError.set(null);
}

/** True while a save-as-default is in flight. */
export const analysisScopeSaving = writable<boolean>(false);

/** Set after a successful save, cleared as soon as the scope is staged
 *  differently again — a "saved" badge that outlives the thing it describes
 *  is worse than none. */
export const analysisScopeSaved = writable<boolean>(false);

/**
 * Promote the *applied* scope to this repo's default, by writing the
 * analysis keys into `<root>/.nao/settings.json` (CFG-010).
 *
 * The applied scope, not the staged one: saving something the reader has not
 * yet seen the graph for would make the button a second, quieter Apply. It
 * also means saving never re-analyzes — there is nothing to recompute.
 *
 * Not offered in serve mode. `nao serve` never reads a submitted repo's
 * settings file, so a file written there would be one nothing will ever read
 * (ADR-0008); the route does not exist there either.
 */
export async function saveScopeAsRepoDefault(): Promise<boolean> {
  if (isServeMode()) return false;
  analysisScopeSaving.set(true);
  analysisScopeError.set(null);
  try {
    const resp = await fetch(apiUrl('/api/settings/analysis'), { method: 'POST' });
    if (!resp.ok) {
      // The server refuses rather than overwrites when the existing file
      // cannot be parsed, and rather than writing a spec_dir the loader
      // would reject on the next start. Both send prose worth showing.
      throw new Error((await resp.text()) || `HTTP ${resp.status}`);
    }
    settingsReport.set((await resp.json()) as SettingsReport);
    analysisScopeSaved.set(true);
    return true;
  } catch (e) {
    analysisScopeError.set(`Could not save: ${e instanceof Error ? e.message : e}`);
    return false;
  } finally {
    analysisScopeSaving.set(false);
  }
}

/** Apply staged changes: POST to the backend. Returns true on success. */
export async function applyAnalysisScope(): Promise<boolean> {
  const staged = get(stagedAnalysisLanguages);
  const stagedDocs = get(stagedIncludeDocs);
  // The empty string is not "unchanged" but "clear it" — the backend reads
  // it that way (see `AnalysisScopeRequest::spec_dir`), which is how the
  // field can be emptied to get back to every `.elv` under the root.
  const stagedSpec = get(stagedSpecDir).trim();
  analysisScopeApplying.set(true);
  analysisScopeError.set(null);
  // A newly applied scope is not the saved one, whatever was saved before.
  analysisScopeSaved.set(false);
  try {
    const body = JSON.stringify({
      languages: staged === null ? null : [...staged],
      include_docs: stagedDocs,
      spec_dir: stagedSpec,
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
    appliedSpecDir.set(stagedSpec);
    stagedSpecDir.set(stagedSpec);
    return true;
  } catch (e) {
    analysisScopeError.set(`Failed to apply: ${e}`);
    return false;
  } finally {
    analysisScopeApplying.set(false);
  }
}
