/**
 * What the Quality panel is measuring — the rules, apart from the stores.
 *
 * The panel names a *population* ("whole analysis scope", "what the canvas is
 * drawing", "the current selection") and every number in it — the repo banner,
 * the summary counts, the scatters, the entity leaderboard, the file and
 * folder rollups — has to be that same population, or the reader is comparing
 * a summary of one set of files against a table of another. These functions
 * are the rules that decide membership; `stores/quality.ts` wires them to the
 * stores and re-exports them.
 *
 * Kept pure and store-free so they can be unit-tested (`npm run
 * test:quality-population`) without dragging in `displayPlan`, `scope`, and
 * the VS Code adapter behind them.
 */

import type { D3Node, GraphData } from '../types/graph';
import type { DiffData } from '../stores/diff';

/**
 * Does `filePath` belong to the rollup at `scopePath`?
 *
 * The one membership rule, shared by every consumer, and it mirrors the
 * engine's `is_descendant`: a file rollup holds exactly its own file, a folder
 * rollup holds its whole subtree, and the root folder (`''`) holds everything.
 * Written once because the answer decides which rows a population lists, and
 * two copies of it would drift into two different populations.
 */
export function scopeHolds(scopePath: string, isFolder: boolean, filePath: string): boolean {
  if (!isFolder) return filePath === scopePath;
  if (scopePath === '') return true;
  return filePath.startsWith(`${scopePath}/`);
}

/** Every directory above a file, root (`''`) included. */
export function ancestorDirs(filePath: string): string[] {
  const parts = filePath.split('/');
  parts.pop();
  const dirs = [''];
  let cur = '';
  for (const part of parts) {
    cur = cur ? `${cur}/${part}` : part;
    dirs.push(cur);
  }
  return dirs;
}

/**
 * Cut a graph's file/folder rollups down to the population its nodes describe.
 *
 * What narrows is which rows are *listed*, never the numbers on them. The
 * rollup fields (cohesion, fan-out, entity count) are computed by the engine
 * over the whole file or directory, and recomputing them from a filtered slice
 * would answer a different question — cohesion over half a file is not a lower
 * cohesion, it is nothing. A file the population no longer holds is not a file
 * that got better; it is a file the reader is not looking at, so it goes.
 *
 * This is what kept the Files and Folders tabs out of step with the rest of
 * the panel: they read `graphData` directly, so the population selector moved
 * the summary and the entity table and left them showing the visual scope.
 */
export function narrowRollups(g: GraphData, nodes: D3Node[] = g.nodes): GraphData {
  const files = new Set<string>();
  const dirs = new Set<string>();
  for (const n of nodes) {
    if (!n.file_path) continue;   // ghosts carry no path and roll up nowhere
    files.add(n.file_path);
    for (const dir of ancestorDirs(n.file_path)) dirs.add(dir);
  }
  return {
    ...g,
    nodes,
    files: (g.files ?? []).filter((f) => files.has(f.path)),
    folders: (g.folders ?? []).filter((m) => dirs.has(m.path)),
  };
}

/**
 * Children of `n` in the containment tree.
 *
 * `parent_id` normally holds the parent's `original_id`, but Rust impl blocks
 * record the bare type name instead, so a name key counts too — and only
 * within the same file, because a bare name is not unique across a repo and
 * over-collecting silently inflates the population the metrics describe.
 */
function childrenOf(n: D3Node, byParent: Map<string, D3Node[]>): D3Node[] {
  const byId = byParent.get(n.original_id) ?? [];
  if (n.name === n.original_id) return byId;
  const byName = (byParent.get(n.name) ?? []).filter((c) => c.file_path === n.file_path);
  return [...byId, ...byName];
}

/**
 * What a selected node stands for as a population.
 *
 * A File or Folder node names a path and stands for everything under it. Any
 * other entity stands for itself *and what it contains*, so selecting a struct
 * measures its methods rather than reporting a single row and calling it a
 * scope. Selecting a leaf function does report one row, which is the honest
 * answer to "how does the thing I selected score".
 */
export function selectionPopulation(all: D3Node[], sel: D3Node | null): D3Node[] {
  if (!sel) return [];
  if (sel.kind_raw === 'File' || sel.kind_raw === 'Folder') {
    const isFolder = sel.kind_raw === 'Folder';
    return all.filter((n) => scopeHolds(sel.original_id, isFolder, n.file_path));
  }
  const byParent = new Map<string, D3Node[]>();
  for (const n of all) {
    if (!n.parent_id) continue;
    const bucket = byParent.get(n.parent_id);
    if (bucket) bucket.push(n);
    else byParent.set(n.parent_id, [n]);
  }
  // The selection may be a collapsed copy of an entity in `all`; walk from
  // whichever object `all` actually holds so the subtree is found.
  const root = all.find((n) => n.id === sel.id) ?? sel;
  const out: D3Node[] = [];
  const seen = new Set<string>();
  const queue: D3Node[] = [root];
  while (queue.length > 0) {
    const n = queue.pop()!;
    if (seen.has(n.id)) continue;
    seen.add(n.id);
    out.push(n);
    queue.push(...childrenOf(n, byParent));
  }
  return out;
}

/** Files the diff reports a "core" change in — added, removed, or modified
 *  with the source actually different. Mirrors what `scopeToChangedFiles`
 *  scopes to, so the two agree on what "changed" means. */
export function changedFilePaths(diff: DiffData): Set<string> {
  return new Set(
    diff.entities
      .filter((e) =>
        e.status === 'added' || e.status === 'removed'
        || (e.status === 'modified' && e.source_changed === true)
      )
      .map((e) => e.file_path)
      .filter((p): p is string => typeof p === 'string' && p.length > 0),
  );
}
