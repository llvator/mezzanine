/**
 * Git's list of changed files, joined against what the graph says about the
 * same change (UI-134).
 *
 * Every other diff reading in this app is a projection of `diff.json`, which
 * is one row per *entity*. That makes the graph unable to check itself: a file
 * the analysis never loaded has no row to be missing from, so "outside the
 * configured languages" and "nothing changed" arrive as the same silence. Git
 * is the other side of the join, and the join is what this module computes.
 *
 * Pure and store-free on purpose — this is the half that makes a claim about
 * correctness, so it is the half that has to be testable without a browser.
 */

// Both imports stay erasable — `Commit`/`Stash` are types and `refLabel`'s own
// only import is a type — so this module still loads under `node --test`
// without pulling svelte in behind it. That is the property the whole file is
// written for; a value import from `../stores/diff` would end it.
import type { Commit, Stash } from '../stores/diff';
// `.ts` on the value import, the way `diffScope` and `diffChurn` spell theirs:
// vite resolves it either way, `node --test` only this one.
import { refLabel } from './refLabel.ts';
import type { RefLabel } from './refLabel';

/** One path in the change, exactly as `POST /api/changed-files` reports it. */
export interface ChangedFile {
  /** Git's own letter — `A`, `M`, `D`, `R`, `C`, `T`. Kept as given. */
  status: string;
  path: string;
  /** Where a rename came from, so the base side can be asked for by name. */
  old_path?: string;
  additions: number;
  deletions: number;
  /** Git could not count it in lines, so `additions`/`deletions` are 0. */
  binary: boolean;
  /** Never added to the index: the one row with no base side in any tree. */
  untracked: boolean;
}

/**
 * What the graph has to say about a file git reports as changed.
 *
 * Four answers, because they mean four different things to a reader checking
 * the canvas against the change:
 *
 * - `drawn` — `entities` of this file's entities changed at source level, so
 *   the ladder's narrowest rung seeds from them and the canvas draws the file.
 * - `silent` — the file is in the analysis and nothing in it changed at source
 *   level. Ordinary for a comment or whitespace edit; worth a second look
 *   otherwise, because it is also what a parser that stopped seeing a
 *   construct looks like.
 * - `unanalysed` — the file is outside the analysis: another language, an
 *   excluded pattern, a lockfile, an image. The canvas is silent about it by
 *   construction and is not wrong to be.
 * - `unknown` — the analysed file set has not arrived yet. Stated rather than
 *   guessed: reading "not loaded" as "not analysed" would put a false
 *   accusation next to every row for as long as the fetch takes.
 */
export type GraphAgreement =
  | { kind: 'drawn'; entities: number }
  | { kind: 'silent' }
  | { kind: 'unanalysed' }
  | { kind: 'unknown' };

export interface ChangedFileRow {
  file: ChangedFile;
  agreement: GraphAgreement;
}

/** What the pane needs to draw, in one value. */
export interface ChangedFilesReading {
  rows: ChangedFileRow[];
  /**
   * Files the *diff* reports changed entities for and git does not list.
   *
   * Empty on a healthy comparison, and the reason this pane is an instrument
   * rather than a listing: a non-empty residue means the two disagree about
   * what changed — a diff left over from a previous comparison, or a path
   * spelling that stopped matching between `diff.json` and the tree.
   */
  onlyInGraph: string[];
  totals: { files: number; additions: number; deletions: number };
}

/**
 * One `diff.json` row, as much of it as the join reads.
 */
export interface DiffRow {
  file_path: string;
  status: 'added' | 'removed' | 'modified' | 'unchanged';
  source_changed: boolean;
}

/**
 * Whether a row is a change *to this file*, rather than a change this file
 * felt.
 *
 * The rule `editKind` states for a drawn node, restated over a row: an entity
 * whose `fan_in`/`fan_out` moved because something nearby changed has not been
 * edited, and on a real diff that ripple outnumbers the edits several times
 * over — 474 impact-only rows against 60 real ones on the change this pane was
 * built under.
 *
 * Counting the ripple was the first thing this join got wrong, and the way it
 * showed was not a wrong number: it was eighteen files in **Only in the
 * graph** that git was right to leave out, turning the one signal worth
 * reading into permanent noise. The two rules have to be the same rule, or the
 * pane accuses the canvas of drawing files it never drew.
 */
export function isSourceEdit(row: DiffRow): boolean {
  return row.status !== 'unchanged' && row.source_changed;
}

/** What the join needs from the rest of the app. */
export interface GraphFacts {
  /**
   * How many entities each file changed *at source level*, from `diff.json`
   * through `isSourceEdit`. These are the entities the ladder's narrowest rung
   * seeds from, so the number is what the canvas draws for that file.
   */
  changedEntities: Map<string, number>;
  /**
   * Every path the analysis loaded, on either side of the comparison.
   *
   * `null` while the detail sidecars are still in flight — see `unknown`
   * above. Both sides, because a *deleted* file is absent from the head
   * sidecar for the honest reason that it is gone, and calling it unanalysed
   * would be exactly backwards.
   */
  analysed: Set<string> | null;
}

/**
 * The status letters this pane knows how to name. Git's own vocabulary, and a
 * letter that is not here still reaches the reader as itself — the rule
 * `StagedFile` follows server-side, held to on this end too.
 */
export const STATUS_LABELS: Record<string, string> = {
  A: 'added',
  M: 'modified',
  D: 'deleted',
  R: 'renamed',
  C: 'copied',
  T: 'type changed',
};

export function statusLabel(status: string): string {
  return STATUS_LABELS[status] ?? status;
}

/**
 * The letter a reader sees, which is not always the letter git sends.
 *
 * `git diff --name-status` has no code for "untracked" — it only reports paths
 * git is already following — so `/api/changed-files` tags a never-added file
 * `A` and sets `untracked`, which is faithful to git's vocabulary and is the
 * right thing to *store*. It is the wrong thing to *show*: `A` beside a staged
 * addition and `A` beside a file git has never heard of are the same claim,
 * and only one of them survives a checkout.
 *
 * `U` is the letter every source-control pane in VS Code uses for this, and
 * telling the two apart is most of why a reader opens the list at all.
 *
 * The row keeps both — `status` as git gave it, the letter as a reader reads
 * it — so nothing downstream has to know which of the two it is holding.
 */
export function statusLetter(file: Pick<ChangedFile, 'status' | 'untracked'>): string {
  return file.untracked ? 'U' : file.status;
}

/**
 * The same distinction in words, for the tooltip and the screen reader.
 *
 * "untracked" on its own rather than "added, untracked": a file git is not
 * following has not been added to anything, and naming it twice invites the
 * reading that it was.
 */
export function statusPhrase(file: Pick<ChangedFile, 'status' | 'untracked'>): string {
  return file.untracked ? 'untracked' : statusLabel(file.status);
}

/**
 * Which of the diff colours a status letter reads as.
 *
 * Renames and copies are `modified`: the file survived, and giving a `git mv`
 * the same green as new code would make a restructure read as a rewrite.
 */
export function statusChange(status: string): 'added' | 'removed' | 'modified' {
  if (status === 'A') return 'added';
  if (status === 'D') return 'removed';
  return 'modified';
}

/**
 * The ref to send for a head that `diff.json` reports as a literal.
 *
 * Two vocabularies meet here and nowhere else. `diff.json` carries `working`
 * or `staged` in `to_ref` — the discriminator `diffHeadIsWorking` already
 * reads — while the endpoints take the `WORKING` / `STAGED` sentinels the diff
 * endpoint defined. Sending the lowercase literal through would reach git as a
 * ref that does not resolve, and the failure would present as an empty list of
 * changed files rather than as an error.
 */
export function headRefFor(toRef: string | undefined): string | null {
  if (!toRef) return null;
  if (toRef === 'working') return 'WORKING';
  if (toRef === 'staged') return 'STAGED';
  return toRef;
}

/**
 * Join git's list with the graph's reading of the same change.
 *
 * Order of the tests matters and is not arbitrary: a file with changed
 * entities is `drawn` whatever the analysed set says about it, because the
 * diff having found entities *is* the proof it was analysed. Only a file the
 * diff is silent about needs the set consulted at all, which is what keeps a
 * deleted file — absent from the head sidecar because it is gone — out of the
 * `unanalysed` bucket.
 */
export function readChangedFiles(
  files: ChangedFile[],
  facts: GraphFacts,
): ChangedFilesReading {
  const rows = files.map((file) => ({ file, agreement: agreementFor(file, facts) }));
  const listed = new Set(files.map((f) => f.path));
  for (const f of files) if (f.old_path) listed.add(f.old_path);
  const onlyInGraph = [...facts.changedEntities.keys()]
    .filter((p) => p && !listed.has(p))
    .sort();
  return {
    rows,
    onlyInGraph,
    totals: {
      files: files.length,
      additions: files.reduce((n, f) => n + f.additions, 0),
      deletions: files.reduce((n, f) => n + f.deletions, 0),
    },
  };
}

function agreementFor(file: ChangedFile, facts: GraphFacts): GraphAgreement {
  const entities = facts.changedEntities.get(file.path)
    ?? (file.old_path ? facts.changedEntities.get(file.old_path) : undefined);
  if (entities) return { kind: 'drawn', entities };
  if (!facts.analysed) return { kind: 'unknown' };
  const analysed = facts.analysed.has(file.path)
    || (!!file.old_path && facts.analysed.has(file.old_path));
  return analysed ? { kind: 'silent' } : { kind: 'unanalysed' };
}

/**
 * How the row says what the graph will do with it. One short phrase, because
 * it sits at the end of a row in a 360px column.
 */
export function agreementLabel(a: GraphAgreement): string {
  switch (a.kind) {
    case 'drawn':
      return `${a.entities} ${a.entities === 1 ? 'entity' : 'entities'}`;
    case 'silent':
      return 'no entity changed';
    case 'unanalysed':
      return 'not analysed';
    case 'unknown':
      return '';
  }
}

/** The tooltip behind that phrase — the part that says whether to worry. */
export function agreementHint(a: GraphAgreement): string {
  switch (a.kind) {
    case 'drawn':
      return `${a.entities} ${a.entities === 1 ? 'entity' : 'entities'} in this file changed at source level, so the Edits rung seeds from ${a.entities === 1 ? 'it' : 'them'} and the canvas draws this file.`;
    case 'silent':
      return 'The analysis loaded this file and no entity in it changed at source level — a comment or whitespace edit, a change to something the parser does not model, or only fan-in/fan-out movement from a change elsewhere.';
    case 'unanalysed':
      return 'Outside the analysis: another language, an excluded pattern, or a file no parser reads. The canvas cannot draw it, and its absence there means nothing.';
    case 'unknown':
      return 'The analysed file list has not loaded yet.';
  }
}

/**
 * The reading in the shape the extension's native Changes tree takes (UI-137).
 *
 * The tree cannot import this module — the extension's `tsconfig` sets
 * `rootDir: src` — so the *finished* reading crosses the bridge instead, and
 * that is the point rather than a workaround: a second copy of the join on the
 * far side would be a join that can disagree with itself, about `onlyInGraph`
 * above all, which is the one number whose whole value is being zero.
 *
 * The agreement travels as its rendered `label` and `hint` for the same
 * reason. Both surfaces then say the same sentence about the same row by
 * construction rather than by two developers keeping two switch statements in
 * step.
 *
 * Pure, and here rather than inline in `App.svelte`, because this is the part
 * that makes a claim — that the tree is shown exactly what the tab is shown.
 */
export interface ChangedFilesPayload {
  active: boolean;
  fromRef?: string;
  /** As `diff.json` spells it: `working`, `staged`, or a sha. For display. */
  toRef?: string;
  /** The same head in the spelling the endpoints take, via `headRefFor` — the
   *  one place the two vocabularies are allowed to meet. */
  headRef?: string;
  /** How the two sides are *named* — hash plus subject for a commit, a word
   *  for the working tree or the index (UI-139). Rendered here rather than on
   *  the far side because only this end holds the commit list to resolve them
   *  against, and because both surfaces then read identically. */
  fromLabel: RefLabel;
  toLabel: RefLabel;
  rows: {
    path: string;
    oldPath?: string;
    /** Git's own letter, as given. */
    status: string;
    /** The letter a reader sees — `U` where git said `A` and never followed
     *  the file. Sent rather than re-derived so the tree and the tab cannot
     *  disagree about which of the two a row is. */
    letter: string;
    /** The same distinction in words, for the tooltip. */
    phrase: string;
    additions: number;
    deletions: number;
    binary: boolean;
    untracked: boolean;
    agreement: { kind: string; label: string; hint: string };
  }[];
  onlyInGraph: string[];
  totals: { files: number; additions: number; deletions: number };
}

export function changedFilesPayload(
  reading: ChangedFilesReading | null,
  refs: { from_ref: string; to_ref: string } | null,
  names: { commits?: Commit[]; stashes?: Stash[] } = {},
): ChangedFilesPayload {
  return {
    fromLabel: refLabel(refs?.from_ref, names.commits, names.stashes),
    toLabel: refLabel(refs?.to_ref, names.commits, names.stashes),
    // Both, or neither. A row list labelled with the previous comparison's
    // refs is worse than no row list: every side it then fetches is fetched
    // against the wrong pair, and the diff that opens looks plausible.
    active: !!reading && !!refs,
    fromRef: refs?.from_ref,
    toRef: refs?.to_ref,
    headRef: headRefFor(refs?.to_ref) ?? undefined,
    rows: (reading?.rows ?? []).map((row) => ({
      path: row.file.path,
      oldPath: row.file.old_path,
      status: row.file.status,
      letter: statusLetter(row.file),
      phrase: statusPhrase(row.file),
      additions: row.file.additions,
      deletions: row.file.deletions,
      binary: row.file.binary,
      untracked: row.file.untracked,
      agreement: {
        kind: row.agreement.kind,
        label: agreementLabel(row.agreement),
        hint: agreementHint(row.agreement),
      },
    })),
    onlyInGraph: reading?.onlyInGraph ?? [],
    totals: reading?.totals ?? { files: 0, additions: 0, deletions: 0 },
  };
}

/**
 * The directory a row's name hangs off, VS Code's way: the basename reads at
 * full contrast and the folder trails it, dimmed.
 */
export function splitPath(path: string): { name: string; dir: string } {
  const i = path.lastIndexOf('/');
  return i < 0 ? { name: path, dir: '' } : { name: path.slice(i + 1), dir: path.slice(0, i) };
}
