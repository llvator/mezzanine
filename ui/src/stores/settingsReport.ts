/**
 * The settings in effect, and which link of the precedence chain decided
 * each one (CFG-006, CFG-008, CFG-009).
 *
 * Read-only, and deliberately a snapshot of the *file* resolution rather
 * than of the live session. Narrowing the analysis scope next door changes
 * the graph without changing the file; reporting the narrowed set here would
 * attribute the reader's own experiment to their repo.
 *
 * Wire format: `GET /api/settings`. A 404 means a server too old to answer,
 * which is not an error worth showing — the panel simply does not draw the
 * section.
 */

import { writable, get } from 'svelte/store';
import { apiUrl } from '../vscodeAdapter';

/** Which link of the chain supplied a value. Mirrors `settings::report::Origin`. */
export type Origin = 'flag' | 'env' | 'repo-file' | 'user-file' | 'default';

/** What a key controls, which is what decides whether it is editable here. */
export type Tier = 'analysis' | 'view' | 'process';

export type Severity = 'rejected' | 'ignored' | 'malformed';

export interface Entry {
  value: string;
  source: Origin;
}

export interface Row {
  key: string;
  tier: Tier;
  value: unknown;
  /** Every link that contributed, highest first. One entry unless the key
   *  merges rather than overrides. */
  sources: Origin[];
  note?: string;
  /** Per-item origin, for the keys whose value is a concatenated list. */
  entries?: Entry[];
}

export interface SettingsWarning {
  file: string;
  key?: string;
  severity: Severity;
  message: string;
}

export interface SettingsReport {
  rows: Row[];
  warnings: SettingsWarning[];
  user_path: string | null;
  user_exists: boolean;
  repo_path: string | null;
  repo_exists: boolean;
  /** False under `serve`, which never reads a submitted repo's file. An
   *  empty repo scope there means "never looked", not "no file". */
  repo_scope_read: boolean;
}

export const settingsReport = writable<SettingsReport | null>(null);
export const settingsReportError = writable<string | null>(null);

/** How each origin is spelled in the UI. One place, so a badge and the prose
 *  beside it cannot drift. */
export const ORIGIN_LABEL: Record<Origin, string> = {
  flag: 'command line',
  env: 'environment',
  'repo-file': 'this repo',
  'user-file': 'your settings',
  default: 'default',
};

export const ORIGIN_TITLE: Record<Origin, string> = {
  flag: 'Passed as a flag to the command that started this server.',
  env: 'From a MEZZ_* environment variable.',
  'repo-file': "From this repo's .mezz/settings.json.",
  'user-file': 'From your ~/.config/mezz/settings.json.',
  default: "Nobody set this — it's mezz's built-in default.",
};

export const TIER_TITLE: Record<Tier, string> = {
  analysis: 'What gets parsed',
  view: 'What reaches the canvas',
  process: 'This process and installation',
};

export const TIER_BLURB: Record<Tier, string> = {
  analysis:
    'These decide what the analyzer reads. Change them in the Analysis Scope panel — it re-parses, and can save the result here as this repo’s default.',
  view: 'These narrow what is drawn out of what was parsed. The filter panel and saved views own them.',
  process: 'Read-only: these were consumed when the server started, so changing the file takes effect on the next restart.',
};

/** Fetch the report. Safe to call repeatedly — after a save, for instance. */
export async function loadSettingsReport(): Promise<void> {
  try {
    const resp = await fetch(apiUrl('/api/settings'));
    if (resp.status === 404) {
      // A server too old to have the route. Not worth reporting: the rest of
      // the panel works, this section just stays away.
      settingsReport.set(null);
      settingsReportError.set(null);
      return;
    }
    if (!resp.ok) {
      settingsReportError.set(await resp.text());
      return;
    }
    settingsReport.set((await resp.json()) as SettingsReport);
    settingsReportError.set(null);
  } catch (e) {
    settingsReportError.set(e instanceof Error ? e.message : String(e));
  }
}

/** Rows of one tier, in the order the server listed them. */
export function rowsOf(report: SettingsReport | null, tier: Tier): Row[] {
  return report ? report.rows.filter((r) => r.tier === tier) : [];
}

/** Warnings about one key, so they can sit beside the value they concern
 *  rather than in a separate log. */
export function warningsFor(report: SettingsReport | null, key: string): SettingsWarning[] {
  return report ? report.warnings.filter((w) => w.key === key) : [];
}

/** Warnings that belong to no single key — a file that failed to parse is
 *  the one that matters, and it is the easiest to miss. */
export function generalWarnings(report: SettingsReport | null): SettingsWarning[] {
  return report ? report.warnings.filter((w) => !w.key) : [];
}

/** Render a value the way the file would spell it. */
export function displayValue(value: unknown): string {
  if (value === null || value === undefined) return 'not set';
  if (Array.isArray(value)) return value.length ? value.join(', ') : 'empty';
  if (typeof value === 'boolean') return value ? 'on' : 'off';
  return String(value);
}

/** True once a report has been fetched at least once this session. */
export function hasReport(): boolean {
  return get(settingsReport) !== null;
}
