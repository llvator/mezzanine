/**
 * Where the reader is, as an address (UI-095).
 *
 * The wayback (UI-092) named where they *were*; this names where they *are*.
 * Two axes, two controls: the arrows walk a sequence, these crumbs walk a
 * hierarchy, and neither pretends to be the other. A back stack of
 * `ui` → a committed search → a marked set has no ancestry in it at all, so
 * rendering history as `a › b › c` would promise a containment that is not
 * there.
 *
 * The address is derived from the scope rules and nothing else — no store, no
 * DOM — which is what lets the whole thing be unit-tested (`npm run
 * test:crumbs`) and what keeps the one interesting decision honest: a scope is
 * not always a path, and this module says so rather than guessing.
 *
 * ADR 0009's rule list is richer than an address can be. Three shapes come
 * out of it:
 *
 *   empty   — nothing selected. Not "the repo": an empty scope draws nothing,
 *             and a crumb reading `repo` would claim the opposite.
 *   path    — exactly one include, and it is a plain prefix. Climbable.
 *   opaque  — anything else: several paths (the marked-set drill), or a glob.
 *             It has no single address and is not offered one.
 *
 * Exclusions never change *where* you are, only what is filtered out of it,
 * so they leave the crumbs alone and set `filtered` — enough for the strip to
 * avoid claiming a folder is shown whole when part of it is not.
 */

import type { ScopeRule } from '../utils/scopeRules';

/** One step of the address. */
export interface Crumb {
  /** What focusing this crumb scopes to. `''` is the whole repo — a real
   *  value here, the same one `setScopes([''])` already means. */
  path: string;
  /** The segment's own name. The root has none of its own, so it is named. */
  label: string;
  /** The deepest crumb: where the reader is standing. Not a link — there is
   *  nowhere for it to go. */
  current: boolean;
}

export type ScopeAddress =
  | { kind: 'empty' }
  | { kind: 'path'; crumbs: Crumb[]; filtered: boolean }
  | { kind: 'opaque'; label: string; filtered: boolean };

/** What the root crumb is called. Not `/` — this is a repository, and the
 *  analysed root is not the reader's filesystem root. */
export const ROOT_LABEL = 'repo';

/** The address of a scope. */
export function scopeAddress(rules: ScopeRule[]): ScopeAddress {
  const includes = rules.filter((r) => !r.negate);
  const filtered = rules.some((r) => r.negate);
  if (includes.length === 0) return { kind: 'empty' };
  if (includes.length > 1) {
    return { kind: 'opaque', label: `${includes.length} paths`, filtered };
  }
  const only = includes[0].pattern;
  // A glob is a rule, not a place. `ui/**/*.test.ts` has no folder to climb
  // to, and picking its longest literal prefix would name a folder the reader
  // is not looking at the whole of.
  if (only.includes('*')) return { kind: 'opaque', label: only, filtered };
  return { kind: 'path', crumbs: crumbsFor(only), filtered };
}

/** `ui/src/stores` → repo › ui › src › stores, each carrying the path that
 *  focusing it would scope to. */
export function crumbsFor(path: string): Crumb[] {
  const segments = path.split('/').filter((s) => s !== '');
  const crumbs: Crumb[] = [{ path: '', label: ROOT_LABEL, current: segments.length === 0 }];
  let acc = '';
  segments.forEach((segment, i) => {
    acc = acc === '' ? segment : `${acc}/${segment}`;
    crumbs.push({ path: acc, label: segment, current: i === segments.length - 1 });
  });
  return crumbs;
}

/** How many crumbs fit before the strip starts competing with the counts. */
export const MAX_CRUMBS = 5;

/**
 * Drop crumbs from the *left* when there are too many.
 *
 * Which end to cut is the whole decision. The deepest crumbs are the ones
 * that say where you are — `… › viewmodels › savedViews` is still an answer,
 * `repo › ui › src › …` is not — and the ancestors that go are the ones the
 * reader is least likely to jump to, because the interesting climb after a
 * deep drill is one or two levels, not back to the root.
 *
 * The root is kept regardless: it is the one crumb whose destination the
 * reader may genuinely want from anywhere, and it costs four characters.
 */
export function elideCrumbs(crumbs: Crumb[], max = MAX_CRUMBS): { hidden: Crumb[]; shown: Crumb[] } {
  if (crumbs.length <= max) return { hidden: [], shown: crumbs };
  const [root, ...rest] = crumbs;
  const keep = max - 1;
  return { hidden: rest.slice(0, rest.length - keep), shown: [root, ...rest.slice(-keep)] };
}

/** What the `…` says it is hiding, on hover. */
export function hiddenTitle(hidden: Crumb[]): string {
  if (hidden.length === 0) return '';
  return `${hidden.length} more level${hidden.length > 1 ? 's' : ''}: ${hidden.map((c) => c.label).join(' › ')}`;
}

/** What a climbable crumb promises. Names the destination, like the wayback's
 *  arrows do — the label alone is a folder name, not a gesture. */
export function crumbTitle(crumb: Crumb): string {
  if (crumb.current) return `You are here: ${crumb.path === '' ? 'the whole repo' : crumb.path}`;
  return crumb.path === '' ? 'Focus the whole repo' : `Focus ${crumb.path}`;
}
