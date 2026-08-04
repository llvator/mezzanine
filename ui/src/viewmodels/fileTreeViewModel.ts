/**
 * FileTreeViewModel — view-state and actions owned on behalf of
 * `FileTree.svelte`. Keeps the "which folders are expanded" and "what's the
 * search text" state out of the component, and provides a single derived
 * `tree` shape instead of letting the template rebuild it every render.
 *
 * Bridges to the Model/filter layer through FilterViewModel so FileTree
 * never imports `stores/graph.ts` directly — one less seam to worry about.
 */

import { writable, derived, get } from 'svelte/store';
import type { Readable } from 'svelte/store';
import { graphData } from '../stores/graph';
import { visibleFiles, hiddenFiles, setVisibleFiles, toggleFile } from './filterViewModel';
import { parseQuery, matchesQuery } from '../utils/fuzzyPath';

export { visibleFiles, toggleFile };

// --- View-owned UI state ---
export const filterText = writable<string>('');
export const openFolders = writable<Set<string>>(new Set());

export function setFilterText(text: string): void {
  filterText.set(text);
}

export function toggleFolderOpen(path: string): void {
  openFolders.update((s) => {
    const ns = new Set(s);
    if (ns.has(path)) ns.delete(path);
    else ns.add(path);
    return ns;
  });
}

// --- Tree shape ---
export interface TreeNode {
  [key: string]: TreeNode | null;
}

function buildTree(files: string[]): TreeNode {
  const tree: TreeNode = {};
  for (const filePath of files) {
    const parts = filePath.split('/').filter((p) => p.length > 0);
    let current = tree;
    parts.forEach((part, i) => {
      if (!current[part]) {
        current[part] = i === parts.length - 1 ? null : {};
      }
      if (i < parts.length - 1) {
        if (current[part] === null) current[part] = {};
        current = current[part] as TreeNode;
      }
    });
  }
  return tree;
}

export const allFiles: Readable<string[]> = derived(graphData, ($data) =>
  [...new Set($data.nodes.map((n) => n.file_path))].sort(),
);

/** True while the user has typed into the panel's own filter box. */
export const queryActive: Readable<boolean> = derived(
  filterText,
  ($f) => $f.trim().length > 0,
);

/**
 * Files the query keeps — every file when there is no query.
 *
 * The tree is then built from *these* rather than from everything, which is
 * what fixes the old filtering behaviour: a folder used to render only when
 * the folder's own path matched, so typing a filename hid the folder
 * containing it and the file with it. Deriving the tree from the surviving
 * files makes an ancestor's presence a consequence of its descendants
 * instead of a separate test that could disagree with them.
 */
export const matchedFiles: Readable<string[]> = derived(
  [allFiles, filterText],
  ([$files, $filter]) => {
    const terms = parseQuery($filter);
    if (terms.length === 0) return $files;
    return $files.filter((f) => matchesQuery(terms, f));
  },
);

export const tree: Readable<TreeNode> = derived(matchedFiles, ($files) => buildTree($files));

/** Entity count per file, built once per dataset. The per-row helper it
 *  replaces scanned every node in the graph, which was survivable while only
 *  two levels of the tree rendered and is not now that all of them do. */
export const entityCounts: Readable<Map<string, number>> = derived(graphData, ($data) => {
  const counts = new Map<string, number>();
  for (const n of $data.nodes) {
    counts.set(n.file_path, (counts.get(n.file_path) ?? 0) + 1);
  }
  return counts;
});

export const allSelected: Readable<boolean> = derived(
  [matchedFiles, visibleFiles],
  ([$files, $visible]) => $files.length > 0 && $files.every((f) => $visible.has(f)),
);

// --- Actions ---
export function getFilesInTree(tree: TreeNode, basePath: string): string[] {
  const files: string[] = [];
  for (const key of Object.keys(tree)) {
    const fullPath = basePath ? basePath + '/' + key : key;
    if (tree[key] === null) {
      files.push(fullPath);
    } else {
      files.push(...getFilesInTree(tree[key] as TreeNode, fullPath));
    }
  }
  return files;
}

export type FolderState = 'all' | 'some' | 'none';

/** How much of a folder is visible, for the checked / indeterminate /
 *  unchecked box. The box used to be a hardcoded `checked` with no binding
 *  at all, so it claimed every folder was fully shown no matter what. */
export function folderState(subTree: TreeNode, basePath: string, hidden: Set<string>): FolderState {
  const files = getFilesInTree(subTree, basePath);
  if (files.length === 0) return 'all';
  let shown = 0;
  for (const f of files) if (!hidden.has(f)) shown++;
  if (shown === files.length) return 'all';
  if (shown === 0) return 'none';
  return 'some';
}

export function toggleFolder(subTree: TreeNode, basePath: string, checked: boolean): void {
  const files = getFilesInTree(subTree, basePath);
  hiddenFiles.update((s) => {
    const ns = new Set(s);
    for (const f of files) {
      if (checked) ns.delete(f);
      else ns.add(f);
    }
    return ns;
  });
}

/** All / None over what the query left, not over the whole dataset — "None"
 *  after typing a filter used to clear far more than was on screen. */
export function toggleAll(): void {
  const shown = get(matchedFiles);
  if (get(allSelected)) {
    hiddenFiles.update((s) => new Set([...s, ...shown]));
  } else {
    hiddenFiles.update((s) => new Set([...s].filter((f) => !shown.includes(f))));
  }
}

export { setVisibleFiles };
