/**
 * Folding the Changes list into the folders the change happened in.
 *
 * Four claims carry the view, and each one is something a reader would
 * otherwise discover by being misled: every file appears exactly once and
 * under its own path, a folder's counts are its whole subtree's, a chain of
 * single-child folders is one row, and the row a click scopes on is the
 * deepest folder that row names rather than the first.
 *
 * Run: node --experimental-strip-types --test scripts/change-tree.test.ts
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { buildChangeTree } from '../src/viewmodels/changeTree.ts';
import type { ChangeTreeFolder, ChangeTreeNode } from '../src/viewmodels/changeTree.ts';
import type { ChangedFile, ChangedFileRow } from '../src/viewmodels/changedFiles.ts';

const row = (path: string, over: Partial<ChangedFile> = {}): ChangedFileRow => ({
  file: {
    status: 'M',
    path,
    additions: 3,
    deletions: 1,
    binary: false,
    untracked: false,
    ...over,
  },
  agreement: { kind: 'silent' },
});

/** An ordinary change in this repo: two panes, a store, a parser, and a note
 *  at the root. Deep paths whose first levels hold nothing but each other —
 *  the shape compaction exists for. */
const ROWS = [
  row('ui/src/components/ChangedFiles.svelte'),
  row('ui/src/components/ChangesBar.svelte', { additions: 10, deletions: 0 }),
  row('ui/src/stores/panes.ts', { additions: 4, deletions: 2 }),
  row('src/parser/rust/calls.rs', { status: 'A', additions: 40, deletions: 0 }),
  row('README.md'),
];

/** The folder at `path`, anywhere in the tree. */
function folder(nodes: ChangeTreeNode[], path: string): ChangeTreeFolder {
  for (const node of nodes) {
    if (node.kind !== 'folder') continue;
    if (node.path === path) return node;
    const found = tryFolder(node.children, path);
    if (found) return found;
  }
  throw new Error(`no folder ${path} in the tree`);
}

function tryFolder(nodes: ChangeTreeNode[], path: string): ChangeTreeFolder | null {
  for (const node of nodes) {
    if (node.kind !== 'folder') continue;
    if (node.path === path) return node;
    const found = tryFolder(node.children, path);
    if (found) return found;
  }
  return null;
}

/** Every file the tree draws, wherever it sits. */
function files(nodes: ChangeTreeNode[]): string[] {
  return nodes.flatMap((node) =>
    node.kind === 'folder' ? files(node.children) : [node.path],
  );
}

test('every row appears once, at its own path', () => {
  // The grouping is a re-arrangement of the list and nothing else. A file
  // dropped here reads as a file git did not report, which is the one thing
  // this pane exists to be trusted about.
  const tree = buildChangeTree(ROWS);
  assert.deepEqual(
    files(tree).sort(),
    ROWS.map((r) => r.file.path).sort(),
  );
});

test('a chain of single-child folders is one row, named for its deepest end', () => {
  const tree = buildChangeTree(ROWS);
  // `src` holds only `parser`, which holds only `rust`, which holds the file.
  const compacted = tree.find(
    (n): n is ChangeTreeFolder => n.kind === 'folder' && n.label === 'src/parser/rust',
  );
  assert.ok(compacted, 'the src chain should be drawn as one row');
  // The label names three levels; the path names the one a click means.
  assert.equal(compacted.path, 'src/parser/rust');
  assert.equal(compacted.children.length, 1);
});

test('a folder with two children keeps its own row', () => {
  // `ui/src` branches into `components` and `stores`, so the compaction stops
  // there — merging it would put two subtrees under one label that names only
  // one path.
  const tree = buildChangeTree(ROWS);
  const ui = tree.find((n): n is ChangeTreeFolder => n.kind === 'folder' && n.path === 'ui/src');
  assert.ok(ui, 'ui/src should be a row of its own');
  assert.equal(ui.label, 'ui/src');
  assert.deepEqual(
    ui.children.map((c) => (c.kind === 'folder' ? c.label : c.name)),
    ['components', 'stores'],
  );
});

test("a folder's counts are its whole subtree's", () => {
  const tree = buildChangeTree(ROWS);
  // Two files under components (3+10 added, 1+0 removed) and one under
  // stores (4 added, 2 removed).
  assert.deepEqual(folder(tree, 'ui/src').totals, { files: 3, additions: 17, deletions: 3 });
  assert.deepEqual(
    folder(tree, 'ui/src/components').totals,
    { files: 2, additions: 13, deletions: 1 },
  );
  assert.deepEqual(
    folder(tree, 'src/parser/rust').totals,
    { files: 1, additions: 40, deletions: 0 },
  );
});

test('folders sort before files, each alphabetically', () => {
  // The order `FileTreeNode` draws the scope tree in. A root-level file
  // sorting into the middle of the folders would put `README.md` between
  // `src` and `ui`, where nobody looks for it.
  const tree = buildChangeTree(ROWS);
  assert.deepEqual(
    tree.map((n) => (n.kind === 'folder' ? n.label : n.name)),
    ['src/parser/rust', 'ui/src', 'README.md'],
  );
});

test('a file at the repo root is a top-level row, not a folder', () => {
  const tree = buildChangeTree([row('Cargo.toml')]);
  assert.equal(tree.length, 1);
  assert.equal(tree[0].kind, 'file');
  assert.equal(tree[0].kind === 'file' && tree[0].name, 'Cargo.toml');
});

test('an empty change is an empty tree', () => {
  // Reached whenever the facet chips filter everything out, which is a state
  // the chips deliberately allow.
  assert.deepEqual(buildChangeTree([]), []);
});

test('the tree describes the rows it is given, not the whole comparison', () => {
  // The facet chips hand this the shown rows. A folder counting the hidden
  // ones would badge `2 files` over a single visible row.
  const shown = ROWS.filter((r) => r.file.status === 'A');
  const tree = buildChangeTree(shown);
  assert.deepEqual(folder(tree, 'src/parser/rust').totals.files, 1);
  assert.equal(files(tree).length, 1);
});
