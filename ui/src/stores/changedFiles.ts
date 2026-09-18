/**
 * The change as git reports it, beside the change as the graph reports it
 * (UI-134).
 *
 * The list follows `diffData`: loading a diff fetches it, a live refresh
 * refetches it — the working tree moves under a `→ working` comparison, and a
 * file list that did not follow would be describing the previous save —
 * and leaving diff mode clears it. That subscription is the single trigger, so
 * no caller has to remember to keep the two in step.
 */

import { writable, derived, get } from 'svelte/store';
import type { Readable } from 'svelte/store';
import { apiUrl } from '../vscodeAdapter';
import { isServeMode } from './serveMode';
import { diffData, baseDetailsCache } from './diff';
import { graphData, focusNode, selectedNode } from './graph';
import { filterOnChangeClick } from './panes';
import type { D3Node } from '../types/graph';
import { detailsMap } from './details';
import { normalizeScopePath } from '../viewmodels/diffRollup';
import { holdersOf, litPaths } from '../viewmodels/changeHighlight';
import {
  headRefFor, isSourceEdit, readChangedFiles,
  type ChangedFile, type ChangedFilesReading,
} from '../viewmodels/changedFiles';

export type { ChangedFile };

/** Git's rows for the loaded comparison. `null` before the first answer. */
export const changedFiles = writable<ChangedFile[] | null>(null);

/** Why the list is missing, when it is missing for a reason worth showing. */
export const changedFilesError = writable<string | null>(null);

/** Which row the reader opened. Cleared when the comparison changes. */
export const selectedChangedFile = writable<ChangedFile | null>(null);

/**
 * The path of the row under the pointer, or `null`.
 *
 * Pointer state in a store rather than in the component because what it moves
 * is on the *canvas* — the same reason `specHoverId` lives beside the spec
 * pane's filter and not inside it. Not persisted and not mirrored: a hover is
 * a question being asked right now by the hand that is moving.
 */
export const hoveredChangedFile = writable<string | null>(null);

/** The rows lighting the canvas: the one open plus the one under the pointer.
 *  See `litPaths` for why it is a union rather than a replacement. */
const litChangedPaths: Readable<string[]> = derived(
  [selectedChangedFile, hoveredChangedFile],
  ([$open, $hover]) => litPaths($open?.path ?? null, $hover),
);

/**
 * What the canvas rings for the Changes pane: ids, resolved against
 * `graphData`.
 *
 * `graphData` and not `rawEntityGraph`, for the reason `specClaimHighlightIds`
 * gives: the canvas puts a class on a node it *has*, and above entity level the
 * node it has is a rollup whose id `collapseGraph` minted. Matching on
 * `file_path` is what lets one path answer for the entity, its file circle and
 * the folder circle above it.
 *
 * Resolved to ids here rather than handing the canvas the paths, so the
 * per-node path arithmetic stays out of the render loop — where it would rerun
 * on every plan apply for an answer that only changes when the pointer does.
 */
export const changedFileLitIds: Readable<Set<string>> = derived(
  [litChangedPaths, graphData],
  ([$paths, $data]) => holdersOf($paths, $data.nodes),
);

/**
 * Which kinds of change the list is narrowed to (UI-149). Empty is no filter.
 *
 * Letters as `statusLetter` spells them — `U` for untracked — so the selection
 * is in the same vocabulary as the chips and the rows.
 *
 * It survives a live refresh and not a new comparison; see the subscription at
 * the foot of this file for why those two have to be told apart.
 */
export const changeFilter = writable<Set<string>>(new Set());

/**
 * Which folders the reader has *closed* in the tree view (UI-154). Absent means
 * open.
 *
 * Stored as the exception rather than as the selection, so a tree opens
 * showing the whole change: a comparison is a bounded thing a reviewer wants
 * to see all of, and a tree that opened collapsed would make them expand their
 * way to a list they already had. Recording closures instead of openings also
 * survives the working tree moving under a `→ working` comparison — a folder
 * that gains a file keeps the state its neighbours have, rather than appearing
 * shut because nobody had opened it yet.
 *
 * Cleared with the facet filter, and on the same rule: a new comparison is a
 * different change, and folders collapsed in the old one mean nothing in it.
 */
export const collapsedChangeFolders = writable<Set<string>>(new Set());

/** Open or close one folder, as a new set — a set mutated in place is a store
 *  that does not notify. */
export function toggleChangeFolder(path: string): void {
  collapsedChangeFolders.update((s) => {
    const next = new Set(s);
    if (!next.delete(path)) next.add(path);
    return next;
  });
}

/**
 * The canvas node drawing each file, by repo-relative path.
 *
 * A File node carries its scope path where an entity carries an id, which is
 * the same lookup `changeOf` makes and the reason both can be keyed by path
 * without colliding. Derived once here rather than rebuilt per row: the list
 * and the tree both need it, and a map built inside a row component is a walk
 * of every node in the graph per changed file.
 */
export const changedFileNodes: Readable<Map<string, D3Node>> = derived(graphData, ($data) =>
  new Map(
    $data.nodes
      .filter((n) => n.kind_raw === 'File')
      .map((n) => [normalizeScopePath(n.original_id), n] as const),
  ),
);

/**
 * Open a file: read it in the Details pane, and point the canvas at it.
 *
 * The diff is opened either way — that is what the row is for. What
 * `filterOnChangeClick` decides is whether the canvas is also *re-rooted* on
 * the file, which is what selecting a node does. Off, the pointing is done by
 * the light, which hides nothing: the row stays marked, so the file's circles
 * stay ringed after the pointer has moved on.
 *
 * Here rather than in the component because the flat list and the tree are two
 * components drawing the same row, and a click policy kept in one of them is a
 * policy the other can quietly disagree with.
 */
export function openChangedFile(file: ChangedFile): void {
  selectedChangedFile.set(file);
  if (!get(filterOnChangeClick)) return;
  const node = get(changedFileNodes).get(file.path);
  if (node) focusNode(node);
}

/** One side of a file, as `POST /api/file-diff` reports it. */
export interface FileSide {
  text: string;
  binary: boolean;
  truncated: boolean;
}

export interface FileDiffContent {
  base?: FileSide;
  head?: FileSide;
  binary: boolean;
}

/**
 * Every path the analysis loaded, from whichever detail sidecars have arrived.
 *
 * Both sides, and that is the point: a *deleted* file is absent from the head
 * sidecar because it is gone, and reading that absence as "not analysed" would
 * put the wrong verdict on every deletion. The base sidecar still holds it.
 *
 * File entries are keyed by repo-relative path and entity entries by an id
 * carrying `:line:name`, so the two cannot collide — the same property
 * `changeOf` relies on. Ids are kept out anyway: a path never contains `:`
 * in any tree this reads.
 */
const analysedPaths: Readable<Set<string> | null> = derived(
  [detailsMap, baseDetailsCache],
  ([$head, $base]) => {
    if (!$head && !$base) return null;
    const paths = new Set<string>();
    for (const map of [$head, $base]) {
      for (const key of Object.keys(map ?? {})) {
        if (!key.includes(':')) paths.add(key);
      }
    }
    return paths;
  },
);

/**
 * Entities changed at source level, per file — the population the ladder's
 * narrowest rung seeds from.
 *
 * `isSourceEdit` and not "any row the diff mentions": see the rule there for
 * why the ripple has to be left out, and what including it did to the
 * disagreement list.
 */
const changedEntitiesByFile: Readable<Map<string, number>> = derived(diffData, ($d) => {
  const counts = new Map<string, number>();
  if (!$d) return counts;
  for (const e of $d.entities) {
    if (!e.file_path || !isSourceEdit(e)) continue;
    // The base side was analysed in a throwaway worktree, so its rows carry a
    // temp path in front of every file. Normalised here for the same reason
    // every id is normalised on the way in: the two sides have to be keyed
    // alike before anything can be joined on them.
    const path = normalizeScopePath(e.file_path);
    counts.set(path, (counts.get(path) ?? 0) + 1);
  }
  return counts;
});

/** The join the pane draws. `null` until git has answered. */
export const changedFilesReading: Readable<ChangedFilesReading | null> = derived(
  [changedFiles, changedEntitiesByFile, analysedPaths],
  ([$files, $entities, $analysed]) =>
    $files === null
      ? null
      : readChangedFiles($files, { changedEntities: $entities, analysed: $analysed }),
);

/** The ref pair the loaded diff describes, in the spelling the endpoints take. */
function refsForLoadedDiff(): { from_ref: string; to_ref: string } | null {
  const d = get(diffData);
  const to = headRefFor(d?.to_ref);
  if (!d || !to) return null;
  return { from_ref: d.from_ref, to_ref: to };
}

/** Fetch git's list for the loaded comparison. */
export async function loadChangedFiles(): Promise<void> {
  // Serve mode has no diff endpoints at all (SRV-003), so there is no
  // comparison here to list files for.
  if (isServeMode()) return;
  const refs = refsForLoadedDiff();
  if (!refs) {
    changedFiles.set(null);
    return;
  }
  try {
    const resp = await fetch(apiUrl('/api/changed-files'), {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(refs),
    });
    if (!resp.ok) throw new Error((await resp.text()) || `HTTP ${resp.status}`);
    changedFiles.set(await resp.json());
    changedFilesError.set(null);
  } catch (err) {
    changedFiles.set(null);
    changedFilesError.set(`Could not list the changed files: ${err}`);
  }
}

/**
 * Both sides of one file, from git rather than from the detail sidecar.
 *
 * The sidecar would have been free — it is already in the browser and already
 * keyed by path — and it is the wrong source twice over: it holds only
 * analysed files, which excludes every row this pane exists to explain, and it
 * truncates at 256 KB without saying so. Asking git keeps the pane a check on
 * the analysis rather than another view of it.
 */
export async function loadFileDiff(file: ChangedFile): Promise<FileDiffContent> {
  const refs = refsForLoadedDiff();
  if (!refs) throw new Error('No diff is loaded.');
  const resp = await fetch(apiUrl('/api/file-diff'), {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ ...refs, path: file.path, base_path: file.old_path }),
  });
  if (!resp.ok) throw new Error((await resp.text()) || `HTTP ${resp.status}`);
  return resp.json();
}

/**
 * Selecting a node replaces whatever file the Changes tab had open — unless it
 * is the node that file *is*.
 *
 * The row pins its own file node when the canvas has one, so the two selections
 * are deliberately in step in that case and clearing on any selection at all
 * would undo the click that made it. Comparing the paths rather than ordering
 * the two `set` calls keeps that true however the node was selected: from the
 * canvas, from a search result, from a relationship row.
 */
selectedNode.subscribe((n) => {
  const file = get(selectedChangedFile);
  if (!file || !n) return;
  const path = n.kind_raw === 'File' ? normalizeScopePath(n.original_id) : n.file_path;
  if (path !== file.path) selectedChangedFile.set(null);
});

/**
 * Keep the list in step with the comparison.
 *
 * A module-level subscription rather than a call at each `loadDiff` site: the
 * list is a function of the loaded diff, and three call sites that each have
 * to remember it is three places for it to go stale — which for a working-tree
 * head means silently describing the previous save.
 *
 * The facet filter is cleared on a *new comparison* and not on a refresh of
 * the one already loaded, which is why the pair is remembered here. Under
 * `→ working` this subscription fires on every save; a filter dropped there
 * would be a filter the reader has to re-apply each time they touch a file,
 * which is precisely when they are reading the list. Changing what is being
 * compared is the other thing entirely — the letters mean a different change.
 */
let loadedPair: string | null = null;

diffData.subscribe(($d) => {
  if (!$d) {
    changedFiles.set(null);
    changedFilesError.set(null);
    selectedChangedFile.set(null);
    // The pointer is not over a list that no longer exists, and a hover left
    // behind here would ring a circle with nothing on screen explaining it.
    hoveredChangedFile.set(null);
    changeFilter.set(new Set());
    collapsedChangeFolders.set(new Set());
    loadedPair = null;
    return;
  }
  const pair = `${$d.from_ref}→${$d.to_ref}`;
  if (pair !== loadedPair) {
    if (loadedPair !== null) {
      changeFilter.set(new Set());
      collapsedChangeFolders.set(new Set());
    }
    loadedPair = pair;
  }
  void loadChangedFiles();
});
