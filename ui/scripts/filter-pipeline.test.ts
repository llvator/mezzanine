/**
 * Unit tests for the filter pipeline — the strip that says what is narrowing
 * the canvas (UI-099).
 *
 * Two failures matter and they are opposites. A strip that **stays quiet**
 * about a running filter is the bug the feature exists to fix, only now with a
 * control on screen that appears to confirm nothing is wrong. A strip that
 * **claims** a filter nobody set — a chip for hidden ghosts in a graph with no
 * ghosts, a spec filter with no spec — teaches the reader to ignore it, which
 * costs more than never having built it.
 *
 * The order is tested too: the whole claim of the word *pipeline* is that
 * these run in a known sequence, and a strip that shuffles them explains
 * nothing about why a search of forty put eight on screen.
 *
 *   npm run test:pipeline
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  STAGE_ORDER,
  effectiveDepth,
  filterPipeline,
  nameList,
  pipelineSummary,
  type FilterStageId,
  type PipelineInput,
} from '../src/viewmodels/filterPipeline.ts';

/** Nothing filtering anything: every switch at its "show me everything"
 *  position, in a dataset that has ghosts and template vars to hide. */
const clear = (): PipelineInput => ({
  kinds: { hidden: [], total: 11 },
  relations: { hidden: [], total: 7, outgoingHidden: false, incomingHidden: false },
  languages: { hidden: [], total: 3 },
  files: { hidden: [], total: 40 },
  ghosts: { allHidden: false, builtinsHidden: false, present: true, builtinsPresent: true },
  templateVars: { hidden: false, present: true },
  spec: null,
  search: null,
  diff: null,
  focus: null,
  levels: {
    directHidden: false,
    crossLevelHidden: false,
    peerHiddenLevels: [],
    overridesOff: 0,
  },
  hubs: null,
});

const ids = (input: PipelineInput): FilterStageId[] =>
  filterPipeline(input).map((s) => s.id);

const stage = (input: PipelineInput, id: FilterStageId) => {
  const found = filterPipeline(input).find((s) => s.id === id);
  assert.ok(found, `expected a ${id} stage`);
  return found;
};

// --- silence when nothing is filtering ---

test('an unfiltered canvas produces no stages at all', () => {
  // Not a chip reading "no filters": that is one more thing to read past on
  // the way to the graph, every session, forever.
  assert.deepEqual(filterPipeline(clear()), []);
});

test('a filter with nothing to filter is not reported', () => {
  // Builtin ghosts are hidden by default. In a graph that has none, saying so
  // is noise that trains the reader to ignore the strip.
  const input = clear();
  input.ghosts.builtinsHidden = true;
  input.ghosts.builtinsPresent = false;
  input.templateVars.hidden = true;
  input.templateVars.present = false;
  assert.deepEqual(ids(input), []);
});

test('a default-on filter is still reported when it bites', () => {
  const input = clear();
  input.ghosts.builtinsHidden = true;
  assert.deepEqual(ids(input), ['ghosts']);
  assert.equal(stage(input, 'ghosts').value, 'builtins hidden');
});

test('hiding every ghost outranks the builtin sub-toggle', () => {
  // One question ("why is the call to len not drawn"), one chip. The wider
  // setting is the true answer, so it is the one shown.
  const input = clear();
  input.ghosts.allHidden = true;
  input.ghosts.builtinsHidden = true;
  const found = filterPipeline(input).filter((s) => s.id === 'ghosts');
  assert.equal(found.length, 1);
  assert.equal(found[0].value, 'all hidden');
});

// --- order ---

test('stages come out in the order the canvas applies them', () => {
  const input = clear();
  input.hubs = { count: 5 };
  input.focus = { name: 'compute', depth: 2, mode: 'force' };
  input.kinds = { hidden: ['Parameter'], total: 11 };
  input.spec = { entities: ['Auth'], paths: 3 };
  input.search = { term: 'parse', kept: 8, hides: false };
  assert.deepEqual(ids(input), ['spec', 'kinds', 'search', 'focus', 'hubs']);
});

test('whatever is active is a subsequence of the declared order', () => {
  const input = clear();
  input.ghosts.allHidden = true;
  input.templateVars.hidden = true;
  input.spec = { entities: ['Auth'], paths: 2 };
  input.kinds = { hidden: ['Parameter'], total: 11 };
  input.languages = { hidden: ['Markdown'], total: 3 };
  input.files = { hidden: ['a.ts'], total: 40 };
  input.search = { term: 'x', kept: 1, hides: true };
  input.diff = { level: 'edits', dims: false };
  input.focus = { name: 'f', depth: 1, mode: 'force' };
  input.relations = { hidden: ['Contains'], total: 7, outgoingHidden: false, incomingHidden: false };
  input.levels.directHidden = true;
  input.hubs = { count: 3 };

  const produced = ids(input);
  assert.equal(produced.length, STAGE_ORDER.length, 'every stage should be active here');
  let at = -1;
  for (const id of produced) {
    const next = STAGE_ORDER.indexOf(id);
    assert.ok(next > at, `${id} is out of pipeline order`);
    at = next;
  }
});

// --- what each stage says ---

test('a set filter counts what survives, not what was removed', () => {
  // "6 of 11" answers "how much of the graph am I looking at". "5 hidden"
  // answers a question nobody asked while staring at a sparse canvas.
  const input = clear();
  input.kinds = { hidden: ['Parameter', 'Property', 'Constant', 'Variable', 'Import'], total: 11 };
  assert.equal(stage(input, 'kinds').value, '6 of 11');
});

test('files are counted, not enumerated, in the chip', () => {
  const input = clear();
  input.files = { hidden: ['a.ts', 'b.ts', 'c.ts'], total: 40 };
  const s = stage(input, 'files');
  assert.equal(s.value, '3 hidden');
  assert.match(s.detail, /a\.ts/);
});

test('a spec entity that declares no code says so rather than reading as empty', () => {
  // `paths: 0` is an answer about the spec — a Feature with no cr: coverage —
  // and folding it into "nothing selected" hides exactly the drift the split
  // view exists to surface.
  const input = clear();
  input.spec = { entities: ['Billing'], paths: 0 };
  const s = stage(input, 'spec');
  assert.match(s.detail, /declares no code/);
});

test('a dimming search says the graph is still there', () => {
  const input = clear();
  input.search = { term: 'parse', kept: 8, hides: false };
  const s = stage(input, 'search');
  assert.equal(s.dims, true);
  assert.match(s.detail, /dimmed, not removed/);

  input.search = { term: 'parse', kept: 8, hides: true };
  const hiding = stage(input, 'search');
  assert.equal(hiding.dims, false);
  assert.match(hiding.detail, /removed/);
});

test('a selection is reported as a filter, because it is one', () => {
  // The one nobody expects: clicking a node to read its Details also cuts the
  // canvas to that node's reach.
  const input = clear();
  input.focus = { name: 'parseQuery', depth: 2, mode: 'force' };
  const s = stage(input, 'focus');
  assert.equal(s.value, 'parseQuery · 2 hops');
});

test('a focus with every level switched off says the canvas is one node', () => {
  const input = clear();
  input.focus = { name: 'parseQuery', depth: 0, mode: 'force' };
  assert.match(stage(input, 'focus').detail, /alone/);
});

test('level rules collapse into one chip and name themselves when there is one', () => {
  const input = clear();
  input.focus = { name: 'f', depth: 2, mode: 'force' };
  input.levels.directHidden = true;
  assert.equal(stage(input, 'levels').value, 'direct edges hidden');

  input.levels.crossLevelHidden = true;
  input.levels.peerHiddenLevels = [2];
  assert.equal(stage(input, 'levels').value, '3 rules');
  assert.match(stage(input, 'levels').detail, /peer edges off at L2/);
});

test('level and direction rules stay quiet with nothing selected', () => {
  // They steer the walk out of a selection. With no selection there is no
  // walk, and a chip for a rule that cannot bite is how a strip earns the
  // reader's indifference.
  const input = clear();
  input.levels = {
    directHidden: true,
    crossLevelHidden: true,
    peerHiddenLevels: [1, 2],
    overridesOff: 4,
  };
  input.relations.outgoingHidden = true;
  assert.deepEqual(ids(input), []);
});

test('a direction toggle is reported once something is selected', () => {
  const input = clear();
  input.focus = { name: 'f', depth: 1, mode: 'force' };
  input.relations.incomingHidden = true;
  assert.equal(stage(input, 'relations').value, 'outgoing only');

  input.relations.hidden = ['Contains', 'Imports'];
  assert.equal(stage(input, 'relations').value, '5 of 7 · outgoing only');
});

test('hub demotion is reported as hiding edges, never entities', () => {
  const input = clear();
  input.hubs = { count: 5 };
  const s = stage(input, 'hubs');
  assert.equal(s.value, 'top 5 muted');
  assert.match(s.detail, /still drawn/);
});

test('every stage carries a distinct restoration, not a generic Clear', () => {
  const input = clear();
  input.ghosts.allHidden = true;
  input.templateVars.hidden = true;
  input.spec = { entities: ['Auth'], paths: 2 };
  input.kinds = { hidden: ['Parameter'], total: 11 };
  input.languages = { hidden: ['Markdown'], total: 3 };
  input.files = { hidden: ['a.ts'], total: 40 };
  input.search = { term: 'x', kept: 1, hides: true };
  input.diff = { level: 'rewiring', dims: false };
  input.focus = { name: 'f', depth: 1, mode: 'tree' };
  input.relations = { hidden: ['Contains'], total: 7, outgoingHidden: false, incomingHidden: true };
  input.levels.directHidden = true;
  input.hubs = { count: 3 };

  const hints = filterPipeline(input).map((s) => s.clearHint);
  assert.equal(new Set(hints).size, hints.length, 'clear hints must be distinguishable');
  for (const hint of hints) assert.notEqual(hint.trim(), '');
});

// --- effective depth ---

test('depth is what the view reaches, not what the depth control says', () => {
  // "3 hops" over a view that stops at 1 is the confident wrong answer this
  // strip exists to remove.
  const levels = { 1: { enabled: true }, 2: { enabled: false }, 3: { enabled: false } };
  assert.equal(effectiveDepth(levels, 3), 1);
});

test('depth is capped by the tree-depth control even when deeper levels are on', () => {
  const levels = { 1: { enabled: true }, 2: { enabled: true }, 3: { enabled: true } };
  assert.equal(effectiveDepth(levels, 2), 2);
});

test('every level off is depth zero, not depth one', () => {
  const levels = { 1: { enabled: false }, 2: { enabled: false }, 3: { enabled: false } };
  assert.equal(effectiveDepth(levels, 3), 0);
});

// --- helpers ---

test('a long name list is truncated with a count rather than dumped', () => {
  assert.equal(nameList(['a', 'b', 'c'], 6), 'a, b, c');
  assert.equal(
    nameList(['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h'], 6),
    'a, b, c, d, e, f, and 2 more',
  );
});

test('the summary line names the stages for a folded toolbar', () => {
  const input = clear();
  input.kinds = { hidden: ['Parameter'], total: 11 };
  input.focus = { name: 'f', depth: 1, mode: 'force' };
  assert.equal(pipelineSummary(filterPipeline(input)), '2 filters: Kinds, Focus');
  assert.match(pipelineSummary([]), /No filters/);
});
