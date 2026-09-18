/**
 * The spec composer's decisions — UI-145.
 *
 * The click writes to a tracked `.elv`, so the questions worth asserting are
 * the ones a reader cannot check by looking at the form: which parents are
 * offerable at all, which file the write lands in, and whether the four lines
 * previewed are the four lines the engine will write. Clicking through the
 * panel tells you the field is filled; it does not tell you the Category the
 * picker offered is a *spec* Category rather than a Rust struct that happens
 * to be called one.
 *
 *   npm run test:specdraft
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  SPEC_KINDS,
  defaultRefs,
  draftFor,
  parentOptions,
  previewChildRef,
  previewSource,
  requiresParent,
  specFiles,
  suggestName,
  targetFile,
  validateDraft,
  type ParentOption,
  type SpecDraft,
} from '../src/viewmodels/specDraft.ts';
import type { D3Node, GraphData } from '../src/types/graph.ts';

function node(
  id: string,
  kind: string,
  tags: string[],
  file_path: string,
  qualified_name = id,
): D3Node {
  return {
    id, original_id: id, name: id, qualified_name,
    kind, kind_raw: kind, file_path, line: 1,
    visibility: 'Public', parent_id: null, parameters: [], return_type: null,
    extends: [], implements: [], tags, source_code: null, fields: [],
    impl_blocks: [], language: 'Elevator',
  } as D3Node;
}

const SPEC = ['elevator'];

/** A spec with two Categories, one Feature, and a Rust struct that would be
 *  mistaken for a Category by anything reading `kind` alone.
 *
 *  The ids and qualified names are the real shapes `emit.rs` produces —
 *  `elevator::f.visual_scopes` addresses the entity, `visual_scopes` is the
 *  name the spec is written in — because the composer uses one for the
 *  request and the other for the source it renders. */
function graph(): GraphData {
  return {
    nodes: [
      node('elevator::c.server', 'category', SPEC, 'spec/server.elv', 'server'),
      node('elevator::c.visualizer', 'category', SPEC, 'spec/visualizer.elv', 'visualizer'),
      node('elevator::f.visual_scopes', 'feature', SPEC, 'spec/visualizer.elv', 'visual_scopes'),
      node('src/lib.rs:9:Category', 'category', ['rust'], 'src/lib.rs'),
      node('src/lib.rs:20:Feature', 'feature', ['rust'], 'src/lib.rs'),
    ],
    links: [],
  } as GraphData;
}

// ─── What a target suggests ────────────────────────────────────────────

test('a folder ref carries the trailing slash that makes it cover the subtree', () => {
  assert.deepEqual(defaultRefs({ grain: 'folder', path: 'ui/src' }), ['ui/src/']);
  // Already-slashed, and Windows separators, land on the same string.
  assert.deepEqual(defaultRefs({ grain: 'folder', path: 'ui/src/' }), ['ui/src/']);
  assert.deepEqual(defaultRefs({ grain: 'folder', path: 'ui\\src' }), ['ui/src/']);
});

test('a file ref is the file, and an entity contributes its file not its lines', () => {
  assert.deepEqual(defaultRefs({ grain: 'file', path: 'src/server/mod.rs' }), [
    'src/server/mod.rs',
  ]);
  assert.deepEqual(
    defaultRefs({ grain: 'entity', path: 'src/server/mod.rs', entityName: 'build_router' }),
    ['src/server/mod.rs'],
  );
});

test('the repo root has nothing to claim', () => {
  assert.deepEqual(defaultRefs({ grain: 'folder', path: '' }), []);
});

test('a suggested name is an identifier the lexer will read back', () => {
  assert.equal(suggestName({ grain: 'file', path: 'src/server/diff_handler.rs' }), 'diff_handler');
  assert.equal(suggestName({ grain: 'file', path: 'ui/src/components/EntityInfo.svelte' }), 'entity_info');
  assert.equal(suggestName({ grain: 'folder', path: 'ui/src' }), 'src');
  assert.equal(
    suggestName({ grain: 'entity', path: 'a.rs', entityName: 'buildNodeEncoding' }),
    'build_node_encoding',
  );
  // A leading digit is not an identifier; an underscore rescues it.
  assert.equal(suggestName({ grain: 'file', path: 'src/2fast.rs' }), '_2fast');
  // Nothing usable is left blank rather than guessed at.
  assert.equal(suggestName({ grain: 'folder', path: '' }), '');
});

// ─── Which parents are offerable ───────────────────────────────────────

test('only spec entities are offered as parents, never a code entity of the same kind', () => {
  const categories = parentOptions(graph(), 'f');
  assert.deepEqual(
    categories.map((c) => c.id),
    ['elevator::c.server', 'elevator::c.visualizer'],
  );
  // The Rust `Category` struct is in the graph and is not in the list.
  assert.ok(!categories.some((c) => c.file.endsWith('.rs')));
});

test('a functionality is offered features, and a category is offered nothing', () => {
  assert.deepEqual(
    parentOptions(graph(), 'fu').map((p) => p.qualifiedName),
    ['visual_scopes'],
  );
  assert.deepEqual(parentOptions(graph(), 'c'), []);
  assert.deepEqual(parentOptions(graph(), 'concept'), []);
  assert.deepEqual(parentOptions(null, 'f'), []);
});

test('the spec files offered are the elv files the analysis loaded', () => {
  assert.deepEqual(specFiles(graph()), ['spec/server.elv', 'spec/visualizer.elv']);
  assert.deepEqual(specFiles(null), []);
});

// ─── Where the write lands ─────────────────────────────────────────────

const PARENTS: ParentOption[] = [
  { id: 'elevator::c.server', qualifiedName: 'server', file: 'spec/server.elv' },
  { id: 'elevator::c.visualizer', qualifiedName: 'visualizer', file: 'spec/visualizer.elv' },
];

function draft(over: Partial<SpecDraft> = {}): SpecDraft {
  return {
    kind: 'f',
    name: 'diff_streaming',
    parentId: 'elevator::c.server',
    description: '',
    codeRefs: ['src/server/diff_handler.rs'],
    file: '',
    ...over,
  };
}

test('a child is written into its parents own file, never a file the reader picked', () => {
  assert.equal(targetFile(draft({ file: 'spec/elsewhere.elv' }), PARENTS), 'spec/server.elv');
});

test('a kind that stands alone is written where the reader said', () => {
  const d = draft({ kind: 'concept', parentId: null, file: 'spec/concepts.elv' });
  assert.equal(targetFile(d, PARENTS), 'spec/concepts.elv');
});

// ─── What may be submitted ─────────────────────────────────────────────

test('a well-formed draft has nothing to report', () => {
  assert.deepEqual(validateDraft(draft()), []);
});

test('names that would not lex are caught before the request', () => {
  assert.ok(validateDraft(draft({ name: '' }))[0].includes('name'));
  assert.ok(validateDraft(draft({ name: 'has-dash' })).length === 1);
  assert.ok(validateDraft(draft({ name: '2fast' })).length === 1);
  assert.ok(validateDraft(draft({ name: 'f.thing' }))[0].includes('leaf name'));
});

test('a functionality without a feature cannot be submitted', () => {
  const problems = validateDraft(draft({ kind: 'fu', parentId: null }));
  assert.ok(problems.some((p) => p.includes('Feature')), problems.join(' / '));
  assert.ok(requiresParent('fu'));
  assert.ok(!requiresParent('f'));
});

test('an orphan feature is submittable, because the language calls it a hint', () => {
  assert.deepEqual(validateDraft(draft({ parentId: null })), []);
});

test('a standalone kind needs a elv file and refuses anything else', () => {
  const noFile = validateDraft(draft({ kind: 'c', parentId: null, file: '' }));
  assert.ok(noFile.some((p) => p.includes('.elv')), noFile.join(' / '));
  const wrongExt = validateDraft(draft({ kind: 'c', parentId: null, file: 'spec/server.rs' }));
  assert.ok(wrongExt.some((p) => p.includes('.elv')), wrongExt.join(' / '));
  assert.deepEqual(validateDraft(draft({ kind: 'c', parentId: null, file: 'spec/new.elv' })), []);
});

// ─── What gets written ─────────────────────────────────────────────────

test('a described feature with a ref renders the block the engine writes', () => {
  const d = draft({ description: '  Streams a diff.  ' });
  assert.equal(
    previewSource(d, PARENTS),
    'f diff_streaming {\n    d: "Streams a diff."\n    cr: "src/server/diff_handler.rs"\n}',
  );
});

test('an empty draft renders the body-less sketch rather than an empty body', () => {
  const d = draft({ description: '', codeRefs: [] });
  assert.equal(previewSource(d, PARENTS), 'f diff_streaming');
});

/**
 * The language has no string escapes — `lexer.rs::string` ends the literal at
 * the first `"` and reads `\` as an ordinary character — so a quote in the
 * prose must be *replaced*. Escaping it, which is the obvious thing to write,
 * produces a file that stops parsing mid-sentence.
 *
 * Asserted on the preview as well as on the engine because the two renderers
 * disagreeing about this is the case where the reader approves four lines and
 * a different four are written.
 */
test('a quote in the prose is replaced, because the language cannot escape one', () => {
  const d = draft({ description: 'the "cr:" field, or C:\\ paths', codeRefs: [] });
  const rendered = previewSource(d, PARENTS);
  assert.equal(rendered, 'f diff_streaming {\n    d: "the \'cr:\' field, or C:\\ paths"\n}');
  // Exactly the two quotes that open and close the literal.
  assert.equal(rendered.split('"').length - 1, 2, rendered);
});

test('a newline typed into the description folds instead of breaking the file', () => {
  const d = draft({ description: 'one\ntwo\t three', codeRefs: [] });
  assert.ok(previewSource(d, PARENTS).includes('d: "one two three"'));
});

test('a functionality is written in the explicit form qualified by its feature', () => {
  const features: ParentOption[] = [
    { id: 'elevator::f.visual_scopes', qualifiedName: 'visual_scopes', file: 'spec/visualizer.elv' },
  ];
  const d = draft({
    kind: 'fu',
    name: 'shape',
    parentId: 'elevator::f.visual_scopes',
    codeRefs: [],
  });
  assert.equal(previewSource(d, features), 'fu f.visual_scopes.shape');
  // Bare inside the parent's body, where it resolves to the same entity.
  assert.equal(previewChildRef(d), 'fu shape');
});

test('the second edit is shown, because the click changes two places in the file', () => {
  assert.equal(previewChildRef(draft()), 'f diff_streaming');
  // Nothing to show when the entity stands alone.
  assert.equal(previewChildRef(draft({ parentId: null })), null);
  assert.equal(previewChildRef(draft({ kind: 'concept', parentId: null })), null);
});

// ─── Opening the form ──────────────────────────────────────────────────

test('a lone candidate parent is preselected and several are not', () => {
  const target = { grain: 'file' as const, path: 'src/server/diff_handler.rs' };
  const one = draftFor(target, 'f', [PARENTS[0]], ['spec/server.elv']);
  assert.equal(one.parentId, 'elevator::c.server');
  assert.equal(one.name, 'diff_handler');
  assert.deepEqual(one.codeRefs, ['src/server/diff_handler.rs']);

  const many = draftFor(target, 'f', PARENTS, ['spec/server.elv']);
  assert.equal(many.parentId, null, 'guessing among several would file it somewhere wrong');
});

test('every offered kind is one the engine accepts', () => {
  assert.deepEqual(
    SPEC_KINDS.map((k) => k.code),
    ['c', 'f', 'fu', 'concept'],
  );
});
