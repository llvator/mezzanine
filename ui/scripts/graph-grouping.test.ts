/**
 * Unit tests for the pure parts of the folder-grouping work: the cohesion
 * force (UI-052) and its ancestor chain (UI-069), the layout seed (UI-053),
 * the hover membership lookup (UI-054) and the hull geometry (UI-055).
 *
 * Same zero-dependency setup as the sibling suites — Node's built-in runner
 * plus type stripping. These belong here rather than only in the browser
 * probe because the properties that matter are numeric and invisible: that
 * `off` is *exactly* inert, that a settled centroid actually moves nodes
 * toward it, that two runs agree to the bit, that an outline clears the
 * circles it encloses. A click-through can show you a picture; it cannot
 * show you determinism.
 *
 * Note the extensionless-import trap: modules reached from here must not
 * `import` a sibling without a `.ts` suffix, because Vite resolves that and
 * bare Node does not. `folderHulls` takes its group key as an argument
 * partly for this reason, and partly because UI-059 may change what a group
 * is.
 *
 *   npm run test:grouping
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  folderKeyOf, cohesionStrengthFor, forceFolderCohesion,
  ancestorChainOf, tierWeightFor, folderTierDepth,
  fileKeyOf, groupKeyOf, groupChainOf, groupGrainFor,
  COHESION_STRENGTH, COHESION_LEVELS,
} from '../src/utils/forceCohesion.ts';
import { makeSeeder, wedgeKeyOf, fileWedgeKeyOf } from '../src/utils/layoutSeed.ts';
import { computeFolderHulls, labelBoxOf, MIN_HULL_MEMBERS } from '../src/viewmodels/folderHulls.ts';
import { regionsAtPoint, tightestRegion, sameRegions } from '../src/viewmodels/regionsAtPoint.ts';
import { groupMemberIds } from '../src/viewmodels/hoverHighlight.ts';
import { rankHubs, hubNames } from '../src/viewmodels/hubs.ts';
import { collapseGraph } from '../src/viewmodels/collapseGraph.ts';
import {
  linkFolderWeights, folderKinship, folderTreeDistance,
  KIN_DISTANCE, STRANGER_DISTANCE, STRANGER_STRENGTH,
} from '../src/utils/linkFolderWeights.ts';
import type { D3Link, D3Node } from '../src/types/graph.ts';

/** The real d3 forces, for the UI-102 settle. Every other test here runs
 *  hand-written arithmetic, deliberately — but the thing being asserted at
 *  the bottom of this file is that a *d3* link force and the cohesion force
 *  reach a particular truce, and reimplementing d3's tick to prove that would
 *  be asserting against a copy of the code under test. */
import * as d3 from 'd3-force';

/** Minimal node: the two fields both modules read, plus somewhere for d3 to
 *  write. Cast once here rather than at every call site. */
function node(id: string, file_path: string, x = 0, y = 0): D3Node {
  return { id, file_path, x, y, vx: 0, vy: 0 } as unknown as D3Node;
}

type Sim = D3Node & { x: number; y: number; vx: number; vy: number };
const sim = (n: D3Node) => n as unknown as Sim;

// ── folderKeyOf ─────────────────────────────────────────────────────────

test('the group is the directory holding the file', () => {
  assert.equal(folderKeyOf(node('a', 'ui/src/stores/graph.ts')), 'ui/src/stores');
});

test('a repo-root file belongs to the root group, not to no group', () => {
  assert.equal(folderKeyOf(node('a', 'vite.config.ts')), '');
});

test('the same directory spelled two ways is one group', () => {
  // One analyze run emits both spellings (AN-015). On this repo's payload
  // that splits five directories, worst of them `ui/src/stores` at 744
  // entities one way and 85 the other — two centroids pulling one folder
  // apart, and a hover that lights a tenth of what it was asked about.
  assert.equal(folderKeyOf(node('a', './ui/src/stores/graph.ts')), 'ui/src/stores');
  assert.equal(folderKeyOf(node('b', 'ui/src/stores/graph.ts')), 'ui/src/stores');
});

test('a dot-slash root file is in the root group, not a group called dot', () => {
  assert.equal(folderKeyOf(node('a', './vite.config.ts')), '');
});

test('both spellings of one folder cohere as one', () => {
  // The property the two above exist for, at the level that matters: a node
  // written one way is pulled toward nodes written the other.
  const a = node('a', './pkg/one.ts', 0, 0);
  const b = node('b', 'pkg/two.ts', 100, 0);
  const force = forceFolderCohesion().strength(0.5);
  force.initialize([a, b]);
  force(1);
  assert.ok(sim(a).vx > 0, `a.vx = ${sim(a).vx}`);
  assert.ok(sim(b).vx < 0, `b.vx = ${sim(b).vx}`);
});

test('a ghost has no group at all', () => {
  // Not '' — that is the root folder, and collecting every stdlib reference
  // into it would drag one phantom group across the canvas.
  assert.equal(folderKeyOf(node('a', '')), null);
});

// ── cohesionStrengthFor ─────────────────────────────────────────────────

test('off is exactly zero, so the layout at off is the pre-UI-052 layout', () => {
  assert.equal(COHESION_STRENGTH.off, 0);
  assert.equal(cohesionStrengthFor('entity', 'off'), 0);
});

test('strength rises with the setting', () => {
  const s = COHESION_LEVELS.map((l) => cohesionStrengthFor('entity', l));
  for (let i = 1; i < s.length; i++) assert.ok(s[i] > s[i - 1], `${s[i]} > ${s[i - 1]}`);
});

test('the force is inert at module level, where each node is already a folder', () => {
  for (const level of COHESION_LEVELS) {
    assert.equal(cohesionStrengthFor('folder', level), 0);
  }
});

test('entity and file level both get the force', () => {
  assert.ok(cohesionStrengthFor('entity', 'high') > 0);
  assert.ok(cohesionStrengthFor('file', 'high') > 0);
});

// ── forceFolderCohesion ─────────────────────────────────────────────────

test('nodes are pulled toward their own folder centroid', () => {
  const a = node('a', 'ui/src/stores/one.ts', 0, 0);
  const b = node('b', 'ui/src/stores/two.ts', 100, 0);
  const nodes = [a, b];

  const force = forceFolderCohesion().strength(0.5);
  force.initialize(nodes);
  force(1);

  // Centroid is x=50. `a` is left of it and must gain rightward velocity;
  // `b` is right of it and must gain leftward velocity.
  assert.ok(sim(a).vx > 0, `a.vx = ${sim(a).vx}`);
  assert.ok(sim(b).vx < 0, `b.vx = ${sim(b).vx}`);
  assert.equal(sim(a).vy, 0);
});

test('nodes in different folders are not pulled toward each other', () => {
  const a = node('a', 'ui/src/stores/one.ts', 0, 0);
  const b = node('b', 'src/parser/two.rs', 100, 0);
  const nodes = [a, b];

  const force = forceFolderCohesion().strength(0.5);
  force.initialize(nodes);
  force(1);

  // Each is alone in its group, so each is already at its own centroid.
  assert.equal(sim(a).vx, 0);
  assert.equal(sim(b).vx, 0);
});

test('strength 0 touches nothing', () => {
  const a = node('a', 'ui/src/stores/one.ts', 0, 0);
  const b = node('b', 'ui/src/stores/two.ts', 100, 0);
  const nodes = [a, b];

  const force = forceFolderCohesion().strength(0);
  force.initialize(nodes);
  force(1);

  assert.equal(sim(a).vx, 0);
  assert.equal(sim(b).vx, 0);
});

test('ghosts are never moved by the force', () => {
  const g1 = node('g1', '', 0, 0);
  const g2 = node('g2', '', 500, 500);
  const real = node('r', 'ui/src/one.ts', 10, 10);
  const nodes = [g1, g2, real];

  const force = forceFolderCohesion().strength(0.5);
  force.initialize(nodes);
  force(1);

  assert.equal(sim(g1).vx, 0);
  assert.equal(sim(g1).vy, 0);
  assert.equal(sim(g2).vx, 0);
  assert.equal(sim(g2).vy, 0);
});

test('the pull scales with alpha, so it cools with the simulation', () => {
  const hot = [node('a', 'p/one.ts', 0, 0), node('b', 'p/two.ts', 100, 0)];
  const cold = [node('a', 'p/one.ts', 0, 0), node('b', 'p/two.ts', 100, 0)];

  const f1 = forceFolderCohesion().strength(0.5);
  f1.initialize(hot);
  f1(1);
  const f2 = forceFolderCohesion().strength(0.5);
  f2.initialize(cold);
  f2(0.1);

  assert.ok(sim(hot[0]).vx > sim(cold[0]).vx * 5);
});

test('re-initialising rebuilds groups for a new node set', () => {
  const force = forceFolderCohesion().strength(0.5);
  force.initialize([node('a', 'p/one.ts', 0, 0), node('b', 'p/two.ts', 100, 0)]);

  // d3 calls initialize again whenever simulation.nodes() is set — the
  // incremental update path relies on this.
  const fresh = [node('c', 'q/three.ts', 0, 0), node('d', 'q/four.ts', 100, 0)];
  force.initialize(fresh);
  force(1);

  assert.ok(sim(fresh[0]).vx > 0);
});

// ── the ancestor chain (UI-069) ─────────────────────────────────────────

test('the chain runs from the node\'s own folder upward, nearest first', () => {
  assert.deepEqual(ancestorChainOf('ui/src/stores'), ['ui/src/stores', 'ui/src', 'ui']);
});

test('a top-level folder has no tier above it', () => {
  // The root is not a directory anyone put anything in on purpose, and a tier
  // holding the whole tree is priced out anyway.
  assert.deepEqual(ancestorChainOf('ui'), ['ui']);
  assert.deepEqual(ancestorChainOf(''), ['']);
});

test('the chain is capped, so a deep path does not cost a pass per level', () => {
  const chain = ancestorChainOf('a/b/c/d/e/f/g');
  assert.equal(chain.length, 4);
  assert.deepEqual(chain, ['a/b/c/d/e/f/g', 'a/b/c/d/e/f', 'a/b/c/d/e', 'a/b/c/d']);
});

test('a tier holding the whole tree is worth nothing', () => {
  // It has the graph's centroid, so pulling toward it is a disguised
  // forceCenter. This is the rule that keeps an ancestor from stealing pull
  // from the leaf folders while telling the reader nothing.
  assert.equal(tierWeightFor(1, 100, 100), 0);
});

test('a tier is worth more the more of the graph it leaves out', () => {
  const quarter = tierWeightFor(1, 25, 100);
  const most = tierWeightFor(1, 90, 100);
  assert.ok(quarter > most, `${quarter} vs ${most}`);
  assert.ok(quarter > 0 && most > 0);
});

test('a nearer ancestor outweighs a farther one at equal coverage', () => {
  assert.ok(tierWeightFor(1, 20, 100) > tierWeightFor(2, 20, 100));
});

test('the node\'s own folder keeps its full weight whatever it covers', () => {
  // UI-052's promise. A scope narrowed to one folder must keep cohering the
  // way it always did rather than losing it to a rule about ancestors.
  assert.equal(tierWeightFor(0, 100, 100), 1);
});

// ── the folder tier at file grain (UI-113) ─────────────────────────────

test('folder grain is unchanged by the folder exemption', () => {
  // The whole safety argument: at folder grain the folder IS depth 0, the
  // early return fires first, and `folderDepth` can never be reached. Every
  // assertion above therefore still describes the shipped force.
  for (const [d, size] of [[1, 20], [2, 20], [1, 90], [3, 5]] as const) {
    assert.equal(tierWeightFor(d, size, 100), tierWeightFor(d, size, 100, 0));
  }
});

test('at file grain the folder is not charged the ancestor decay', () => {
  // The bug UI-113 fixes. The folder sits at depth 1 because `groupChainOf`
  // prepended the file, and it was collecting a discount meant for the
  // subtree above it while still being a group the node is directly in.
  const asAncestor = tierWeightFor(1, 25, 100);
  const asOwnFolder = tierWeightFor(1, 25, 100, 1);
  assert.ok(asOwnFolder > asAncestor, `${asOwnFolder} vs ${asAncestor}`);
  // Exactly the decay removed, and nothing else: the coverage term stays.
  assert.equal(asOwnFolder, 1 - 25 / 100);
});

test('the exempt folder still pays the coverage discount', () => {
  // The two rules answer different questions and only one of them was wrong.
  // A folder holding the whole drawn set has the graph's centroid whatever
  // depth the grain puts it at, so it is still worth nothing.
  assert.equal(tierWeightFor(1, 100, 100, 1), 0);
  assert.ok(tierWeightFor(1, 25, 100, 1) > tierWeightFor(1, 90, 100, 1));
});

test('the exemption lands on one tier, not on everything below it', () => {
  // A grandparent at file grain is depth 2 and is still an ancestor.
  const parent = tierWeightFor(1, 20, 100, 1);
  const grandparent = tierWeightFor(2, 20, 100, 1);
  assert.ok(parent > grandparent, `${parent} vs ${grandparent}`);
  assert.equal(grandparent, tierWeightFor(2, 20, 100, 0));
});

test('folderTierDepth matches where groupChainOf puts the folder', () => {
  // The pairing that must not drift: whatever tier the chain gives the
  // folder is the tier the weighting exempts. Asserted against the chain
  // itself rather than against a literal, so a change to either is caught.
  const n = node('a', 'ui/src/stores/graph.ts');
  for (const grain of ['folder', 'file'] as const) {
    const chain = groupChainOf(n, grain);
    assert.equal(chain[folderTierDepth(grain)], 'ui/src/stores');
  }
});

// ── the force, over an ancestor chain (UI-069) ──────────────────────────

/**
 * Run the force to rest, the way d3 would: cool alpha, apply, decay velocity,
 * integrate. Cohesion is the only force here — no charge, no collision — so
 * this measures what cohesion alone claims and nothing else.
 */
function settle(nodes: D3Node[], strength: number, ticks = 400): void {
  const force = forceFolderCohesion().strength(strength);
  force.initialize(nodes);
  let alpha = 1;
  for (let t = 0; t < ticks; t++) {
    alpha += (0 - alpha) * 0.0228;   // d3's default cooling
    force(alpha);
    for (const n of nodes) {
      const s = sim(n);
      s.vx *= 0.6;                   // d3's default velocityDecay of 0.4
      s.vy *= 0.6;
      s.x += s.vx;
      s.y += s.vy;
    }
  }
}

/**
 * Four leaf folders in two subtrees, three nodes each, seeded **interleaved**
 * along a line: SF11, SF21, SF12, SF22.
 *
 * The interleaving is the point. Siblings start 800 apart and cousins average
 * 600, so a test that finds siblings closer at the end has watched the force
 * invert its own starting layout — it cannot pass on the seed. The slots are
 * also asymmetric on purpose: F1's centroid lands at -200 and F2's at +200,
 * where a symmetric arrangement would put both subtree centroids on the same
 * point and collapse the whole world to it, which measures nothing.
 *
 * `rename` builds the identical geometry under different paths, which is how
 * the control case below strips the kinship while changing nothing else.
 */
const KIN_SLOTS: ReadonlyArray<readonly [string, number]> = [
  ['F1/SF11', -600],
  ['F2/SF21', -200],
  ['F1/SF12', 200],
  ['F2/SF22', 600],
];

function kinshipWorld(rename: (folder: string) => string): D3Node[] {
  const out: D3Node[] = [];
  for (const [folder, x] of KIN_SLOTS) {
    for (let i = 0; i < 3; i++) {
      out.push(node(`${folder}/f${i}`, `${rename(folder)}/f${i}.ts`, x + i * 12, i * 9 - 9));
    }
  }
  return out;
}

/** Ids carry the *nested* folder regardless of what the paths say, so both
 *  worlds are measured with the same pairing. */
const leafOf = (n: D3Node) => n.id.split('/').slice(0, 2).join('/');
const subtreeOf = (n: D3Node) => n.id.split('/')[0];

/** Mean distance over pairs in different leaf folders, split by whether the
 *  two leaves share a parent. */
function kinshipDistances(nodes: D3Node[]): { siblings: number; cousins: number } {
  let sSum = 0, sN = 0, cSum = 0, cN = 0;
  for (let i = 0; i < nodes.length; i++) {
    for (let j = i + 1; j < nodes.length; j++) {
      if (leafOf(nodes[i]) === leafOf(nodes[j])) continue;
      const d = Math.hypot(nodes[i].x! - nodes[j].x!, nodes[i].y! - nodes[j].y!);
      if (subtreeOf(nodes[i]) === subtreeOf(nodes[j])) { sSum += d; sN++; }
      else { cSum += d; cN++; }
    }
  }
  return { siblings: sSum / sN, cousins: cSum / cN };
}

test('sibling folders settle closer than cousin folders', () => {
  // The claim of the whole ticket. Asserted as a distance ratio on settled
  // positions, because UI-055 shipped a green suite over an unusable canvas
  // by asserting presence instead of separation.
  const world = kinshipWorld((f) => f);
  const start = kinshipDistances(world);
  assert.ok(start.siblings > start.cousins,
    `the seed already favours siblings (${start.siblings} vs ${start.cousins}) — this test has no teeth`);

  settle(world, COHESION_STRENGTH.medium);
  const end = kinshipDistances(world);
  assert.ok(end.siblings < end.cousins * 0.6,
    `siblings ${Math.round(end.siblings)} vs cousins ${Math.round(end.cousins)}`);
});

test('kinship comes from the tree, not from the arithmetic', () => {
  // Same geometry, same group sizes, same everything — except the four
  // folders are top-level, so no two of them share a parent. With no shared
  // ancestor the force must leave the seed's arrangement standing, siblings
  // still the farther-apart pair. If this inverts too, the effect above came
  // from the numbers rather than from the paths.
  const flat = kinshipWorld((f) => f.split('/')[1]);
  const start = kinshipDistances(flat);
  settle(flat, COHESION_STRENGTH.medium);
  const end = kinshipDistances(flat);
  assert.ok(end.siblings > end.cousins,
    `a flat tree invented kinship: siblings ${Math.round(end.siblings)} vs cousins ${Math.round(end.cousins)}`);
  const drift = Math.abs(end.siblings / end.cousins - start.siblings / start.cousins);
  assert.ok(drift < 0.1, `the between-folder picture moved by ${drift.toFixed(3)}`);
});

test('a flat tree gets exactly the pull UI-052 gave it', () => {
  // One tier, weight 1, no rescaling. A reader's saved cohesion setting must
  // not quietly become stronger or weaker because the force learned about
  // ancestors.
  const a = node('a', 'p/one.ts', 0, 0);
  const b = node('b', 'p/two.ts', 100, 0);
  const force = forceFolderCohesion().strength(0.5);
  force.initialize([a, b]);
  force(1);
  assert.equal(sim(a).vx, 50 * 0.5);
});

test('the tiers share one pull rather than stacking several', () => {
  // Two tiers whose centroids coincide: whatever the weights are, the total
  // must come out as a single pull of the configured strength. If the shares
  // did not sum to one, this node would be yanked twice toward the same point
  // and `high` would mean something new. `G` keeps `F1` from covering the
  // whole tree, which would price its tier out and prove nothing.
  const nodes = [
    node('a', 'F1/SF11/one.ts', 0, 0),
    node('b', 'F1/SF11/two.ts', 100, 0),
    node('c', 'F1/SF12/one.ts', 0, 0),
    node('d', 'F1/SF12/two.ts', 100, 0),
    node('e', 'G/one.ts', 900, 0),
    node('f', 'G/two.ts', 900, 0),
  ];
  const force = forceFolderCohesion().strength(0.5);
  force.initialize(nodes);
  force(1);
  // SF11's centroid and F1's centroid are both (50, 0).
  assert.equal(sim(nodes[0]).vx, 50 * 0.5);
});

test('the tier every node shares exerts no pull', () => {
  // Both folders sit under `ui/src`, so `ui/src` holds the entire tree and
  // its centroid is the graph's centroid. Each node here is already at its
  // own folder's centroid, so any movement at all would be the shared tier
  // acting as a disguised centering force.
  const nodes = [
    node('a', 'ui/src/a/one.ts', 0, 0),
    node('b', 'ui/src/a/two.ts', 0, 0),
    node('c', 'ui/src/b/one.ts', 100, 0),
    node('d', 'ui/src/b/two.ts', 100, 0),
  ];
  const force = forceFolderCohesion().strength(0.5);
  force.initialize(nodes);
  force(1);
  for (const n of nodes) assert.equal(sim(n).vx, 0, `${n.id} moved`);
});

test('a ghost is not in the coverage denominator', () => {
  // Otherwise a canvas full of stdlib references would make every real tier
  // look like a small share of the graph and hand it pull it has not earned.
  // Here `pkg` holds every placed node, so its tier is worth nothing whether
  // or not the ghosts are counted — and the two nodes must stay put.
  const nodes = [
    node('a', 'pkg/sub/one.ts', 0, 0),
    node('b', 'pkg/other/two.ts', 100, 0),
    node('g1', '', 500, 500),
    node('g2', '', 900, 900),
  ];
  const force = forceFolderCohesion().strength(0.5);
  force.initialize(nodes);
  force(1);
  assert.equal(sim(nodes[0]).vx, 0);
  assert.equal(sim(nodes[1]).vx, 0);
});

test('a lone file in its own folder is pulled by its parent', () => {
  // Its own tier holds one member and can only pull it toward where it
  // already is. Dropping that tier before normalising hands its share to the
  // parent, which is the difference between a stray sitting where the link
  // forces left it and a stray drifting home to its subtree.
  const solo = node('solo', 'F1/SF11/only.ts', 0, 0);
  const nodes = [
    solo,
    node('sib1', 'F1/SF12/one.ts', 300, 0),
    node('sib2', 'F1/SF12/two.ts', 300, 0),
    node('far1', 'G1/one.ts', -900, 0),
    node('far2', 'G1/two.ts', -900, 0),
  ];
  const force = forceFolderCohesion().strength(0.5);
  force.initialize(nodes);
  force(1);
  assert.ok(sim(solo).vx > 0, `solo.vx = ${sim(solo).vx}`);
});

test('a node with no pullable tier is left alone', () => {
  // One node per folder, no shared parent below the root: every tier holds
  // one member, so there is nothing to pull toward and no velocity to write.
  const a = node('a', 'p/one.ts', 0, 0);
  const b = node('b', 'q/two.ts', 100, 0);
  const force = forceFolderCohesion().strength(0.5);
  force.initialize([a, b]);
  force(1);
  assert.equal(sim(a).vx, 0);
  assert.equal(sim(b).vx, 0);
});

// ── group grain (UI-103) ────────────────────────────────────────────────

test('the folder grain is the pre-UI-103 chain, delegated not reimplemented', () => {
  // The whole safety argument for the grain: folder grain must be the same
  // answer `ancestorChainOf` already gave, or every measurement, screenshot
  // and tuned setting that predates UI-103 quietly stops describing the
  // shipped picture.
  const n = node('a', 'ui/src/stores/graph.ts');
  assert.deepEqual(groupChainOf(n, 'folder'), ancestorChainOf(folderKeyOf(n)!));
  assert.deepEqual(groupChainOf(n, 'folder'), ['ui/src/stores', 'ui/src', 'ui']);
});

test('the file grain puts the file below its folder', () => {
  assert.deepEqual(groupChainOf(node('a', 'ui/src/stores/graph.ts'), 'file'), [
    'ui/src/stores/graph.ts', 'ui/src/stores', 'ui/src', 'ui',
  ]);
});

test('the file is an extra tier, not one taken from the folders', () => {
  // Both grains reach the same distance up the tree at the same limit. A file
  // that cost a folder tier would silently shorten the ancestor reach a
  // reader has tuned, and the layout would change for a reason nothing names.
  const n = node('a', 'a/b/c/d/e.ts');
  const folders = groupChainOf(n, 'folder', 3);
  const files = groupChainOf(n, 'file', 3);
  assert.deepEqual(folders, ['a/b/c/d', 'a/b/c', 'a/b']);
  assert.deepEqual(files, ['a/b/c/d/e.ts', ...folders]);
});

test('the same file spelled two ways is one region', () => {
  // AN-015 one level down. Raw, `./ui/src/stores/graph.ts` and
  // `ui/src/stores/graph.ts` are two regions with two centroids for one file
  // — the defect `folderKeyOf` already fixes for the directory.
  assert.equal(fileKeyOf(node('a', './ui/src/stores/graph.ts')), 'ui/src/stores/graph.ts');
  assert.equal(fileKeyOf(node('b', 'ui/src/stores/graph.ts')), 'ui/src/stores/graph.ts');
  assert.deepEqual(
    groupChainOf(node('a', './pkg/one.ts'), 'file'),
    groupChainOf(node('b', 'pkg/one.ts'), 'file'),
  );
});

test('a repo-root file is its own region inside the root group', () => {
  assert.deepEqual(groupChainOf(node('a', 'vite.config.ts'), 'file'), ['vite.config.ts', '']);
  assert.deepEqual(groupChainOf(node('a', './vite.config.ts'), 'file'), ['vite.config.ts', '']);
});

test('a ghost is in no group at either grain', () => {
  // Not negotiable: a shared key would collect every unrelated stdlib
  // reference into one phantom region. `folderHulls`, `regionTraffic` and
  // `hoverHighlight` each document the same exemption.
  assert.equal(fileKeyOf(node('g', '')), null);
  assert.equal(groupKeyOf(node('g', ''), 'file'), null);
  assert.equal(groupKeyOf(node('g', ''), 'folder'), null);
  assert.deepEqual(groupChainOf(node('g', ''), 'file'), []);
  assert.deepEqual(groupChainOf(node('g', ''), 'folder'), []);
});

test('the innermost key is what the grain says it is', () => {
  const n = node('a', 'ui/src/stores/graph.ts');
  assert.equal(groupKeyOf(n, 'folder'), 'ui/src/stores');
  assert.equal(groupKeyOf(n, 'file'), 'ui/src/stores/graph.ts');
  assert.equal(groupKeyOf(n, 'file'), groupChainOf(n, 'file')[0]);
  assert.equal(groupKeyOf(n, 'folder'), groupChainOf(n, 'folder')[0]);
});

test('file grain is an entity-level reading and collapses elsewhere', () => {
  // At file level every node already *is* a file, so every file region would
  // hold one member and draw nothing; at module level the force is inert.
  assert.equal(groupGrainFor('entity', 'file'), 'file');
  assert.equal(groupGrainFor('file', 'file'), 'folder');
  assert.equal(groupGrainFor('folder', 'file'), 'folder');
  assert.equal(groupGrainFor('entity', 'folder'), 'folder');
});

test('at file grain the file centroid wins, where the folder had nothing to say', () => {
  // Two files of two entities each, in one folder. At folder grain all four
  // pull toward one centroid at x=250, so `a2` (x=100) is pulled right. At
  // file grain `a2` belongs to `p/one.ts`, whose centroid is x=50, and it is
  // pulled *left* instead. The reversal is the feature: same nodes, same
  // positions, and a grouping the folder could not express.
  const at = (grain: 'folder' | 'file') => {
    const a1 = node('a1', 'p/one.ts', 0, 0);
    const a2 = node('a2', 'p/one.ts', 100, 0);
    const nodes = [a1, a2, node('b1', 'p/two.ts', 400, 0), node('b2', 'p/two.ts', 500, 0)];
    const force = forceFolderCohesion((n) => groupChainOf(n, grain)).strength(0.5);
    force.initialize(nodes);
    force(1);
    return sim(a2).vx;
  };
  assert.ok(at('folder') > 0, `folder grain: ${at('folder')}`);
  assert.ok(at('file') < 0, `file grain: ${at('file')}`);
});

test('a file holding one entity needs no special case', () => {
  // Its tier has one member, so `initialize` drops it before normalising and
  // hands the whole share to the folder — which is exactly the pull the node
  // gets at folder grain. The "lone file in a folder of its own" rule, one
  // level down, for free.
  const at = (grain: 'folder' | 'file') => {
    const solo = node('solo', 'p/solo.ts', 0, 0);
    const nodes = [
      solo,
      node('b1', 'p/two.ts', 300, 0),
      node('b2', 'p/two.ts', 300, 0),
      node('f1', 'q/one.ts', -900, 0),
      node('f2', 'q/two.ts', -900, 0),
    ];
    const force = forceFolderCohesion((n) => groupChainOf(n, grain)).strength(0.5);
    force.initialize(nodes);
    force(1);
    return sim(solo).vx;
  };
  assert.ok(at('folder') > 0, `folder grain: ${at('folder')}`);
  assert.equal(at('file'), at('folder'));
});

test('the force defaults to the folder grain', () => {
  // A caller with no opinion gets the pre-UI-103 force exactly, which is what
  // keeps every settle assertion above measuring what it used to.
  const tick = (chainOf?: (n: D3Node) => string[]) => {
    const nodes = [node('a', 'p/sub/one.ts', 0, 0), node('b', 'p/sub/two.ts', 100, 0)];
    const force = chainOf ? forceFolderCohesion(chainOf) : forceFolderCohesion();
    force.strength(0.5);
    force.initialize(nodes);
    force(1);
    return nodes.map((n) => sim(n).vx);
  };
  assert.deepEqual(tick(), tick((n) => groupChainOf(n, 'folder')));
  assert.ok(tick()[0] > 0, 'the default force is inert, so this proves nothing');
});

// ── makeSeeder ──────────────────────────────────────────────────────────

test('the wedge key is the folder, with ghosts and root files kept apart', () => {
  assert.equal(wedgeKeyOf(node('a', 'ui/src/stores/graph.ts')), 'ui/src/stores');
  assert.notEqual(wedgeKeyOf(node('b', '')), wedgeKeyOf(node('c', 'vite.config.ts')));
});

test('the seed puts both spellings of one folder in one wedge', () => {
  // `.` sorts before every letter, so the two spellings would otherwise take
  // wedges on opposite sides of the circle and the folder would start the
  // layout torn in half. The seed and the force have to agree on what a
  // folder is, or the force spends the settle undoing the seed.
  assert.equal(wedgeKeyOf(node('a', './ui/src/stores/graph.ts')),
    wedgeKeyOf(node('b', 'ui/src/stores/graph.ts')));
  assert.equal(wedgeKeyOf(node('c', './main.ts')), wedgeKeyOf(node('d', 'main.ts')));
});

test('seeding is deterministic across runs', () => {
  const build = () => [
    node('a', 'ui/src/stores/one.ts'),
    node('b', 'ui/src/components/two.ts'),
    node('c', 'src/parser/three.rs'),
  ];
  const first = build();
  const second = build();
  const s1 = makeSeeder(first, 1600, 1000);
  const s2 = makeSeeder(second, 1600, 1000);

  for (let i = 0; i < first.length; i++) {
    assert.deepEqual(s1.position(first[i]), s2.position(second[i]));
  }
});

test('seeding does not depend on node array order', () => {
  // The whole point: array order is what the phyllotaxis default keyed on,
  // and it changes on every re-analysis.
  const a = node('a', 'ui/src/stores/one.ts');
  const b = node('b', 'ui/src/components/two.ts');
  const forward = makeSeeder([a, b], 1600, 1000);
  const reversed = makeSeeder([b, a], 1600, 1000);

  assert.deepEqual(forward.position(a), reversed.position(a));
  assert.deepEqual(forward.position(b), reversed.position(b));
});

test('a top-level directory occupies one contiguous arc', () => {
  // Sibling folders sort adjacent, so a subtree is contiguous in angle
  // without any tree walk. This is the property the radial partition exists
  // for, and it is what makes the seed readable rather than merely stable.
  const nodes = [
    node('a', 'ui/src/stores/one.ts'),
    node('b', 'ui/src/components/two.ts'),
    node('c', 'ui/src/utils/three.ts'),
    node('d', 'src/parser/four.rs'),
    node('e', 'src/analysis/five.rs'),
  ];
  const seeder = makeSeeder(nodes, 1600, 1000);

  const angles = nodes.map((n) => {
    const p = seeder.position(n);
    const raw = Math.atan2(p.y - 500, p.x - 800);
    return { root: n.file_path.split('/')[0], angle: raw < 0 ? raw + Math.PI * 2 : raw };
  });
  angles.sort((x, y) => x.angle - y.angle);

  // Walking the circle by angle, each root appears in exactly one run.
  const runs: string[] = [];
  for (const a of angles) if (runs[runs.length - 1] !== a.root) runs.push(a.root);
  assert.equal(new Set(runs).size, runs.length, `interleaved: ${runs.join(' ')}`);
});

test('ghosts get distinct positions rather than stacking on one point', () => {
  // They share an empty file_path, so a path-derived jitter would put every
  // one of them at the same pixel. The id is what separates them.
  const ghosts = [node('println', ''), node('HashMap', ''), node('Vec', '')];
  const seeder = makeSeeder(ghosts, 1600, 1000);
  const seen = new Set(ghosts.map((g) => JSON.stringify(seeder.position(g))));
  assert.equal(seen.size, ghosts.length);
});

test('every seed lands inside the viewport', () => {
  const nodes = Array.from({ length: 200 }, (_, i) =>
    node(`n${i}`, `pkg${i % 7}/sub${i % 3}/file${i}.ts`));
  const seeder = makeSeeder(nodes, 1600, 1000);
  for (const n of nodes) {
    const { x, y } = seeder.position(n);
    assert.ok(x > 0 && x < 1600, `x = ${x}`);
    assert.ok(y > 0 && y < 1000, `y = ${y}`);
  }
});

test('an empty node set does not divide by zero', () => {
  const seeder = makeSeeder([], 1600, 1000);
  const { x, y } = seeder.position(node('late', 'ui/src/one.ts'));
  assert.ok(Number.isFinite(x) && Number.isFinite(y));
});

// ── rankHubs (UI-056) ───────────────────────────────────────────────────

/** A node carrying a fan-in metric. */
function hub(id: string, fanIn: number | null, name = id): D3Node {
  return {
    id, name, file_path: `pkg/${id}.ts`,
    metrics: fanIn === null ? undefined : { fan_in: fanIn },
  } as unknown as D3Node;
}

test('the most-depended-on nodes are the ones demoted, in order', () => {
  const nodes = [hub('a', 3), hub('b', 40), hub('c', 12), hub('d', 1)];
  assert.deepEqual(rankHubs(nodes, 2), ['b', 'c']);
});

test('asking for none demotes none', () => {
  assert.deepEqual(rankHubs([hub('a', 99)], 0), []);
});

test('asking for more than exist is not an error', () => {
  assert.equal(rankHubs([hub('a', 5), hub('b', 3)], 10).length, 2);
});

test('a node the analyzer had nothing to say about is never a hub', () => {
  // Treating a missing fan_in as high would silently hide real edges.
  const nodes = [hub('known', 2), hub('unknown', null), hub('zero', 0)];
  assert.deepEqual(rankHubs(nodes, 3), ['known']);
});

test('ties break deterministically, so the demoted set does not shuffle', () => {
  const nodes = [hub('z', 10), hub('a', 10), hub('m', 10)];
  assert.deepEqual(rankHubs(nodes, 2), rankHubs(nodes, 2));
  assert.deepEqual(rankHubs(nodes, 2), ['a', 'm']);
});

test('the demoted set is named, so the panel can say what it took', () => {
  const nodes = [hub('a', 9, 'types.ts'), hub('b', 1, 'main.ts')];
  assert.deepEqual(hubNames(nodes, rankHubs(nodes, 1)), ['types.ts']);
});

test('a name is still produced for an id that left the graph', () => {
  assert.deepEqual(hubNames([], ['gone']), ['gone']);
});

test('two demoted files with the same name are told apart', () => {
  // This repo really does demote both stores/graph.ts and types/graph.ts. A
  // list reading "graph.ts, …, graph.ts" reads as a bug and leaves the reader
  // unable to tell which one lost its edges.
  const nodes = [
    { id: 'a', name: 'graph.ts', file_path: 'ui/src/stores/graph.ts' },
    { id: 'b', name: 'graph.ts', file_path: 'ui/src/types/graph.ts' },
    { id: 'c', name: 'scope.ts', file_path: 'ui/src/stores/scope.ts' },
  ] as unknown as D3Node[];
  assert.deepEqual(hubNames(nodes, ['a', 'b', 'c']),
    ['stores/graph.ts', 'types/graph.ts', 'scope.ts']);
});

// ── file wedges (UI-103) ────────────────────────────────────────────────

test('the file wedge key is the file, normalised the one way', () => {
  assert.equal(fileWedgeKeyOf(node('a', 'ui/src/stores/graph.ts')), 'ui/src/stores/graph.ts');
  assert.equal(fileWedgeKeyOf(node('b', './ui/src/stores/graph.ts')), 'ui/src/stores/graph.ts');
  assert.equal(fileWedgeKeyOf(node('g', '')), ' ghost');
});

test('a root-level file keeps the reserved arc, with its own wedge in it', () => {
  // Left as a bare filename it would sort among the top-level directories.
  // The force still groups root files by the `''` tier, so the seed would be
  // scattering across the circle exactly what the force then drags back.
  const a = fileWedgeKeyOf(node('a', 'vite.config.ts'));
  const b = fileWedgeKeyOf(node('b', './svelte.config.js'));
  assert.ok(a.startsWith(' root/'), a);
  assert.ok(b.startsWith(' root/'), b);
  assert.notEqual(a, b);
  // Still after ghosts and before every real path, so the reserved arc holds.
  assert.ok(' ghost' < a && a < 'ui/src/stores/graph.ts');
});

test('file wedges keep a folder’s files in one arc', () => {
  // Lexicographic order over full paths is still depth-first order over the
  // tree, which is the property the whole seeder rests on — a subtree takes
  // one arc with no tree walk. Sorting the keys must not interleave a file
  // from another folder between two of this folder's.
  const keys = [
    'ui/src/stores/graph.ts', 'ui/src/components/GraphView.svelte',
    'ui/src/stores/settings.ts', 'src/parser/mod.rs',
  ].map((p) => fileWedgeKeyOf(node(p, p))).sort();
  const storesAt = keys.map((k, i) => (k.startsWith('ui/src/stores/') ? i : -1)).filter((i) => i >= 0);
  assert.deepEqual(storesAt, [storesAt[0], storesAt[0] + 1], `interleaved: ${keys.join(', ')}`);
});

test('the seeder takes the key function, and the default is the folder', () => {
  const nodes = [
    node('a', 'p/one.ts'), node('b', 'p/two.ts'), node('c', 'q/three.ts'),
  ];
  const byDefault = makeSeeder(nodes, 1600, 1000);
  const byFolder = makeSeeder(nodes, 1600, 1000, wedgeKeyOf);
  const byFile = makeSeeder(nodes, 1600, 1000, fileWedgeKeyOf);
  // The default must be the folder key, or every seeding assertion above
  // silently starts measuring a grain nobody asked for.
  assert.deepEqual(byDefault.position(nodes[0]), byFolder.position(nodes[0]));
  // And the file key has to actually reach the allocation: `p` is one wedge
  // holding two nodes at folder grain and two wedges of one at file grain, so
  // the same node cannot land in the same place.
  assert.notDeepEqual(byFolder.position(nodes[0]), byFile.position(nodes[0]));
});

// ── groupMemberIds (UI-054) ─────────────────────────────────────────────

test('hovering a node lights everything in its folder, itself included', () => {
  const a = node('a', 'ui/src/stores/one.ts');
  const b = node('b', 'ui/src/stores/two.ts');
  const c = node('c', 'ui/src/utils/three.ts');
  const ids = groupMemberIds([a, b, c], a, folderKeyOf);
  assert.deepEqual([...ids].sort(), ['a', 'b']);
});

test('hovering a ghost lights nothing rather than every other ghost', () => {
  // '' is not a group. Collecting every unrelated stdlib reference under one
  // highlight would invent a grouping that does not exist.
  const g1 = node('println', '');
  const g2 = node('Vec', '');
  const real = node('r', 'ui/src/one.ts');
  assert.equal(groupMemberIds([g1, g2, real], g1, folderKeyOf).size, 0);
});

test('a repo-root file lights its root siblings', () => {
  const a = node('a', 'vite.config.ts');
  const b = node('b', 'main.ts');
  const c = node('c', 'ui/src/one.ts');
  assert.deepEqual([...groupMemberIds([a, b, c], a, folderKeyOf)].sort(), ['a', 'b']);
});

test('a lone member lights only itself', () => {
  const a = node('a', 'ui/src/solo/one.ts');
  const b = node('b', 'ui/src/other/two.ts');
  assert.deepEqual([...groupMemberIds([a, b], a, folderKeyOf)], ['a']);
});

// ── computeFolderHulls (UI-055) ─────────────────────────────────────────

/** One tier: a region per leaf folder and nothing above it. This is the
 *  UI-055 picture, and every test written before UI-070 still describes it. */
const hullOpts = (pad?: number) => ({
  radiusOf: () => 10,
  keysOf: (n: D3Node) => { const k = folderKeyOf(n); return k === null ? [] : [k]; },
  ...(pad === undefined ? {} : { pad }),
});

/** `tiers` levels of regions, the way GraphView builds it: the whole chain
 *  for membership, the tier count as a separate limit on what is drawn. */
const nestedOpts = (tiers: number, pad?: number) => ({
  radiusOf: () => 10,
  keysOf: (n: D3Node) => {
    const k = folderKeyOf(n);
    return k === null ? [] : ancestorChainOf(k, Infinity);
  },
  tiers,
  ...(pad === undefined ? {} : { pad }),
});

/** `n` nodes spread along a line inside one folder. */
function folder(path: string, count: number, x0 = 0, y0 = 0): D3Node[] {
  return Array.from({ length: count }, (_, i) =>
    node(`${path}/f${i}`, `${path}/f${i}.ts`, x0 + i * 50, y0 + (i % 2) * 30));
}

test('one hull per folder, labelled with the folder name', () => {
  const hulls = computeFolderHulls(
    [...folder('ui/src/stores', 4), ...folder('ui/src/utils', 4, 800)], hullOpts());
  assert.equal(hulls.length, 2);
  assert.deepEqual(hulls.map((h) => h.label).sort(), ['stores', 'utils']);
  assert.deepEqual(hulls.map((h) => h.path).sort(), ['ui/src/stores', 'ui/src/utils']);
});

test('a group below the member floor gets no hull', () => {
  const hulls = computeFolderHulls(folder('ui/src/stores', MIN_HULL_MEMBERS - 1), hullOpts());
  assert.equal(hulls.length, 0);
});

test('a group at the floor does get one', () => {
  const hulls = computeFolderHulls(folder('ui/src/stores', MIN_HULL_MEMBERS), hullOpts());
  assert.equal(hulls.length, 1);
});

test('ghosts are in no region', () => {
  const ghosts = [node('println', '', 0, 0), node('Vec', '', 10, 10), node('len', '', 20, 20)];
  assert.equal(computeFolderHulls(ghosts, hullOpts()).length, 0);
});

test('the outline clears every node it encloses', () => {
  // A hull through the node centres would cut each boundary circle in half.
  // Every member must sit strictly inside the polygon, radius included.
  const nodes = folder('pkg', 6);
  const [hull] = computeFolderHulls(nodes, hullOpts(20));
  for (const n of nodes) {
    const inside = hull.points.some((p) =>
      Math.hypot(p[0] - n.x!, p[1] - n.y!) > 10);
    assert.ok(inside, `${n.id} not cleared`);
  }
  // and the polygon's extent must exceed the nodes' own extent on both axes
  const xs = hull.points.map((p) => p[0]);
  const ys = hull.points.map((p) => p[1]);
  assert.ok(Math.min(...xs) < Math.min(...nodes.map((n) => n.x!)));
  assert.ok(Math.max(...xs) > Math.max(...nodes.map((n) => n.x!)));
  assert.ok(Math.min(...ys) < Math.min(...nodes.map((n) => n.y!)));
  assert.ok(Math.max(...ys) > Math.max(...nodes.map((n) => n.y!)));
});

test('the label sits above the outline, not inside it', () => {
  const nodes = folder('pkg', 5);
  const [hull] = computeFolderHulls(nodes, hullOpts());
  const topY = Math.min(...hull.points.map((p) => p[1]));
  assert.ok(hull.labelY <= topY, `label ${hull.labelY} below hull top ${topY}`);
});

/** Every pair of region names that share space on the canvas. */
function collidingNames(hulls: ReturnType<typeof computeFolderHulls>): string[] {
  const boxes = hulls.map((h) => ({ path: h.path, ...labelBoxOf(h) }));
  const hits: string[] = [];
  for (let i = 0; i < boxes.length; i++) {
    for (let j = i + 1; j < boxes.length; j++) {
      const a = boxes[i], b = boxes[j];
      if (a.x1 <= b.x0 || a.x0 >= b.x1) continue;
      if (a.y1 <= b.y0 || a.y0 >= b.y1) continue;
      hits.push(`${a.path} × ${b.path}`);
    }
  }
  return hits;
}

test('two region names are never drawn on top of each other', () => {
  // The nesting case, which is the structural one: `src/server` holds the
  // topmost node of `src`, so both outlines pass through the same ring point
  // and both names were anchored on the same line.
  const hulls = computeFolderHulls(
    [...folder('src/server', 5), ...folder('src/output', 5, 0, 400)], nestedOpts(2));
  assert.ok(hulls.length >= 2, 'expected a parent and a child to compare');
  assert.deepEqual(collidingNames(hulls), []);
});

test('sibling regions whose tops line up keep their names apart', () => {
  // Side by side and level, close enough that the names would run together
  // horizontally. Nothing encloses anything here, so this is the same rule
  // applied without any ancestry to lean on.
  const hulls = computeFolderHulls(
    [...folder('averylongfoldername', 3), ...folder('anotherlongfoldername', 3, 60)], hullOpts());
  assert.equal(hulls.length, 2);
  assert.deepEqual(collidingNames(hulls), []);
});

test('the enclosing region\'s name is the one that moves, and it moves up', () => {
  const hulls = computeFolderHulls(
    [...folder('src/server', 5), ...folder('src/output', 5, 0, 400)], nestedOpts(2));
  const parent = hulls.find((h) => h.path === 'src')!;
  const child = hulls.find((h) => h.path === 'src/server')!;
  // A parent's name is the heading over the regions inside it, so it has to
  // end up above them rather than below.
  assert.ok(parent.labelY < child.labelY,
    `parent name at ${parent.labelY} is not above the child's at ${child.labelY}`);
});

test('a name pushed clear of another still sits outside its own outline', () => {
  // The UI-055 promise, retested after the lift: only ever upward, so a name
  // can never end up over the nodes it is describing.
  const hulls = computeFolderHulls(
    [...folder('src/server', 5), ...folder('src/output', 5, 0, 400)], nestedOpts(2));
  for (const h of hulls) {
    const topY = Math.min(...h.points.map((p) => p[1]));
    assert.ok(h.labelY <= topY, `${h.path}: label ${h.labelY} below hull top ${topY}`);
  }
});

test('a name far from every other is left where its outline puts it', () => {
  // Separation must not become a layout of its own: a region with room keeps
  // the anchor that ties its name to its shape.
  const [alone] = computeFolderHulls(folder('pkg', 5), hullOpts());
  const topY = Math.min(...alone.points.map((p) => p[1]));
  assert.equal(alone.labelY, topY - 6);
});

test('bigger regions come first, so a small one is never buried', () => {
  const hulls = computeFolderHulls(
    [...folder('small', 3), ...folder('big', 9, 500), ...folder('mid', 5, 1200)], hullOpts());
  assert.deepEqual(hulls.map((h) => h.size), [9, 5, 3]);
});

test('nodes with no position are skipped rather than placed at the origin', () => {
  const placed = folder('pkg', 4);
  const unplaced = node('pkg/ghosted', 'pkg/ghosted.ts');
  (unplaced as unknown as { x?: number }).x = undefined;
  const [hull] = computeFolderHulls([...placed, unplaced], hullOpts());
  // A node at (undefined → 0,0) would drag the outline back to the origin.
  assert.ok(Math.min(...hull.points.map((p) => p[1])) > -100);
});

test('the root folder is a real group with a readable name', () => {
  const roots = [
    node('a', 'vite.config.ts', 0, 0),
    node('b', 'main.ts', 40, 0),
    node('c', 'index.ts', 80, 0),
  ];
  const [hull] = computeFolderHulls(roots, hullOpts());
  assert.equal(hull.path, '');
  assert.equal(hull.label, '(root)');
});

test('one stray member does not drag the outline across the canvas', () => {
  // A hull is convex, so an outlier does not stretch it a little — it pulls
  // the whole polygon over everything in between. This is what turned five
  // folders into five overlapping sheets on the first build.
  const core = folder('pkg', 6);
  const stray = node('pkg/stray', 'pkg/stray.ts', 4000, 4000);
  const [tight] = computeFolderHulls(core, hullOpts());
  const [withStray] = computeFolderHulls([...core, stray], hullOpts());
  const span = (h: typeof tight) =>
    Math.max(...h.points.map((p) => p[0])) - Math.min(...h.points.map((p) => p[0]));
  assert.ok(span(withStray) < span(tight) * 2,
    `stray widened the hull from ${Math.round(span(tight))} to ${Math.round(span(withStray))}`);
});

test('the reported size is the folder, not the trimmed polygon', () => {
  const core = folder('pkg', 6);
  const stray = node('pkg/stray', 'pkg/stray.ts', 4000, 4000);
  const [hull] = computeFolderHulls([...core, stray], hullOpts());
  assert.equal(hull.size, 7);
});

test('a genuinely spread-out folder is still drawn whole', () => {
  // Trimming must not turn every loose group into a tight one — only reject
  // members far outside the group's own core.
  const spread = Array.from({ length: 8 }, (_, i) =>
    node(`p/f${i}`, `p/f${i}.ts`, i * 140, (i % 3) * 120));
  const [hull] = computeFolderHulls(spread, hullOpts());
  const width = Math.max(...hull.points.map((p) => p[0])) - Math.min(...hull.points.map((p) => p[0]));
  assert.ok(width > 900, `spread folder collapsed to ${Math.round(width)}px`);
});

test('trimming never drops a group below the member floor', () => {
  // Three nodes, one far away: trimming to the core would leave two, which
  // cannot be hulled. The group keeps all its members instead of vanishing.
  const nodes = [
    node('p/a', 'p/a.ts', 0, 0),
    node('p/b', 'p/b.ts', 20, 20),
    node('p/c', 'p/c.ts', 3000, 3000),
  ];
  const hulls = computeFolderHulls(nodes, hullOpts());
  assert.equal(hulls.length, 1);
});

test('a region full of other folders\' nodes is not drawn', () => {
  // The outline would be a lasso around the middle of the graph rather than
  // a region, and drawing it asserts a grouping the layout does not have.
  const spread = [
    node('a/1', 'a/1.ts', 0, 0),
    node('a/2', 'a/2.ts', 600, 0),
    node('a/3', 'a/3.ts', 300, 600),
  ];
  const inside = Array.from({ length: 12 }, (_, i) =>
    node(`b/${i}`, `b/${i}.ts`, 200 + (i % 4) * 60, 100 + Math.floor(i / 4) * 80));
  const hulls = computeFolderHulls([...spread, ...inside], hullOpts());
  assert.ok(!hulls.some((h) => h.path === 'a'), 'the lasso was drawn');
});

test('a region with a couple of strangers passing through survives', () => {
  const own = Array.from({ length: 10 }, (_, i) =>
    node(`a/${i}`, `a/${i}.ts`, (i % 5) * 60, Math.floor(i / 5) * 60));
  const passing = [node('b/1', 'b/1.ts', 120, 30), node('b/2', 'b/2.ts', 60, 30)];
  const hulls = computeFolderHulls([...own, ...passing], hullOpts());
  assert.ok(hulls.some((h) => h.path === 'a'), 'a real region was rejected');
});

test('padding widens the outline', () => {
  const nodes = folder('pkg', 5);
  const tight = computeFolderHulls(nodes, hullOpts(5))[0];
  const loose = computeFolderHulls(nodes, hullOpts(40))[0];
  const span = (h: typeof tight) =>
    Math.max(...h.points.map((p) => p[0])) - Math.min(...h.points.map((p) => p[0]));
  assert.ok(span(loose) > span(tight));
});

// ── nested regions (UI-070) ─────────────────────────────────────────────

/** Two subtrees, two leaf folders each, far enough apart to be separable. */
function twoSubtrees(): D3Node[] {
  return [
    ...folder('F1/SF11', 4, 0, 0),
    ...folder('F1/SF12', 4, 0, 400),
    ...folder('F2/SF21', 4, 3000, 0),
    ...folder('F2/SF22', 4, 3000, 400),
  ];
}

test('one tier is exactly the UI-055 picture', () => {
  const hulls = computeFolderHulls(twoSubtrees(), nestedOpts(1));
  assert.deepEqual(hulls.map((h) => h.path).sort(),
    ['F1/SF11', 'F1/SF12', 'F2/SF21', 'F2/SF22']);
  assert.ok(hulls.every((h) => !h.hasChildren));
});

test('a second tier outlines the subtree around the folders', () => {
  const hulls = computeFolderHulls(twoSubtrees(), nestedOpts(2));
  assert.deepEqual(hulls.map((h) => h.path).sort(),
    ['F1', 'F1/SF11', 'F1/SF12', 'F2', 'F2/SF21', 'F2/SF22']);
});

test('a folder holding both files and a subfolder is a parent at one tier', () => {
  // "One tier" is a limit on how far *above* a node's own folder an outline
  // may sit, not a promise that no region contains another. `p` holds loose
  // files and `p/sub` as well — `ui/src` on this repo — so it is genuinely
  // the region around that one, and drawing it as a sibling would be the
  // lie. What one tier forbids is a region for a folder holding no drawn
  // file of its own.
  const nodes = [...folder('p', 4), ...folder('p/sub', 4, 0, 400)];
  const hulls = computeFolderHulls(nodes, nestedOpts(1));
  const by = new Map(hulls.map((h) => [h.path, h]));
  assert.deepEqual([...by.keys()].sort(), ['p', 'p/sub']);
  assert.equal(by.get('p')!.hasChildren, true);
  // And it holds what its name says: every node beneath it, not just the
  // loose ones. A region drawn from the loose files alone could sit *inside*
  // the subfolder's region while claiming to be the folder around it.
  assert.equal(by.get('p')!.size, 8);
});

test('a folder holding no drawn file of its own gets no region at one tier', () => {
  const nodes = [...folder('p/sub', 4), ...folder('p/other', 4, 0, 400)];
  const hulls = computeFolderHulls(nodes, nestedOpts(1));
  assert.deepEqual(hulls.map((h) => h.path).sort(), ['p/other', 'p/sub']);
});

test('a parent knows it is one, and a leaf knows it is not', () => {
  const hulls = computeFolderHulls(twoSubtrees(), nestedOpts(2));
  const by = new Map(hulls.map((h) => [h.path, h]));
  assert.equal(by.get('F1')!.hasChildren, true);
  assert.equal(by.get('F1/SF11')!.hasChildren, false);
});

test('a descendant is not a foreigner inside its ancestor', () => {
  // The foreign-share guard rejected every parent outline by construction
  // before UI-070: a parent's hull is *supposed* to contain its children's
  // nodes. The guard itself is unchanged — it is the definition of foreign
  // that learned about ancestry.
  const hulls = computeFolderHulls(twoSubtrees(), nestedOpts(2));
  assert.ok(hulls.some((h) => h.path === 'F1'), 'the subtree outline was rejected');
});

test('a lasso is still rejected, tiers or no tiers', () => {
  // The self-regulating half of UI-055 has to survive. `a` is spread around
  // `b`'s dense cluster and shares no ancestor with it, so its outline is a
  // lasso round the middle of the graph and must not be drawn.
  const spread = [
    node('a/1', 'a/1.ts', 0, 0),
    node('a/2', 'a/2.ts', 600, 0),
    node('a/3', 'a/3.ts', 300, 600),
  ];
  const inside = Array.from({ length: 12 }, (_, i) =>
    node(`b/${i}`, `b/${i}.ts`, 200 + (i % 4) * 60, 100 + Math.floor(i / 4) * 80));
  const hulls = computeFolderHulls([...spread, ...inside], nestedOpts(3));
  assert.ok(!hulls.some((h) => h.path === 'a'), 'the lasso was drawn');
});

test('a parent holding exactly one region is not drawn twice', () => {
  // `F1` contains only `F1/SF11`, so its outline would be the same shape
  // under a second name — two nested rings for one set of nodes, and the
  // reader left working out which is which.
  const nodes = [...folder('F1/SF11', 5), ...folder('G/other', 5, 3000)];
  const hulls = computeFolderHulls(nodes, nestedOpts(2));
  assert.ok(!hulls.some((h) => h.path === 'F1'), 'the redundant tier was drawn');
  assert.ok(hulls.some((h) => h.path === 'F1/SF11'));
});

test('a folder holding one file does not draw that file’s outline twice', () => {
  // The file-grain form of the rule above, and the reason file grain needs no
  // new guard: `p` holds exactly what `p/one.ts` holds, so the existing
  // redundancy test marks it and the reader gets one ring, not two rings and
  // the job of working out which name goes with which.
  const oneFile = Array.from({ length: 5 }, (_, i) =>
    node(`a${i}`, 'p/one.ts', i * 50, (i % 2) * 30));
  const elsewhere = Array.from({ length: 5 }, (_, i) =>
    node(`b${i}`, 'q/two.ts', 3000 + i * 50, (i % 2) * 30));
  const hulls = computeFolderHulls([...oneFile, ...elsewhere], {
    radiusOf: () => 10,
    keysOf: (n: D3Node) => groupChainOf(n, 'file', Infinity),
    tiers: 2,
  });
  assert.ok(hulls.some((h) => h.path === 'p/one.ts'), 'the file region was not drawn');
  assert.ok(!hulls.some((h) => h.path === 'p'), 'the folder redrew its only file');
});

test('regions are painted outside-in, so no parent buries a child', () => {
  const hulls = computeFolderHulls(twoSubtrees(), nestedOpts(2));
  const at = (p: string) => hulls.findIndex((h) => h.path === p);
  assert.ok(at('F1') < at('F1/SF11'), 'the parent was painted over its child');
  assert.ok(at('F2') < at('F2/SF21'), 'the parent was painted over its child');
});

test('paint order does not flip between frames on a tie', () => {
  // Two regions of equal size swapping places would flicker, and the sort is
  // rerun on every redraw.
  const nodes = [...folder('p/aa', 4), ...folder('p/bb', 4, 3000)];
  const once = computeFolderHulls(nodes, nestedOpts(2)).map((h) => h.path);
  const twice = computeFolderHulls(nodes, nestedOpts(2)).map((h) => h.path);
  assert.deepEqual(once, twice);
  assert.deepEqual(once.filter((p) => p !== 'p'), ['p/aa', 'p/bb']);
});

test('a deeper tier than the tree has does not invent a region', () => {
  const hulls = computeFolderHulls(folder('solo', 5), nestedOpts(3));
  assert.deepEqual(hulls.map((h) => h.path), ['solo']);
});

test('ghosts are in no region at any tier', () => {
  const ghosts = [node('println', '', 0, 0), node('Vec', '', 10, 10), node('len', '', 20, 20)];
  assert.equal(computeFolderHulls(ghosts, nestedOpts(3)).length, 0);
});

// ── which region the pointer is in (UI-071) ─────────────────────────────

/** The centroid of a hull's own members — a point that is unambiguously
 *  inside it, without hand-picking coordinates that could drift with the
 *  padding. */
function centroidOf(nodes: D3Node[]): [number, number] {
  const n = nodes.length;
  return [
    nodes.reduce((s, d) => s + d.x!, 0) / n,
    nodes.reduce((s, d) => s + d.y!, 0) / n,
  ];
}

test('a point in one region reports that region', () => {
  const inside = folder('ui/src/stores', 4);
  const hulls = computeFolderHulls([...inside, ...folder('ui/src/utils', 4, 3000)], hullOpts());
  const trail = regionsAtPoint(hulls, ...centroidOf(inside));
  assert.deepEqual(trail.map((h) => h.path), ['ui/src/stores']);
});

test('empty canvas is in no region, rather than in the nearest one', () => {
  const hulls = computeFolderHulls(folder('pkg', 5), hullOpts());
  assert.deepEqual(regionsAtPoint(hulls, 9000, 9000), []);
});

test('nested regions come back parent first', () => {
  // The whole point of the trail: standing in `F1/SF11` you are also in `F1`,
  // and "you are in SF11" hides the more useful half of the sentence.
  const leaf = folder('F1/SF11', 4, 0, 0);
  const hulls = computeFolderHulls([...leaf, ...folder('F1/SF12', 4, 0, 400),
    ...folder('F2/SF21', 4, 3000, 0), ...folder('F2/SF22', 4, 3000, 400)], nestedOpts(2));
  const trail = regionsAtPoint(hulls, ...centroidOf(leaf));
  assert.deepEqual(trail.map((h) => h.path), ['F1', 'F1/SF11']);
});

test('the tightest region is the last one, and is what a focus acts on', () => {
  const leaf = folder('F1/SF11', 4, 0, 0);
  const hulls = computeFolderHulls([...leaf, ...folder('F1/SF12', 4, 0, 400),
    ...folder('F2/SF21', 4, 3000, 0), ...folder('F2/SF22', 4, 3000, 400)], nestedOpts(2));
  const trail = regionsAtPoint(hulls, ...centroidOf(leaf));
  assert.equal(tightestRegion(trail)!.path, 'F1/SF11');
  assert.equal(tightestRegion([]), null);
});

test('the part of a parent no child covers reports the parent alone', () => {
  // This is how an ancestor stays reachable at all once its middle is taken
  // by the regions inside it. `p` holds loose files as well as `p/sub`, so
  // there is such a place; it is where those loose files sit.
  const loose = folder('p', 4, 0, 0);
  const hulls = computeFolderHulls([...loose, ...folder('p/sub', 4, 0, 600)], nestedOpts(1));
  const trail = regionsAtPoint(hulls, ...centroidOf(loose));
  assert.deepEqual(trail.map((h) => h.path), ['p']);
});

test('the trail is ordered by the regions, not by the array it arrives in', () => {
  // The caller hands over its paint-order array today. A hover trail that
  // silently depended on that would break the day the drawing order changes
  // for a drawing reason.
  const leaf = folder('F1/SF11', 4, 0, 0);
  const hulls = computeFolderHulls([...leaf, ...folder('F1/SF12', 4, 0, 400),
    ...folder('F2/SF21', 4, 3000, 0), ...folder('F2/SF22', 4, 3000, 400)], nestedOpts(2));
  const trail = regionsAtPoint([...hulls].reverse(), ...centroidOf(leaf));
  assert.deepEqual(trail.map((h) => h.path), ['F1', 'F1/SF11']);
});

test('a non-finite point is in no region', () => {
  // `d3.pointer` returns NaN when the canvas has no layout box yet, and a
  // point-in-polygon test on NaN answers false for every edge crossing —
  // which is an accident, not a decision.
  const hulls = computeFolderHulls(folder('pkg', 5), hullOpts());
  assert.deepEqual(regionsAtPoint(hulls, NaN, 0), []);
});

test('two trails naming the same regions compare equal', () => {
  // What keeps a pointer move that changed nothing from re-rendering.
  const hulls = computeFolderHulls(folder('pkg', 5), hullOpts());
  assert.ok(sameRegions(hulls, [...hulls]));
  assert.ok(!sameRegions(hulls, []));
});

// ── mixed-level expansion (UI-057) and edge fidelity (UI-058) ───────────

/** Two files in one module, two entities each, with a call across them. */
function mixedGraph() {
  const mk = (id: string, file: string) => ({
    id, original_id: id, name: id, file_path: file, kind_raw: 'Function',
    language: 'TypeScript', tags: [],
  } as unknown as D3Node);
  const nodes = [
    mk('a1', 'pkg/sub/a.ts'), mk('a2', 'pkg/sub/a.ts'),
    mk('b1', 'pkg/sub/b.ts'), mk('b2', 'pkg/sub/b.ts'),
    mk('c1', 'other/c.ts'),
  ];
  const link = (s: string, t: string, kind = 'Calls') =>
    ({ source: s, target: t, kind: 'calls', kind_raw: kind, incoming_kind: 'called by', order: null } as unknown as never);
  const links = [
    link('a1', 'b1'), link('a2', 'b2'), link('a1', 'a2'), link('a1', 'c1'),
  ];
  return { nodes, links } as unknown as Parameters<typeof collapseGraph>[0];
}

test('with nothing expanded, module level is one node per directory', () => {
  const out = collapseGraph(mixedGraph(), 'folder');
  assert.deepEqual(out.nodes.map((n) => n.original_id).sort(), ['other', 'pkg/sub']);
});

test('expanding a module opens it to files and leaves the rest collapsed', () => {
  const out = collapseGraph(mixedGraph(), 'folder', new Set(['pkg/sub']));
  assert.deepEqual(out.nodes.map((n) => n.original_id).sort(),
    ['other', 'pkg/sub/a.ts', 'pkg/sub/b.ts']);
  // The unexpanded neighbour is still a single Folder circle.
  assert.equal(out.nodes.find((n) => n.original_id === 'other')?.kind_raw, 'Folder');
  assert.equal(out.nodes.find((n) => n.original_id === 'pkg/sub/a.ts')?.kind_raw, 'File');
});

test('expanding a file at file level opens it to entities', () => {
  const out = collapseGraph(mixedGraph(), 'file', new Set(['pkg/sub/a.ts']));
  const ids = out.nodes.map((n) => n.original_id).sort();
  assert.deepEqual(ids, ['a1', 'a2', 'other/c.ts', 'pkg/sub/b.ts']);
});

test('expansion opens exactly one level, not all the way down', () => {
  // An expanded module yields files, never entities — one gesture must not
  // be able to drop hundreds of nodes onto the canvas.
  const out = collapseGraph(mixedGraph(), 'folder', new Set(['pkg/sub']));
  assert.ok(!out.nodes.some((n) => n.original_id === 'a1'));
});

test('edges into a collapsed neighbour merge into one weighted dependency', () => {
  const out = collapseGraph(mixedGraph(), 'file');
  const ab = out.links.find((l) => String(l.source).includes('a_ts') && String(l.target).includes('b_ts'));
  assert.ok(ab, 'no a→b edge');
  assert.equal(ab!.kind_raw, 'DependsOn');
  assert.equal(ab!.weight, 2);
  assert.deepEqual(ab!.breakdown, { Calls: 2 });
});

test('an edge between two expanded entities keeps its real kind', () => {
  // UI-058: `DependsOn` is the right word only when a rollup is on one end.
  const out = collapseGraph(mixedGraph(), 'file', new Set(['pkg/sub/a.ts', 'pkg/sub/b.ts']));
  const a1b1 = out.links.find((l) => l.source === 'a1' && l.target === 'b1');
  assert.ok(a1b1, 'the entity-to-entity edge was dropped');
  assert.equal(a1b1!.kind_raw, 'Calls');
});

test('an edge from an expanded entity to a collapsed scope still merges', () => {
  const out = collapseGraph(mixedGraph(), 'file', new Set(['pkg/sub/a.ts']));
  const toB = out.links.find((l) => l.source === 'a1' && String(l.target).includes('b_ts'));
  assert.ok(toB, 'no a1 → b.ts edge');
  assert.equal(toB!.kind_raw, 'DependsOn');
});

test('intra-scope edges stay hidden, expanded or not', () => {
  const collapsed = collapseGraph(mixedGraph(), 'file');
  assert.ok(!collapsed.links.some((l) => l.source === l.target));
  // a1 → a2 lives inside a.ts; collapsed it is intra-file and must not draw.
  assert.equal(collapsed.links.filter((l) => String(l.source).includes('a_ts') && String(l.target).includes('a_ts')).length, 0);
  // Expanded, the same edge becomes a real entity-to-entity edge.
  const opened = collapseGraph(mixedGraph(), 'file', new Set(['pkg/sub/a.ts']));
  assert.ok(opened.links.some((l) => l.source === 'a1' && l.target === 'a2'));
});

test('expanding nothing is byte-for-byte the old behaviour', () => {
  const a = collapseGraph(mixedGraph(), 'folder');
  const b = collapseGraph(mixedGraph(), 'folder', new Set());
  assert.deepEqual(a.nodes.map((n) => n.original_id), b.nodes.map((n) => n.original_id));
  assert.deepEqual(a.links.length, b.links.length);
});

// ── folder-aware link weights (UI-102) ─────────────────────────────────

/** A link whose ends are already node objects — the shape d3 hands the
 *  strength and distance accessors, since `forceLink.initialize` resolves ids
 *  before it calls either. */
function link(source: D3Node, target: D3Node): D3Link {
  return { source, target, kind: 'calls', kind_raw: 'Calls', incoming_kind: 'called by', order: null } as unknown as D3Link;
}

const weights = () => linkFolderWeights(folderKeyOf);

test('the tree distance between two folders is what you walk, not what you share', () => {
  assert.equal(folderTreeDistance('ui/src/stores', 'ui/src/stores'), 0);
  // Siblings are two edges apart wherever they sit. The depth-normalised
  // form this replaced scored these two differently, which was the bug.
  assert.equal(folderTreeDistance('ui/src/stores', 'ui/src/components'), 2);
  assert.equal(folderTreeDistance('a', 'b'), 2);
  // Parent to child is one, not two: you only walk down.
  assert.equal(folderTreeDistance('ui', 'ui/src'), 1);
  assert.equal(folderTreeDistance('ui/src/stores', 'docs/adr'), 5);
  // The repo root is a real folder and one level above a top-level one.
  assert.equal(folderTreeDistance('', 'ui'), 1);
  assert.equal(folderTreeDistance('', ''), 0);
});

test('a ghost is infinitely far from everything, including another ghost', () => {
  // `folderKeyOf` returns null for them, and giving two unrelated stdlib
  // references a shared home is the mistake forceCohesion already refuses.
  assert.equal(folderTreeDistance(null, 'ui/src'), Infinity);
  assert.equal(folderTreeDistance(null, null), Infinity);
  assert.equal(folderKinship(null, null), 0);
});

test('kinship decays with distance up the tree and never inverts', () => {
  const same = folderKinship('ui/src/stores', 'ui/src/stores');
  const sibling = folderKinship('ui/src/stores', 'ui/src/components');
  const cousin = folderKinship('ui/src/stores', 'ui/scripts');
  const stranger = folderKinship('ui/src/stores', 'docs/adr');
  assert.equal(same, 1);
  assert.ok(same > sibling && sibling > cousin && cousin > stranger,
    `kinship is not monotone: ${same} ${sibling} ${cousin} ${stranger}`);
  assert.ok(stranger > 0, 'an unrelated pair still has a floor above zero');
});

test('a same-folder edge is exactly the pre-UI-102 force', () => {
  // The claim that makes this change safe to compare against every grouping
  // measurement taken before it: intra-folder layout does not move.
  const w = weights();
  const a = node('a', 'pkg/sub/a.ts');
  const b = node('b', 'pkg/sub/b.ts');
  const links = [link(a, b)];
  assert.equal(w.distance(links[0]), KIN_DISTANCE);
  assert.equal(KIN_DISTANCE, 120, 'the old uniform distance was 120');
  // Two nodes, one edge each: d3's default is 1 / min(1, 1).
  assert.equal(w.strength(links[0], 0, links), 1);
});

test('a crossing edge rests further out and pulls less', () => {
  const w = weights();
  const a = node('a', 'alpha/a.ts');
  const b = node('b', 'beta/b.ts');
  const links = [link(a, b)];
  assert.ok(w.distance(links[0]) > KIN_DISTANCE);
  assert.ok(w.distance(links[0]) <= STRANGER_DISTANCE);
  assert.ok(w.strength(links[0], 0, links) < 1);
  assert.ok(w.strength(links[0], 0, links) >= STRANGER_STRENGTH);
});

test("d3's degree normalisation survives being scaled", () => {
  // Supplying a `.strength()` accessor discards d3's default silently, and a
  // flat strength makes a hub receive the same full correction from each of
  // its edges. The default has to be reproduced, not replaced.
  const w = weights();
  const hub = node('hub', 'pkg/sub/hub.ts');
  const leaves = Array.from({ length: 6 }, (_, i) => node(`leaf${i}`, 'pkg/sub/l.ts'));
  const links = leaves.map((l) => link(hub, l));
  // min(deg(hub) = 6, deg(leaf) = 1) is 1, so a leaf keeps full strength…
  assert.equal(w.strength(links[0], 0, links), 1);
  // …while an edge between two well-connected nodes is divided down.
  const h2 = node('hub2', 'pkg/sub/hub2.ts');
  const both = [...leaves.map((l) => link(hub, l)), ...leaves.map((l) => link(h2, l))];
  const crossbar = link(hub, h2);
  const withBar = [...both, crossbar];
  assert.equal(w.strength(crossbar, withBar.length - 1, withBar), 1 / 7);
});

test('the degree count follows a swapped link set', () => {
  // The incremental path calls `linkForce.links(next)` on every filter
  // toggle, and d3 re-runs the accessors against the new array. A count
  // cached from the old one would price every edge off a stale graph.
  const w = weights();
  const hub = node('hub', 'pkg/sub/hub.ts');
  const leaves = Array.from({ length: 4 }, (_, i) => node(`leaf${i}`, 'pkg/sub/l.ts'));
  const dense = leaves.map((l) => link(hub, l));
  const bar = link(hub, leaves[0]);
  const denseWithBar = [...dense, bar, link(leaves[0], leaves[1])];
  const before = w.strength(bar, dense.length, denseWithBar);
  const sparse = [bar];
  const after = w.strength(bar, 0, sparse);
  assert.notEqual(before, after);
  assert.equal(after, 1, 'against a one-link set both ends have degree 1');
});

/**
 * Settle a world under the forces GraphView actually runs — link, charge,
 * collision, the weak centring pair and cohesion — with the link force
 * configured either the old way or the new one.
 *
 * This is the harness the suite was missing, and the reason UI-102 could ship
 * unnoticed under a green build: `settle()` above runs cohesion *alone*, so
 * every existing grouping assertion measures the folder force against an
 * empty field. The failure being fixed here is entirely an argument between
 * two forces, and nothing in the suite let those two forces argue.
 */
function settleAgainstLinks(nodes: D3Node[], links: D3Link[], folderAware: boolean): void {
  const w = weights();
  const linkForce = d3.forceLink<D3Node, D3Link>(links).id((d) => d.id);
  if (folderAware) linkForce.distance(w.distance).strength(w.strength);
  else linkForce.distance(KIN_DISTANCE);   // the pre-UI-102 uniform force
  d3.forceSimulation<D3Node>(nodes)
    .force('link', linkForce)
    .force('charge', d3.forceManyBody().strength(-400))
    .force('x', d3.forceX(0).strength(0.05))
    .force('y', d3.forceY(0).strength(0.05))
    .force('collision', d3.forceCollide().radius(20))
    .force('cohesion', forceFolderCohesion().strength(COHESION_STRENGTH.low))
    .stop()
    .tick(600);
}

/**
 * Four sibling folders on a ring, each a star of ten, and one leaf per folder
 * holding a single edge into the next folder's hub.
 *
 * Four rather than two, because with two folders the only place a node can be
 * towed *to* is the only other thing on the canvas, and every measurement
 * collapses into how far apart those two sit. Stars rather than chains,
 * because a chain of ten has no centre and its own members average further
 * from the centroid than a strayed node does — a world where the metric
 * cannot see the effect it exists to measure.
 *
 * The strays are leaves and their foreign neighbour is a hub, because d3
 * biases a link's correction onto the lower-degree end. That asymmetry is
 * exactly what lets one edge overrule cohesion, so a world without it would
 * be testing a failure nobody reported.
 *
 * This is the world the constants in `linkFolderWeights` were measured on.
 */
const RING_FOLDERS = 4, RING_MEMBERS = 10;

interface RingWorld { nodes: D3Node[]; links: D3Link[]; folders: D3Node[][]; strays: Set<D3Node>; }

function ringWorld(folderOf: (f: number) => string = (f) => `pkg/f${f}`): RingWorld {
  const folders: D3Node[][] = [];
  const links: D3Link[] = [];
  for (let f = 0; f < RING_FOLDERS; f++) {
    const angle = (f / RING_FOLDERS) * Math.PI * 2;
    const cx = Math.cos(angle) * 400, cy = Math.sin(angle) * 400;
    const members = Array.from({ length: RING_MEMBERS }, (_, i) =>
      node(`F${f}N${i}`, `${folderOf(f)}/n${i}.ts`, cx + (i % 5) * 25 - 50, cy + Math.floor(i / 5) * 25 - 25));
    for (let i = 1; i < RING_MEMBERS; i++) links.push(link(members[0], members[i]));
    folders.push(members);
  }
  const strays = new Set<D3Node>();
  for (let f = 0; f < RING_FOLDERS; f++) {
    const stray = folders[f][1];
    strays.add(stray);
    links.push(link(stray, folders[(f + 1) % RING_FOLDERS][0]));
  }
  return { nodes: folders.flat(), links, folders, strays };
}

/**
 * Mean distance to the own-folder centroid for nodes **with** a crossing
 * edge, over the same for nodes without one — the unit-suite twin of the
 * probe's `strayRatio`.
 *
 * A ratio rather than a distance, because the absolute number moves with
 * cohesion strength, charge and node count, and none of those are what is
 * being asserted. 1.0 means a crossing edge costs a node nothing.
 */
function strayRatio(w: RingWorld): number {
  let sSum = 0, sN = 0, pSum = 0, pN = 0;
  for (const members of w.folders) {
    const cx = members.reduce((s, n) => s + n.x!, 0) / members.length;
    const cy = members.reduce((s, n) => s + n.y!, 0) / members.length;
    for (const n of members) {
      const d = Math.hypot(n.x! - cx, n.y! - cy);
      if (w.strays.has(n)) { sSum += d; sN++; } else { pSum += d; pN++; }
    }
  }
  return (sSum / sN) / (pSum / pN);
}

test('a crossing edge no longer tows a node out of its folder', () => {
  // The report UI-102 came from, as a number, at the shipped default
  // cohesion strength — `folderCohesion` defaults to `low`, and a fix that
  // only showed at `high` would not be a fix for what was reported.
  const before = ringWorld();
  settleAgainstLinks(before.nodes, before.links, false);
  const after = ringWorld();
  settleAgainstLinks(after.nodes, after.links, true);

  const was = strayRatio(before);
  const now = strayRatio(after);
  assert.ok(was > 1.5, `the world does not reproduce the failure: strays only ${was.toFixed(2)}x out`);
  assert.ok(now < was * 0.85, `stray ratio ${was.toFixed(2)} -> ${now.toFixed(2)}`);
});

test('the folders are tightened, not merely pushed apart', () => {
  // The trap this measurement has to avoid. Lengthening crossing edges
  // spreads the canvas, and a stray ratio can improve purely because every
  // folder grew. Intra-folder distance as a share of all-pairs distance is
  // scale-invariant, so it can only improve if folders really are tighter
  // relative to the whole — the same quantity the probe's `cohesionRatio`
  // reports on a real repo.
  const share = (w: RingWorld): number => {
    const ns = w.nodes;
    let iS = 0, iN = 0, aS = 0, aN = 0;
    for (let i = 0; i < ns.length; i++) {
      for (let j = i + 1; j < ns.length; j++) {
        const d = Math.hypot(ns[i].x! - ns[j].x!, ns[i].y! - ns[j].y!);
        aS += d; aN++;
        if (folderKeyOf(ns[i]) === folderKeyOf(ns[j])) { iS += d; iN++; }
      }
    }
    return (iS / iN) / (aS / aN);
  };
  const before = ringWorld(); settleAgainstLinks(before.nodes, before.links, false);
  const after = ringWorld(); settleAgainstLinks(after.nodes, after.links, true);
  assert.ok(share(after) < share(before),
    `folder tightness went the wrong way: ${share(before).toFixed(3)} -> ${share(after).toFixed(3)}`);
});

test('an edge out of the subtree is weakened harder than one inside it', () => {
  // The claim that makes kinship continuous rather than a boolean. Same
  // world, same geometry, only the paths differ: four folders under one
  // parent, versus four unrelated top-level folders. The second set has
  // further to decay, so its strays must come home at least as well.
  const siblings = ringWorld(); settleAgainstLinks(siblings.nodes, siblings.links, true);
  const strangers = ringWorld((f) => `f${f}`); settleAgainstLinks(strangers.nodes, strangers.links, true);
  assert.ok(strayRatio(strangers) <= strayRatio(siblings) + 0.05,
    `unrelated folders held on worse than siblings: ${strayRatio(strangers).toFixed(2)} vs ${strayRatio(siblings).toFixed(2)}`);
});

test('a graph with no crossing edges settles identically', () => {
  // The safety claim. If the new accessors moved a single-folder world at
  // all, every measurement taken before UI-102 would be incomparable with
  // every measurement taken after it.
  const world = () => {
    const ns = Array.from({ length: 8 }, (_, i) => node(`n${i}`, 'pkg/only/f.ts', i * 30 - 100, i * 20 - 80));
    const ls = ns.slice(1).map((n) => link(ns[0], n));
    return { ns, ls };
  };
  const a = world(); settleAgainstLinks(a.ns, a.ls, false);
  const b = world(); settleAgainstLinks(b.ns, b.ls, true);
  for (let i = 0; i < a.ns.length; i++) {
    assert.ok(Math.abs(a.ns[i].x! - b.ns[i].x!) < 1e-9 && Math.abs(a.ns[i].y! - b.ns[i].y!) < 1e-9,
      `node ${i} moved: (${a.ns[i].x}, ${a.ns[i].y}) vs (${b.ns[i].x}, ${b.ns[i].y})`);
  }
});

test('the graph still hangs together', () => {
  // The counterweight to every assertion above. Charge is -400 and the only
  // other restraint is the 0.05 centring pair, so a crossing edge worth
  // nothing lets the subtrees drift until auto-fit hits its zoom floor —
  // UI-022's failure, reached from the other direction. Measured, the canvas
  // grows about a fifth; the guard is set well outside that so it catches a
  // runaway rather than re-asserting the tuning.
  const spanOf = (w: RingWorld) => {
    const xs = w.nodes.map((n) => n.x!), ys = w.nodes.map((n) => n.y!);
    return Math.max(Math.max(...xs) - Math.min(...xs), Math.max(...ys) - Math.min(...ys));
  };
  const before = ringWorld(); settleAgainstLinks(before.nodes, before.links, false);
  const after = ringWorld(); settleAgainstLinks(after.nodes, after.links, true);
  assert.ok(spanOf(after) < spanOf(before) * 1.5,
    `the graph flew apart: ${Math.round(spanOf(before))}px -> ${Math.round(spanOf(after))}px`);
});
