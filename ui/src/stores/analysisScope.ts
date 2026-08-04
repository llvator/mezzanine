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

/** All languages the backend understands. Mirrors the `Language`
 *  enum in `src/models/file_info.rs` (sans `Unknown`). Hardcoded
 *  because the list is small and stable; if it grows often, expose
 *  it from the server. */
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
  'elevator',
] as const;

/** What's *currently* analyzed. `null` means "no filter — everything". */
export const appliedAnalysisLanguages = writable<Set<string> | null>(null);

/** What the user has staged but not applied yet. Mirrors `applied` on
 *  load and after a successful apply. Apply button checks `staged !==
 *  applied`. */
export const stagedAnalysisLanguages = writable<Set<string> | null>(null);

/** True while an apply is in flight. The button shows a spinner and is
 *  disabled until this clears. */
export const analysisScopeApplying = writable<boolean>(false);

/** Last error message from the apply call, or null if the last apply
 *  succeeded (or none has happened yet). */
export const analysisScopeError = writable<string | null>(null);

/** True when the staged set differs from the applied set. The Apply
 *  button is enabled iff this is true. */
export const analysisScopeDirty = derived(
  [stagedAnalysisLanguages, appliedAnalysisLanguages],
  ([$staged, $applied]) => !setsEqual($staged, $applied),
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
  stagedAnalysisLanguages.update((s) => {
    const ns = s === null ? new Set(ALL_ANALYSIS_LANGUAGES) : new Set(s);
    if (checked) ns.add(lang);
    else ns.delete(lang);
    // If the user re-checks every language, treat it as "no filter"
    // so the wire shape matches the startup default.
    if (ns.size === ALL_ANALYSIS_LANGUAGES.length) return null;
    return ns;
  });
}

/** Stage every language, or none of them. "Every" is `null` rather than a
 *  full Set so the wire shape matches the startup default (see
 *  `toggleStagedLanguage`). "None" is an empty Set, which is *not* a valid
 *  thing to apply — the backend reads `[]` as "no filter" — so the panel
 *  disables Apply while the staged set is empty; it exists as the fast way
 *  to get to "just this one language" without unticking sixteen boxes. */
export function setAllStagedLanguages(checked: boolean): void {
  stagedAnalysisLanguages.set(checked ? null : new Set());
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
  analysisScopeError.set(null);
}

/** Apply staged changes: POST to the backend. Returns true on success. */
export async function applyAnalysisScope(): Promise<boolean> {
  const staged = get(stagedAnalysisLanguages);
  analysisScopeApplying.set(true);
  analysisScopeError.set(null);
  try {
    const body = JSON.stringify({
      languages: staged === null ? null : [...staged],
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
    return true;
  } catch (e) {
    analysisScopeError.set(`Failed to apply: ${e}`);
    return false;
  } finally {
    analysisScopeApplying.set(false);
  }
}
