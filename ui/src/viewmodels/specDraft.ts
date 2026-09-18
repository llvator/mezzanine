/**
 * Composing a new Elevator entity from whatever the reader is looking at —
 * UI-145.
 *
 * The moment this exists for is mid-review: a file scrolls past that you
 * don't recognise, you look for the spec entity that would explain it, and
 * `codeRefs.claimsFor` comes back empty. Until now the panel said nothing at
 * that point, which made the rational move to keep reading — the spec gets
 * written when writing it is cheap, and it is never cheaper than the minute
 * you spent working out what the file does.
 *
 * Everything here is pure: a target plus the loaded graph in, a draft and its
 * options out. The store owns the POST and the panels own the markup, so the
 * decisions worth arguing about — which parents are offerable, which file it
 * lands in, what name a path suggests — are decided in one place and asserted
 * in `ui/scripts/spec-draft.test.ts`.
 *
 * **The preview is a preview.** `previewSource` renders the same shape the
 * engine's `spec_write.rs` does, and the engine is what actually writes: the
 * response carries the source it wrote and that is what the panel shows
 * afterwards. Two renderers is a real cost, paid because a composer that
 * cannot show you the four lines it is about to add to a tracked file is
 * asking for a signature on an unread document.
 */

import type { D3Node, GraphData } from '../types/graph';

/** The Elevator kinds the composer creates. Mirrors `SpecKind` in
 *  `src/server/spec_write.rs` — UI Pages and Extensions are deliberately
 *  absent there, so offering them here would only produce 400s. */
export type SpecKindCode = 'c' | 'f' | 'fu' | 'concept';

export interface SpecKindInfo {
  code: SpecKindCode;
  /** What the reader calls it. */
  label: string;
  /** One line for the picker, in the language of the guide. */
  hint: string;
}

export const SPEC_KINDS: readonly SpecKindInfo[] = [
  { code: 'c', label: 'Category', hint: 'A top-level area of the system.' },
  { code: 'f', label: 'Feature', hint: 'Something the system does, filed under a Category.' },
  { code: 'fu', label: 'Functionality', hint: 'One verb of a Feature — its mechanism.' },
  { code: 'concept', label: 'Concept', hint: 'A cross-cutting idea several Features use.' },
];

/** The kind a parent must be, or null for a kind that stands alone.
 *  `f` → Category, `fu` → Feature; the same table as `SpecKind::parent_kind`. */
const PARENT_OF: Record<SpecKindCode, 'category' | 'feature' | null> = {
  c: null,
  f: 'category',
  fu: 'feature',
  concept: null,
};

/** A Functionality's name is *qualified by* its Feature, so there is no
 *  unparented one to write. Everything else may stand alone — an orphan
 *  Feature is a `--check` hint, not an error. */
export function requiresParent(kind: SpecKindCode): boolean {
  return kind === 'fu';
}

/** What the reader is documenting. One of the three things the panels can
 *  have under the pointer, plus the fourth case that has no code at all: an
 *  existing spec entity being given a child. */
export interface SpecTarget {
  grain: 'file' | 'folder' | 'entity' | 'spec';
  /** Root-relative, and for a folder without a trailing slash. Empty for a
   *  `spec` target, which claims nothing by default — a new Functionality
   *  does not claim the `.elv` its parent is written in. */
  path: string;
  /** Seeds the name field. Empty rather than absent when there is nothing to
   *  seed it with, so the parent's own name is not offered as the child's. */
  entityName?: string;
  /** What the composer calls the subject, when the path does not say it. */
  label?: string;
}

/** The `cr:` paths a target starts with.
 *
 * A folder gets a trailing slash because that is what makes the claim cover
 * the subtree — `cr: "ui/"` is how every folder-level claim in this repo's
 * own spec is written, and `cr: "ui"` would resolve to a folder named `ui`
 * and nothing inside it. An entity contributes its *file*, never a line
 * range: `cr:` anchors to paths, and a range would be drift the first time
 * anybody edited above it.
 */
export function defaultRefs(target: SpecTarget): string[] {
  const path = target.path.trim().replace(/\\/g, '/').replace(/\/+$/, '');
  if (!path) return [];
  return [target.grain === 'folder' ? `${path}/` : path];
}

/** Turn a path or entity name into something the lexer will read back as one
 *  identifier: `[A-Za-z_][A-Za-z0-9_]*`.
 *
 *  A suggestion, not a decision — it seeds the field so the common case is
 *  one keystroke away from correct, and the reader renames when the file's
 *  name isn't the concept's name. */
export function suggestName(target: SpecTarget): string {
  const raw = target.entityName ?? target.path.split('/').filter(Boolean).pop() ?? '';
  const stem = raw.replace(/\.[^.]+$/, '');
  const snake = stem
    .replace(/([a-z0-9])([A-Z])/g, '$1_$2')
    .replace(/[^A-Za-z0-9_]+/g, '_')
    .replace(/_+/g, '_')
    .replace(/^_+|_+$/g, '')
    .toLowerCase();
  if (!snake) return '';
  return /^[0-9]/.test(snake) ? `_${snake}` : snake;
}

/** Is this node an Elevator entity of the given kind?
 *
 *  The tag test matters: `kind` is a display string shared with code
 *  entities, and a Rust `Feature` is not a spec Feature. */
function isSpecKind(node: D3Node, kind: 'category' | 'feature'): boolean {
  return (node.tags ?? []).includes('elevator') && node.kind === kind;
}

export interface ParentOption {
  id: string;
  /** `visual_scopes` — the qualified name, which is what the id is built on. */
  qualifiedName: string;
  /** The `.elv` the parent is defined in, which is where the child lands. */
  file: string;
}

/**
 * The parents this kind may be filed under, in the order a reader scans.
 *
 * Read from the **full** graph, never the scoped one: the reader has narrowed
 * the canvas to the code they are reviewing, and the Category that should own
 * the new Feature is almost never inside that narrowing. A picker that
 * offered only what is on screen would offer nothing exactly when it matters.
 */
export function parentOptions(graph: GraphData | null, kind: SpecKindCode): ParentOption[] {
  const wanted = PARENT_OF[kind];
  if (!graph || !wanted) return [];
  return graph.nodes
    .filter((n) => isSpecKind(n, wanted))
    .map((n) => ({
      id: n.id,
      qualifiedName: n.qualified_name || n.name,
      file: n.file_path,
    }))
    .sort((a, b) => a.qualifiedName.localeCompare(b.qualifiedName));
}

/** Every `.elv` the analysis loaded, for the kinds that pick their own file.
 *  Sorted and deduped; the shortest path first is the usual `spec/` root. */
export function specFiles(graph: GraphData | null): string[] {
  if (!graph) return [];
  const files = new Set<string>();
  for (const node of graph.nodes) {
    if ((node.tags ?? []).includes('elevator') && node.file_path.endsWith('.elv')) {
      files.add(node.file_path);
    }
  }
  return [...files].sort();
}

/** What the composer holds while the reader is typing. */
export interface SpecDraft {
  kind: SpecKindCode;
  name: string;
  parentId: string | null;
  description: string;
  codeRefs: string[];
  /** Only consulted when the kind takes no parent. */
  file: string;
}

/** Everything wrong with the draft, in the order the form reads. Empty means
 *  it can be submitted. */
export function validateDraft(draft: SpecDraft): string[] {
  const problems: string[] = [];
  const name = draft.name.trim();
  if (!name) {
    problems.push('Give it a name.');
  } else if (name.includes('.')) {
    problems.push('Give the leaf name only — the parent is picked below.');
  } else if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(name)) {
    problems.push('Letters, digits and underscore only, not starting with a digit.');
  }
  if (requiresParent(draft.kind) && !draft.parentId) {
    problems.push('A Functionality is named by its Feature, so it needs one.');
  }
  if (!PARENT_OF[draft.kind] && !draft.file.trim()) {
    problems.push('Choose a .elv file to write into.');
  }
  if (!PARENT_OF[draft.kind] && draft.file.trim() && !draft.file.trim().endsWith('.elv')) {
    problems.push('The file has to be a .elv.');
  }
  return problems;
}

/** The file this draft will be written to — the parent's, when there is one.
 *
 * Writing a child beside its parent is what keeps the spec free of `import`
 * bookkeeping: a child reference to an entity in another file needs one, and
 * a composer that silently added imports would be editing two files per
 * click instead of one. */
export function targetFile(draft: SpecDraft, parents: ParentOption[]): string | null {
  const parent = parents.find((p) => p.id === draft.parentId);
  if (parent) return parent.file;
  if (PARENT_OF[draft.kind]) return draft.parentId ? null : draft.file.trim() || null;
  return draft.file.trim() || null;
}

/** A description as an `.elv` string literal — the TypeScript half of
 *  `spec_write.rs::quote`, and it has to agree with it exactly or the preview
 *  is a lie about what the file will contain.
 *
 *  A double quote is **replaced, not escaped**: the language has no escapes
 *  at all (`lexer.rs::string` ends the literal at the first `"` and reads a
 *  backslash as an ordinary character), so `\"` would stop the parse
 *  mid-sentence. Backslashes are left as typed, nothing consuming them. */
function quote(text: string): string {
  const folded = text.replace(/\s+/g, ' ').trim();
  return `"${folded.replace(/"/g, "'")}"`;
}

/**
 * The `.elv` source this draft becomes.
 *
 * A draft with neither a description nor a ref renders body-less — `f name`
 * alone, which the language calls a complete definition and the guide calls
 * the right first move. Nothing here invents prose for an empty field.
 */
export function previewSource(draft: SpecDraft, parents: ParentOption[]): string {
  const parent = parents.find((p) => p.id === draft.parentId);
  const name = draft.name.trim() || '…';
  const header =
    draft.kind === 'fu' && parent ? `fu f.${parent.qualifiedName}.${name}` : `${draft.kind} ${name}`;

  const body: string[] = [];
  const described = draft.description.trim();
  if (described) body.push(`    d: ${quote(described)}`);
  const refs = draft.codeRefs.map((p) => p.trim()).filter(Boolean);
  if (refs.length) body.push(`    cr: ${refs.map((p) => `"${p}"`).join(', ')}`);

  return body.length ? `${header} {\n${body.join('\n')}\n}` : header;
}

/** The line the composer adds *inside* the parent's body, shown so the reader
 *  can see that the click edits two places in the file. Bare, because inside
 *  `f X` a bare `fu Y` resolves to `X.Y`. */
export function previewChildRef(draft: SpecDraft): string | null {
  if (!draft.parentId || !PARENT_OF[draft.kind]) return null;
  return `${draft.kind} ${draft.name.trim() || '…'}`;
}

/** A fresh draft for a target — what the form opens with. */
export function draftFor(
  target: SpecTarget,
  kind: SpecKindCode,
  parents: ParentOption[],
  files: string[],
): SpecDraft {
  return {
    kind,
    name: suggestName(target),
    // Only pre-select a parent when there is exactly one it could be.
    // Guessing among several would file the entity somewhere plausible and
    // wrong, which is the one failure a spec cannot absorb.
    parentId: parents.length === 1 ? parents[0].id : null,
    description: '',
    codeRefs: defaultRefs(target),
    file: files[0] ?? '',
  };
}
