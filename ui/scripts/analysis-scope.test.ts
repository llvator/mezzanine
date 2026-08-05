/**
 * Unit tests for the Parsed Languages panel's staging rules.
 *
 * The rule under test is one line — when a staged selection may be collapsed
 * to `null` — and getting it wrong is invisible. `null` means "no filter",
 * the server reads that as its default, and the default excludes the opt-in
 * languages. So collapsing "everything is ticked" to `null` made the Markdown
 * checkbox tick and then do nothing: HTTP 200, a fresh analysis, and no notes
 * in it.
 *
 * Verified against a live server at the time of writing:
 *   POST {"languages": null}            -> 16 347 entities,   0 notes
 *   POST {"languages": [...all 21...]}  -> 16 844 entities, 497 notes
 *
 *   npm run test:analysis-scope
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  ALL_ANALYSIS_LANGUAGES,
  DEFAULT_ANALYSIS_LANGUAGES,
  OPT_IN_ANALYSIS_LANGUAGES,
  nextStagedLanguages,
  allOrNoneStaged,
} from '../src/utils/languageScope.ts';

/** Mirrors the store's single mutable cell without importing the store,
 *  which drags in `vscodeAdapter`. The rules under test are pure. */
let current: Set<string> | null = null;
const staged = (): Set<string> | null => current;
const reset = (): void => { current = null; };
const toggleStagedLanguage = (lang: string, checked: boolean): void => {
  current = nextStagedLanguages(current, lang, checked);
};
const setAllStagedLanguages = (checked: boolean): void => {
  current = allOrNoneStaged(checked);
};

// ---------------------------------------------------------------------
// The lists themselves
// ---------------------------------------------------------------------

test('every opt-in language is also a known language', () => {
  for (const lang of OPT_IN_ANALYSIS_LANGUAGES) {
    assert.ok(ALL_ANALYSIS_LANGUAGES.includes(lang), `${lang} missing from the full list`);
  }
});

test('the default set is everything except the opt-in languages', () => {
  assert.equal(
    DEFAULT_ANALYSIS_LANGUAGES.length,
    ALL_ANALYSIS_LANGUAGES.length - OPT_IN_ANALYSIS_LANGUAGES.length,
  );
  for (const lang of OPT_IN_ANALYSIS_LANGUAGES) {
    assert.ok(!DEFAULT_ANALYSIS_LANGUAGES.includes(lang));
  }
});

test('markdown is opt-in — the whole reason null and all-ticked differ', () => {
  assert.ok(OPT_IN_ANALYSIS_LANGUAGES.includes('markdown'));
});

// ---------------------------------------------------------------------
// Collapsing to null
// ---------------------------------------------------------------------

test('unticking then reticking a normal language returns to null', () => {
  reset();
  toggleStagedLanguage('rust', false);
  assert.ok(staged() !== null, 'a partial selection is an explicit set');
  toggleStagedLanguage('rust', true);
  assert.equal(staged(), null, 'back to exactly the server default');
});

test('ticking markdown does NOT collapse to null', () => {
  // The regression. Before the fix this hit `size === ALL.length` and
  // returned null, which is the one value that excludes markdown.
  reset();
  toggleStagedLanguage('markdown', true);
  const s = staged();
  assert.ok(s !== null, 'all-ticked is a different request from no-filter');
  assert.ok(s!.has('markdown'));
  assert.equal(s!.size, ALL_ANALYSIS_LANGUAGES.length);
});

test('unticking markdown again collapses back to null', () => {
  reset();
  toggleStagedLanguage('markdown', true);
  toggleStagedLanguage('markdown', false);
  assert.equal(staged(), null);
});

test('markdown plus a narrow selection stays explicit', () => {
  reset();
  for (const lang of DEFAULT_ANALYSIS_LANGUAGES) {
    if (lang !== 'rust') toggleStagedLanguage(lang, false);
  }
  toggleStagedLanguage('markdown', true);
  const s = staged();
  assert.deepEqual([...s!].sort(), ['markdown', 'rust']);
});

// ---------------------------------------------------------------------
// The master toggle
// ---------------------------------------------------------------------

test('select-all means all, including the opt-in languages', () => {
  reset();
  setAllStagedLanguages(true);
  const s = staged();
  assert.ok(s !== null, 'null would silently exclude markdown from an "all" toggle');
  assert.equal(s!.size, ALL_ANALYSIS_LANGUAGES.length);
  for (const lang of OPT_IN_ANALYSIS_LANGUAGES) {
    assert.ok(s!.has(lang), `${lang} must be in an explicit "all"`);
  }
});

test('select-none stages an empty set, which the panel refuses to apply', () => {
  reset();
  setAllStagedLanguages(false);
  assert.equal(staged()!.size, 0);
});

test('select-all then untick markdown is the server default again', () => {
  reset();
  setAllStagedLanguages(true);
  toggleStagedLanguage('markdown', false);
  assert.equal(staged(), null);
});

// ---------------------------------------------------------------------
// The docs switch vs the markdown checkbox
//
// The backend ORs them (`accepts_language`: `include_docs || named`), so the
// panel must not present them as one control or as two independent ones.
// ---------------------------------------------------------------------

/** Mirrors `AnalysisScopePanel.impliedByDocs`. */
const impliedByDocs = (lang: string, docsOn: boolean): boolean =>
  docsOn && OPT_IN_ANALYSIS_LANGUAGES.includes(lang);

test('the docs switch implies markdown whatever the language list says', () => {
  assert.ok(impliedByDocs('markdown', true));
});

test('the docs switch implies nothing else', () => {
  for (const lang of DEFAULT_ANALYSIS_LANGUAGES) {
    assert.ok(!impliedByDocs(lang, true), `${lang} must not be implied by the docs switch`);
  }
});

test('with the switch off, markdown is the language list business again', () => {
  assert.ok(!impliedByDocs('markdown', false));
});
