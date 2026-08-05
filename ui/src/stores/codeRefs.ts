/**
 * Elevator code-reference resolution — the bridge between the `.elv`
 * spec layer and the code it declares.
 *
 * A `.elv` entity declares `cr: src/parser/` ("this Feature is
 * implemented there"). `transform.ts` parses those into `D3Node.codeRefs`
 * (UI-026); this module answers the two questions the visualizer needs
 * on top of them:
 *
 *  - **Who claims this file?** (UI-029) — the inverted `path → entities`
 *    lookup, resolving through ancestor folders so a file is claimed by
 *    an entity naming its directory, not only by an exact-path match.
 *  - **Does this ref still point at anything?** (UI-030) — a ref
 *    matching no file in the tree is drift.
 *
 * Both mirror rules that already exist on the Rust side —
 * `elevator_code_map.rs` for the inversion and its duplicate marker,
 * `elevator_drift.rs` for anchor verification — so the UI and the CLI
 * agree about the same spec.
 *
 * **Resolution runs against the full graph, never the scoped one.** A
 * ref that is merely outside the current visual scope is not drift, and
 * resolving against the filtered set would flag half the spec every time
 * the user narrows.
 */

import { derived, get } from 'svelte/store';
import type { D3Node, GraphData } from '../types/graph';
import { fullGraphDataStore, setScopes, addScopes } from './scope';
import { graphData, selectedNode } from './graph';
import {
  normalizeRefPath as normalize,
  buildPathUniverse,
  pathClaims,
  bySpecificity,
} from '../utils/refPaths';

/** One spec entity's claim over a code path. */
export interface CodeRefClaim {
  /** The `.elv` entity declaring the reference. */
  node: D3Node;
  /** Layer partition from `cr.<tag>:`; empty for a bare `cr:`. */
  tag: string;
  /** The declared path, normalized. May be an ancestor folder of the
   *  file being asked about. */
  path: string;
  /** Another entity declares this same path under the same tag —
   *  the "two names for one piece of code" signal. Mirrors
   *  `has_duplicate_kind` in `elevator_code_map.rs`; claims differing
   *  only by tag (`cr.fe` vs `cr.be`) are legitimate and not marked. */
  duplicate: boolean;
}

/** Whether a declared code reference still points at real code. */
export type CodeRefStatus = 'resolved' | 'unresolved';

/**
 * Path arithmetic lives in `utils/refPaths.ts` so it can be tested without
 * booting the store graph. Re-exported here because this module is the
 * public face of code references.
 */
export { normalizeRefPath } from '../utils/refPaths';

/** Entities that can claim code by declaring a path.
 *
 *  Two layers do. Elevator entities carry the `elevator` tag from the
 *  parser — the same predicate `elevator_code_map.rs` filters on. Markdown
 *  notes carry `markdown` and reach here for the same reason: a document
 *  linking to `src/parser/mod.rs` is making the identical claim a `cr:`
 *  makes ("this is the code I am about"), so it gets the identical
 *  treatment — the panel listing, the re-scope, the reverse lookup, and
 *  drift when the path stops resolving. ADR 0005 decided that pairing is
 *  expressed as scope rather than as a visual channel; nothing about that
 *  reasoning is specific to `.elv`. */
export function isSpecEntity(node: D3Node): boolean {
  const tags = node.tags;
  if (!tags) return false;
  return tags.includes('elevator') || tags.includes('markdown');
}

/**
 * The resolved index. Built once per full-graph load and shared by
 * every consumer — the claim lookup and the drift check read the same
 * data so they can never disagree about a path.
 */
export interface CodeRefIndex {
  /** True when the loaded graph has no Elevator layer at all, so
   *  consumers can stay silent rather than rendering empty sections. */
  readonly empty: boolean;
  /** Spec entities claiming this file, most-specific path first. */
  claimsFor(filePath: string): CodeRefClaim[];
  /** False when a declared path matches nothing in the tree — drift. */
  resolves(path: string): boolean;
}

const EMPTY_INDEX: CodeRefIndex = {
  empty: true,
  claimsFor: () => [],
  resolves: () => true,
};

function buildIndex(full: GraphData | null): CodeRefIndex {
  if (!full || full.nodes.length === 0) return EMPTY_INDEX;

  // Collect every declared reference, grouped by normalized path so the
  // duplicate rule can be applied per (path, tag).
  const byPath = new Map<string, CodeRefClaim[]>();
  for (const node of full.nodes) {
    if (!isSpecEntity(node)) continue;
    for (const ref of node.codeRefs ?? []) {
      const path = normalize(ref.path);
      if (!path) continue;
      const claims = byPath.get(path) ?? [];
      claims.push({ node, tag: ref.tag, path, duplicate: false });
      byPath.set(path, claims);
    }
  }

  if (byPath.size === 0) return EMPTY_INDEX;

  // Same path claimed twice under the same tag is the dedup signal;
  // the same path under different tags is how a shared resource is
  // legitimately referenced from two layers.
  for (const claims of byPath.values()) {
    const seen = new Set<string>();
    const duplicated = new Set<string>();
    for (const claim of claims) {
      if (seen.has(claim.tag)) duplicated.add(claim.tag);
      seen.add(claim.tag);
    }
    for (const claim of claims) {
      claim.duplicate = duplicated.has(claim.tag);
    }
  }

  const universe = buildPathUniverse(full.nodes.map((n) => n.file_path));
  const declaredPaths = bySpecificity(byPath.keys());
  const claimCache = new Map<string, CodeRefClaim[]>();

  return {
    empty: false,
    claimsFor(filePath: string): CodeRefClaim[] {
      const subject = normalize(filePath);
      if (!subject) return [];
      const cached = claimCache.get(subject);
      if (cached) return cached;
      const out: CodeRefClaim[] = [];
      for (const declared of declaredPaths) {
        if (pathClaims(declared, subject)) {
          out.push(...(byPath.get(declared) ?? []));
        }
      }
      claimCache.set(subject, out);
      return out;
    },
    resolves(path: string): boolean {
      return universe.has(normalize(path));
    },
  };
}

/**
 * Derived from the *unfiltered* graph on purpose — see the module
 * header. `fullGraphDataStore` is written once per fetch, so this
 * rebuilds on re-analysis and not on every scope change.
 */
export const codeRefIndex = derived(fullGraphDataStore, ($full) => buildIndex($full));

/**
 * The two conditions UI-030 keeps apart. A Feature that never declared
 * a `cr:` is *unanchored* — nobody said where it lives. One whose refs
 * point at nothing is *stale* — someone said, and the code moved. They
 * ask for different fixes, so they must not collapse into one badge.
 */
export type SpecAnchorState = 'anchored' | 'unanchored' | 'stale';

/**
 * Scope the canvas to the code a spec entity declares (UI-028).
 *
 * Pairing is expressed as scope rather than as a new visual channel —
 * see ADR 0005 (elevator/code pairing). The
 * entity's own `.elv` file joins the selection so narrowing to the
 * code doesn't leave the concept that sent you there off-canvas; the
 * Elevator closure pass in `filterToSelection` then keeps its parents
 * and attached Concepts with it.
 *
 * `cr:` values are path prefixes and `setScopes` selects by path
 * prefix, so there is no pairing-specific matching here: the declared
 * paths go through the same door the scope tree uses.
 */
export async function showImplementingCode(node: D3Node): Promise<void> {
  const refs = node.codeRefs ?? [];
  if (refs.length === 0) return;
  const scopes = new Set<string>();
  for (const ref of refs) {
    const path = normalize(ref.path);
    if (path) scopes.add(path);
  }
  if (scopes.size === 0) return;
  // Keep the originating entity visible alongside its code.
  if (node.file_path) scopes.add(node.file_path);
  await setScopes([...scopes]);
}

/**
 * Reveal the spec entity behind a claim (UI-029's return leg).
 *
 * The spec lives in a `.elv` file, so when the user is scoped to code
 * the claiming entity is usually out of the rendered graph. Widening
 * the selection to *include* its file — rather than replacing the
 * selection with it — is the non-destructive choice: the code the user
 * was reading stays on the canvas next to the concept that claims it,
 * and one deselect undoes it.
 */
export async function revealSpecEntity(node: D3Node): Promise<void> {
  const present = get(graphData).nodes.find((n) => n.id === node.id);
  if (present) {
    selectedNode.set(present);
    return;
  }
  if (!node.file_path) return;
  // Append rather than rebuild: `setScopes` would flatten the current scope
  // into a fresh all-includes list and drop any exclusion the user had, which
  // is a destructive way to answer "also show me this file".
  await addScopes([node.file_path]);
  // Re-find after the re-render: `filterToSelection` returns fresh
  // objects, so the pre-scope node identity is not the one on canvas.
  const revealed = get(graphData).nodes.find((n) => n.id === node.id);
  if (revealed) selectedNode.set(revealed);
}

export function anchorState(node: D3Node, index: CodeRefIndex): SpecAnchorState {
  if (!isSpecEntity(node)) return 'anchored';
  const refs = node.codeRefs ?? [];
  if (refs.length === 0) return 'unanchored';
  return refs.every((ref) => index.resolves(ref.path)) ? 'anchored' : 'stale';
}
