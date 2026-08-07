/**
 * What the spec says about a region on the canvas.
 *
 * A region is a folder, and a folder has no metadata of its own — which is
 * the usual argument for teaching a spec language about directories. Elevator
 * does not need teaching: `cr:` is allowed on every entity kind, takes a
 * folder path as readily as a file, and is folder-granular throughout real
 * specs (`cr: "src/parser/elevator/"`, `cr: "src/mcp/"`, `cr: "ui/"`). A
 * description attached to a folder is therefore already expressible, and what
 * was missing is this lookup rather than a construct.
 *
 * The join is `specNodesClaiming`, unchanged and shared with the code→spec
 * highlight, so the region card and the spec pane can never disagree about
 * who owns a path. Handing it a *folder* path works without modification:
 * `pathClaims` is separator-aware string arithmetic over normalized paths and
 * has no opinion about whether either side is a file.
 *
 * Nothing here infers a binding. A region with no claim reports none — which
 * is a fact worth surfacing rather than a blank to fill in, since it names a
 * folder no declared feature owns. Guessing by name similarity would be the
 * "smarter matcher" ADR 0005 rejected in favour of a better ref.
 */

// Suffixed imports: this module is reached from `scripts/spec-graph.test.ts`,
// and bare Node does not resolve an extensionless specifier the way Vite
// does. Same rule `specGraph.ts` follows for the same reason.
import { normalizeRefPath, pathClaims, bySpecificity } from '../utils/refPaths.ts';
import { specNodesClaiming, type SpecGraph } from './specGraph.ts';

export interface RegionSpecClaim {
  /** The claiming entity, for anything that wants to select or reveal it. */
  id: string;
  name: string;
  /** `Feature`, `Category`, … — shown, because "Feature grouping" reads very
   *  differently from "Concept grouping" and the name alone hides which. */
  kind: string;
  /** The `d:` field, or null when the entity has none. */
  description: string | null;
  /** The `cr:` path that matched. */
  claimPath: string;
  /**
   * Whether that ref names this exact folder.
   *
   * The difference is worth showing. `cr: "ui/"` genuinely claims
   * `ui/src/stores`, but the description it carries is about `ui` — reading
   * it as a description *of* `stores` would be the card putting words in the
   * author's mouth. Inherited claims are labelled with the path they came
   * from instead.
   */
  exact: boolean;
}

/**
 * The spec entity claiming `path`, most specific first, or null.
 *
 * `documentationOf` is injected rather than imported: the descriptions live
 * in the `/api/details` sidecar, not in the graph payload, and a viewmodel
 * that fetched them could not be tested without a server. It also keeps this
 * honest about a description that has not loaded yet — that is a null, the
 * same as an entity with no `d:`, and the caller renders both as "no words
 * for this region" rather than as a claim.
 */
export function regionSpecClaim(
  graph: SpecGraph,
  path: string,
  documentationOf: (id: string) => string | null,
): RegionSpecClaim | null {
  const subject = normalizeRefPath(path);
  if (!subject || graph.empty) return null;

  const ids = specNodesClaiming(graph, subject);
  if (ids.length === 0) return null;

  const node = graph.nodes.find((n) => n.id === ids[0]);
  if (!node) return null;

  // Which of this entity's own refs did the claiming. It can declare several,
  // and only the ones covering this folder are evidence about it.
  const matched = bySpecificity(
    (node.codeRefs ?? [])
      .map((ref) => normalizeRefPath(ref.path))
      .filter((p) => p && pathClaims(p, subject)),
  );
  const claimPath = matched[0] ?? subject;

  return {
    id: node.id,
    name: node.name,
    kind: node.kind_raw || node.kind,
    // Keyed by `original_id`: the sidecar is keyed by the entity id the
    // analyzer emitted, and `id` is rewritten whenever a level change mints
    // collapsed nodes.
    description: documentationOf(node.original_id) ?? null,
    claimPath,
    exact: claimPath === subject,
  };
}

/** Trimmed to one line and capped, for a card that has to stay a card.
 *
 *  `d:` is a single string by grammar but not a short one — real specs carry
 *  paragraph-length descriptions, and this repo's longest runs past 300
 *  words. The full text has a home already (the Description pane); a hover
 *  surface that reproduced it would stop being one. */
export function clampDescription(text: string | null, max = 220): string | null {
  if (!text) return null;
  const flat = text.replace(/\s+/g, ' ').trim();
  if (!flat) return null;
  if (flat.length <= max) return flat;
  // Cut on a word boundary when there is one nearby, so the clamp does not
  // end mid-identifier and read as a typo.
  const cut = flat.slice(0, max);
  const space = cut.lastIndexOf(' ');
  return `${(space > max - 30 ? cut.slice(0, space) : cut).trimEnd()}…`;
}

/** The `documentationOf` a caller with the details sidecar in hand passes. */
export function documentationLookup(
  docs: Record<string, { documentation?: string }> | null,
): (id: string) => string | null {
  return (id) => docs?.[id]?.documentation ?? null;
}

/** Whether saying "no spec entity claims this" means anything here. A project
 *  with no `.elv` layer would hear it about every region, which is reporting
 *  the absence of something nobody asked for. */
export function hasSpecLayer(graph: SpecGraph): boolean {
  return !graph.empty && graph.nodes.length > 0;
}
