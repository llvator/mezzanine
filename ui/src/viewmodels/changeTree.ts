/**
 * The same change, folded into the folders it happened in (UI-154).
 *
 * The flat list answers "what did I touch"; it does not answer "where". A
 * fifty-row list with the folder trailing every name in a dimmed column makes
 * the reader reconstruct the shape of the change by scanning path prefixes,
 * which is work a tree does once and correctly. Two edits in `ui/src/stores`
 * and one in `src/parser` is a different change from three edits in
 * `ui/src/stores`, and only one of the two readings says so at a glance.
 *
 * Folders are *compacted* the way a source-control tree compacts them: a
 * folder whose only child is another folder is drawn as one row,
 * `ui/src/components`, rather than as three rows the reader has to expand in
 * turn. In this repo nearly every path is four or five deep and the first
 * three levels hold nothing but each other, so without compaction the tree
 * would spend most of its rows saying nothing.
 *
 * Pure and store-free for the reason `changedFiles.ts` and `changeFacets.ts`
 * are: the grouping is a claim about the change — every file appears exactly
 * once, and a folder's counts are its subtree's counts — and a claim has to be
 * checkable without a browser.
 */

import type { ChangedFileRow } from './changedFiles.ts';

/** What a folder row reports about everything beneath it. */
export interface ChangeTotals {
  files: number;
  additions: number;
  deletions: number;
}

export interface ChangeTreeFile {
  kind: 'file';
  /** Repo-relative path, exactly as the row carries it — the key the canvas,
   *  the diff and the highlight are all keyed by. */
  path: string;
  /** The basename, which is all the row draws: the folder is a row above it. */
  name: string;
  row: ChangedFileRow;
}

export interface ChangeTreeFolder {
  kind: 'folder';
  /**
   * The full repo-relative path of the folder this row *ends* at.
   *
   * After compaction the label can name several levels and this names the
   * deepest of them, which is the one a click means: `ui/src/components`
   * scopes the graph to the components folder, not to `ui`.
   */
  path: string;
  /** What the row reads as — one segment, or the chain compaction merged. */
  label: string;
  children: ChangeTreeNode[];
  totals: ChangeTotals;
}

export type ChangeTreeNode = ChangeTreeFolder | ChangeTreeFile;

/** The mutable shape the walk builds before anything is sorted or compacted. */
interface Draft {
  folders: Map<string, Draft>;
  files: ChangeTreeFile[];
}

/**
 * Fold rows into a tree, in the order they should be drawn.
 *
 * Takes the rows the pane is *showing*, not every row in the comparison: the
 * facet chips narrow the list, and a tree that kept counting hidden files
 * would put a `12 files` badge on a folder showing three.
 */
export function buildChangeTree(rows: readonly ChangedFileRow[]): ChangeTreeNode[] {
  const root: Draft = { folders: new Map(), files: [] };
  for (const row of rows) {
    const parts = row.file.path.split('/');
    // A path with no separator is a file at the repo root, and `pop` on a
    // one-element array leaves the loop below with nothing to walk.
    const name = parts.pop() ?? row.file.path;
    let at = root;
    for (const part of parts) {
      let next = at.folders.get(part);
      if (!next) {
        next = { folders: new Map(), files: [] };
        at.folders.set(part, next);
      }
      at = next;
    }
    at.files.push({ kind: 'file', path: row.file.path, name, row });
  }
  return childrenOf(root, '');
}

/** Folders first, then files, each alphabetical — the order `FileTreeNode`
 *  draws the scope tree in, so the two trees in this app agree about where a
 *  row will be. */
function childrenOf(draft: Draft, prefix: string): ChangeTreeNode[] {
  const folders = [...draft.folders]
    .map(([name, child]) => compact(folderAt(name, child, prefix ? `${prefix}/${name}` : name)))
    .sort((a, b) => a.label.localeCompare(b.label));
  const files = [...draft.files].sort((a, b) => a.name.localeCompare(b.name));
  return [...folders, ...files];
}

function folderAt(name: string, draft: Draft, path: string): ChangeTreeFolder {
  const children = childrenOf(draft, path);
  return { kind: 'folder', path, label: name, children, totals: totalsOf(children) };
}

/**
 * Merge a folder with an only child that is itself a folder.
 *
 * The child's `path` and `totals` are the merged row's: they describe the same
 * set of files, since the only way down from here is through it. Only the
 * label grows, and it grows with a separator so the row still reads as the
 * path it is.
 *
 * Children are compacted on the way up, so one step would do; the loop costs
 * nothing and does not depend on that staying true.
 */
function compact(folder: ChangeTreeFolder): ChangeTreeFolder {
  let at = folder;
  while (at.children.length === 1 && at.children[0].kind === 'folder') {
    const only = at.children[0];
    at = { ...only, label: `${at.label}/${only.label}` };
  }
  return at;
}

/** A folder's counts are its subtree's counts, summed from children that have
 *  already been given theirs — so nothing is counted twice however deep the
 *  change goes. */
function totalsOf(children: readonly ChangeTreeNode[]): ChangeTotals {
  const totals: ChangeTotals = { files: 0, additions: 0, deletions: 0 };
  for (const child of children) {
    if (child.kind === 'folder') {
      totals.files += child.totals.files;
      totals.additions += child.totals.additions;
      totals.deletions += child.totals.deletions;
    } else {
      totals.files += 1;
      totals.additions += child.row.file.additions;
      totals.deletions += child.row.file.deletions;
    }
  }
  return totals;
}
