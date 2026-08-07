/**
 * UI-091 — an unmeasured coupling count must not read as a decoupled one.
 *
 * Every claim below is some version of the same one: `0` is a measurement,
 * and a scope whose only relationships are references was never measured, so
 * it may not borrow the number that means "clean".
 *
 *   npm run test:coupling
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { couplingCell } from '../src/viewmodels/scopeCoupling.ts';

test('a measured zero stays a zero', () => {
  // The whole point of the change is that this case survives it. A scope
  // with no edges of any kind has earned its zero.
  const cell = couplingCell(0, 0, 'out');
  assert.equal(cell.text, '0');
  assert.equal(cell.unmeasured, false);
  assert.equal(cell.tip, '');
});

test('a scope with only reference links reports an em dash', () => {
  const cell = couplingCell(0, 3, 'out');
  assert.equal(cell.text, '—');
  assert.equal(cell.unmeasured, true);
  assert.match(cell.tip, /Not measured/);
  assert.match(cell.tip, /3 reference links/);
});

test('the tip says which side the references are on', () => {
  assert.match(couplingCell(0, 1, 'in').tip, /pointing at this scope/);
  assert.match(couplingCell(0, 1, 'out').tip, /leaving this scope/);
});

test('one reference is a link, not links', () => {
  assert.match(couplingCell(0, 1, 'in').tip, /1 reference link /);
});

test('real coupling is reported as measured even when references exist too', () => {
  // A mixed repo: a folder of code that is also linked from the docs. The
  // count is a measurement here, and annotating it would be noise.
  const cell = couplingCell(4, 9, 'in');
  assert.equal(cell.text, '4');
  assert.equal(cell.unmeasured, false);
  assert.equal(cell.tip, '');
});

test('a negative or absent reference count never triggers the dash', () => {
  // `?? 0` at the call sites means an older graph JSON arrives as 0 here.
  // It must fall through to the plain number, not to "not measured".
  assert.equal(couplingCell(0, 0, 'in').text, '0');
  assert.equal(couplingCell(0, -1, 'in').text, '0');
});
