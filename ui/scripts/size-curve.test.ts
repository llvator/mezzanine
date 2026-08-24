/**
 * Unit tests for the node-size curves (UI-106).
 *
 * What makes this worth a suite rather than a look at the canvas: each curve
 * is a pair — `f` and its inverse — and nothing on screen tells you the pair
 * has drifted. A wrong constant in `invert` draws a *perfectly plausible*
 * legend: three dots of sensible sizes with three numbers next to them that
 * are simply not the values those dots stand for. The picture stays
 * convincing while the legend lies, which is the failure mode the round-trip
 * test below exists to catch.
 *
 * The other half is the endpoints. Every curve must map 0 → 0 and 1 → 1, or
 * "the largest node" stops meaning R_MAX and the size scale silently changes
 * meaning when the curve changes.
 *
 * Same zero-dependency setup as the sibling suites, and the same
 * extensionless-import trap: `sizeCurve` is its own module (like
 * `linkDegrees`) precisely so this file can reach it without pulling
 * `stores/quality` in behind `nodeEncoding`.
 *
 *   npm run test:sizecurve
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  SIZE_CURVES,
  SIZE_CURVE_IDS,
  sizeCurveDef,
  clampBoost,
  SIZE_BOOST_MIN,
  SIZE_BOOST_MAX,
  SIZE_BOOST_DEFAULT,
  clampBins,
  quantiseFraction,
  binEdges,
  displayEdges,
  SIZE_BINS_OFF,
  SIZE_BINS_MIN,
  SIZE_BINS_MAX,
} from '../src/viewmodels/sizeCurve.ts';

/** Fractions of the value range, dense enough to catch a non-monotone patch. */
const SAMPLES = Array.from({ length: 21 }, (_, i) => i / 20);

test('every curve pins both endpoints', () => {
  // The contract that lets the curve be a free choice: whichever is selected,
  // the smallest value still draws at R_MIN and the largest at R_MAX. A curve
  // that returned 0.98 at u=1 would quietly shrink every graph's biggest node.
  for (const c of SIZE_CURVES) {
    assert.equal(c.f(0), 0, `${c.id} f(0)`);
    assert.equal(c.f(1), 1, `${c.id} f(1)`);
  }
});

test('every curve is monotone increasing', () => {
  // Size has to stay an ordering. A curve that dipped anywhere would draw a
  // bigger file as a smaller circle — worse than no encoding at all, because
  // it reads as a fact.
  for (const c of SIZE_CURVES) {
    for (let i = 1; i < SAMPLES.length; i++) {
      assert.ok(
        c.f(SAMPLES[i]) > c.f(SAMPLES[i - 1]),
        `${c.id} not increasing at u=${SAMPLES[i]}`,
      );
    }
  }
});

test('invert round-trips f for every curve', () => {
  // The legend prints `invert(radiusFraction)`; the canvas draws
  // `f(valueFraction)`. If these two disagree the legend describes an
  // encoding the canvas is not using.
  for (const c of SIZE_CURVES) {
    for (const u of SAMPLES) {
      const back = c.invert(c.f(u));
      assert.ok(Math.abs(back - u) < 1e-9, `${c.id}: invert(f(${u})) = ${back}`);
    }
  }
});

test('out-of-range input is clamped, not extrapolated', () => {
  // `radiusFor` already clamps the value to the domain, so this is belt and
  // braces — but a NaN metric reaching a curve must not become a NaN radius,
  // which SVG renders by dropping the circle entirely.
  for (const c of SIZE_CURVES) {
    assert.equal(c.f(-1), 0, `${c.id} f(-1)`);
    assert.equal(c.f(2), 1, `${c.id} f(2)`);
    assert.equal(c.f(NaN), 0, `${c.id} f(NaN)`);
    assert.ok(c.invert(-1) >= 0 && c.invert(2) <= 1, `${c.id} invert out of range`);
  }
});

test('area is the default and reproduces the pre-UI-106 legend samples', () => {
  // The legend samples the ramp at 0.2 and 0.6325 of the RADIUS range. Under
  // the default curve those must invert to 0.04 and 0.40 of the value range —
  // the two fractions `legendStops` hardcoded before the curve was a choice.
  // This is the test that the control's default changed nothing.
  const area = sizeCurveDef('area');
  assert.equal(SIZE_CURVES[0].id, 'area');
  assert.ok(Math.abs(area.invert(0.2) - 0.04) < 1e-9);
  assert.ok(Math.abs(area.invert(0.6325) - 0.4) < 1e-3);
});

test('an unknown curve id falls back to area rather than throwing', () => {
  // A curve is persisted in localStorage and can outlive its own id across a
  // release. The store validates on load; this is the second line of defence,
  // because a throw here takes the whole encoding — and so the canvas — down.
  assert.equal(sizeCurveDef('nonsense' as never).id, 'area');
});

test('log spreads the small end and exponential compresses it', () => {
  // The reason both exist. On a skewed repo most nodes sit near u=0.05; the
  // question a reader is asking is whether those nodes can be told apart.
  const area = sizeCurveDef('area');
  const log = sizeCurveDef('log');
  const exp = sizeCurveDef('exponential');
  const linear = sizeCurveDef('linear');

  const u = 0.05;
  assert.ok(log.f(u) > area.f(u), 'log should lift the small end above area');
  assert.ok(area.f(u) > linear.f(u), 'area should lift it above linear');
  assert.ok(exp.f(u) < linear.f(u), 'exponential should push it below linear');

  // And the discrimination that buys: two small files 1% of the range apart
  // are separated by more radius under log than under area.
  const gapLog = log.f(0.06) - log.f(0.05);
  const gapArea = area.f(0.06) - area.f(0.05);
  assert.ok(gapLog > gapArea, `log gap ${gapLog} should exceed area gap ${gapArea}`);
});

test('the s-curve spreads the middle and flattens both ends', () => {
  const sig = sizeCurveDef('sigmoid');
  const linear = sizeCurveDef('linear');

  // Steeper than linear across the middle…
  const mid = sig.f(0.6) - sig.f(0.4);
  assert.ok(mid > linear.f(0.6) - linear.f(0.4), `middle gap ${mid} not expanded`);
  // …and flatter than linear at both ends. Both, deliberately: an S that only
  // flattened the top would be a log curve with extra steps.
  assert.ok(sig.f(0.1) - sig.f(0) < 0.1, 'bottom end not flattened');
  assert.ok(sig.f(1) - sig.f(0.9) < 0.1, 'top end not flattened');
});

test('every declared id resolves to exactly one curve', () => {
  // `SIZE_CURVE_IDS` is what the settings store validates persisted values
  // against, so a curve missing from it is a curve the UI can offer and then
  // refuse to restore on reload.
  assert.equal(SIZE_CURVE_IDS.length, SIZE_CURVES.length);
  assert.equal(new Set(SIZE_CURVE_IDS).size, SIZE_CURVES.length);
  for (const id of SIZE_CURVE_IDS) assert.equal(sizeCurveDef(id).id, id);
});

// ── Size groups (UI-110) ─────────────────────────────────────────────────

test('continuous is the default and passes the fraction through untouched', () => {
  // The mode every earlier measurement is of. `SIZE_BINS_OFF` must be a
  // no-op, not "one group" or "many groups" — a rounding step that ran at ×1
  // would quietly change the shipped picture.
  for (const u of SAMPLES) {
    assert.equal(quantiseFraction(u, SIZE_BINS_OFF), u);
  }
});

test('N groups produce exactly N evenly spaced radii', () => {
  // The property the whole feature rests on: consecutive groups differ by a
  // constant slice of the radius range, so the pixel gap between them is
  // span/(N-1) and the scale control is what widens it.
  for (let n = SIZE_BINS_MIN; n <= SIZE_BINS_MAX; n++) {
    const dense = Array.from({ length: 401 }, (_, i) => i / 400);
    const levels = [...new Set(dense.map((u) => quantiseFraction(u, n)))].sort((a, b) => a - b);
    assert.equal(levels.length, n, `${n} groups produced ${levels.length} radii`);
    for (let i = 1; i < levels.length; i++) {
      const gap = levels[i] - levels[i - 1];
      assert.ok(Math.abs(gap - 1 / (n - 1)) < 1e-9, `${n} groups: uneven gap ${gap}`);
    }
  }
});

test('grouping keeps both endpoints, so the ends of the scale never move', () => {
  // Same contract the curves hold. `Math.floor` would strand the largest node
  // alone in the top group on every non-uniform graph; rounding keeps 1 → 1.
  for (let n = SIZE_BINS_MIN; n <= SIZE_BINS_MAX; n++) {
    assert.equal(quantiseFraction(0, n), 0, `${n} groups, floor`);
    assert.equal(quantiseFraction(1, n), 1, `${n} groups, ceiling`);
  }
});

test('group count is clamped, and anything unusable falls back to continuous', () => {
  assert.equal(clampBins(6), 6);
  assert.equal(clampBins(99), SIZE_BINS_MAX);
  // One "group" is not a grouping — it is every node at one size, which says
  // nothing. Below the floor means continuous, not a degenerate single class.
  assert.equal(clampBins(1), SIZE_BINS_OFF);
  assert.equal(clampBins(0), SIZE_BINS_OFF);
  assert.equal(clampBins(-4), SIZE_BINS_OFF);
  assert.equal(clampBins(NaN), SIZE_BINS_OFF);
  // localStorage hands back strings; `Number('4.4')` is what a hand-edit or a
  // future migration could produce, and a fractional group count would make
  // `steps` fractional and every radius irrational.
  assert.equal(clampBins(4.4), 4);
});

test('the group edges are where the radius actually switches', () => {
  // The legend prints these. If they drifted from `quantiseFraction`'s own
  // midpoints the legend would name a boundary the canvas does not draw —
  // the same class of lie as a wrong inverse, one level up.
  for (const c of SIZE_CURVES) {
    for (let n = SIZE_BINS_MIN; n <= SIZE_BINS_MAX; n++) {
      const edges = binEdges(c, n);
      assert.equal(edges.length, n - 1, `${c.id}/${n}: edge count`);
      for (let i = 1; i < edges.length; i++) {
        assert.ok(edges[i] > edges[i - 1], `${c.id}/${n}: edges not ascending`);
      }
      for (const [i, e] of edges.entries()) {
        // A hair either side of an edge must land in different groups, and in
        // the two groups the edge divides.
        const below = quantiseFraction(c.f(e - 1e-6), n) * (n - 1);
        const above = quantiseFraction(c.f(e + 1e-6), n) * (n - 1);
        assert.equal(Math.round(below), i, `${c.id}/${n}: below edge ${i}`);
        assert.equal(Math.round(above), i + 1, `${c.id}/${n}: above edge ${i}`);
      }
    }
  }
});

test('printed group edges never collapse onto one number', () => {
  // The defect this function exists for, caught in the browser: at 8 groups
  // on this repo's 2,520-line domain the raw edges 1,041 and 1,556 both round
  // to 1,000 on the 1/2/5 "nice number" ladder, and the legend printed
  // `1,000–1,000` — a band nothing can be in, beside a radius the canvas was
  // definitely drawing.
  const raw = binEdges(sizeCurveDef('area'), 8).map((f) => f * 2520);
  const shown = displayEdges(raw);
  assert.equal(shown.length, raw.length);
  for (let i = 1; i < shown.length; i++) {
    assert.ok(shown[i] > shown[i - 1], `edge ${i} (${shown[i]}) not above ${shown[i - 1]}`);
  }
  // Integers on a domain this size — decimals are for narrow metrics.
  assert.ok(shown.every((v) => Number.isInteger(v)), `${shown} should be whole lines`);
});

test('a narrow domain gets the decimals it needs, not a repeated integer', () => {
  // PageRank in ×10⁻³, or a graph whose largest file is 3 lines: rounding to
  // whole units would print the same boundary several times over.
  const shown = displayEdges(binEdges(sizeCurveDef('area'), 6).map((f) => f * 3));
  for (let i = 1; i < shown.length; i++) {
    assert.ok(shown[i] > shown[i - 1], `narrow domain: ${shown[i]} after ${shown[i - 1]}`);
  }
});

test('the curve decides where the group boundaries land', () => {
  // Grouping does not overrule the curve, it inherits it — which is what
  // makes the boundaries automatic. Under log the first edge sits far down
  // the value range (small files get their own groups); under exponential it
  // sits high (only the largest are separated).
  const edgeOf = (id: 'log' | 'area' | 'exponential') => binEdges(sizeCurveDef(id), 6)[0];
  assert.ok(edgeOf('log') < edgeOf('area'), 'log should push the first edge down');
  assert.ok(edgeOf('exponential') > edgeOf('area'), 'exponential should push it up');
});

test('the size scale is bounded and survives a corrupt stored value', () => {
  // ×0 would draw a graph of invisible points and ×50 one node the size of
  // the viewport; both are reachable by hand-editing localStorage, which is
  // where this value comes from.
  assert.equal(clampBoost(1), 1);
  assert.equal(clampBoost(0), SIZE_BOOST_MIN);
  assert.equal(clampBoost(99), SIZE_BOOST_MAX);
  assert.equal(clampBoost(-3), SIZE_BOOST_MIN);
  // `Number('')` is 0 and `Number('abc')` is NaN — the store passes both
  // straight through, so NaN has to land on the default rather than on the
  // floor, which would look like a deliberate "smallest" setting.
  assert.equal(clampBoost(NaN), SIZE_BOOST_DEFAULT);
  assert.equal(clampBoost(Infinity), SIZE_BOOST_DEFAULT);
});
