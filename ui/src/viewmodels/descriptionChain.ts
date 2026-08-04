/**
 * Ancestry-of-descriptions view model.
 *
 * A node's own description usually answers "what is this?" but not "what is
 * it *for*?" — that lives one or more levels up. For Elevator specs the
 * chain is the whole point (Functionality → Feature → Category, each with
 * its own `description:`), and for code it reads as
 * method → class → module docs.
 *
 * Pure: takes the node, the entity-level node list, and the details sidecar
 * map, and returns a flat list starting at the node itself. Renderers
 * (native VS Code view, standalone panel) share it so the two hosts can't
 * disagree about what the chain is.
 */
import type { D3Node } from '../types/graph';

export interface DescriptionEntry {
  /** Original (unsanitized) entity id — stable across renderers. */
  entityId: string;
  name: string;
  qualifiedName: string;
  /** Display kind — `kind_raw` (`Feature`, `Method`, …), the form
   *  NODE_COLORS and KIND_CODES are keyed by. */
  kind: string;
  filePath: string;
  line: number;
  /** The description itself: doc comment, docstring, or an Elevator
   *  `description:`. Null when the entity has none — rendered as a
   *  placeholder rather than dropped, so a gap in the chain is visible. */
  documentation: string | null;
  /** 0 for the node itself, 1 for its parent, and so on. */
  depth: number;
}

/** Minimal shape this module needs from `stores/details`. */
export interface DocLookup {
  [entityId: string]: { documentation?: string } | undefined;
}

/**
 * Hard stop on the walk. Deep chains stop being an explanation and start
 * being a scrollbar; 8 clears every hierarchy the parsers actually emit
 * (Elevator tops out at 4) while still terminating on malformed data.
 */
const MAX_DEPTH = 8;

/** Last `::`- or `/`-delimited segment of an entity id — the fallback label
 *  for an ancestor that has documentation but isn't in the loaded graph. */
function labelFromId(id: string): string {
  const sep = Math.max(id.lastIndexOf('::'), id.lastIndexOf('/'));
  return sep >= 0 ? id.slice(sep + (id[sep] === ':' ? 2 : 1)) : id;
}

/**
 * Walk `node` up through `parent_id` and return one entry per ancestor.
 *
 * `nodes` should be the *entity-level* graph, not the displayed one: a
 * parent is routinely filtered out of the view (or collapsed away at file /
 * module level) while still being the thing that explains its child.
 */
export function buildDescriptionChain(
  node: D3Node,
  nodes: D3Node[],
  docs: DocLookup,
): DescriptionEntry[] {
  // `parent_id` is the parent's original id, except for Rust impl blocks
  // where it can be the bare type name — index both, same as QualityReport.
  const byId = new Map<string, D3Node>();
  for (const n of nodes) {
    byId.set(n.original_id, n);
    if (!byId.has(n.name)) byId.set(n.name, n);
  }

  const chain: DescriptionEntry[] = [];
  const seen = new Set<string>();
  let current: D3Node = node;

  for (let depth = 0; depth < MAX_DEPTH; depth++) {
    if (seen.has(current.original_id)) break;
    seen.add(current.original_id);
    chain.push({
      entityId: current.original_id,
      name: current.display_label || current.name,
      qualifiedName: current.qualified_name,
      kind: current.kind_raw,
      filePath: current.file_path,
      line: current.line,
      documentation: docs[current.original_id]?.documentation ?? null,
      depth,
    });

    const parentId = current.parent_id;
    if (!parentId || seen.has(parentId)) break;

    const parent = byId.get(parentId);
    if (!parent) {
      // Outside the loaded graph. The details sidecar is repo-wide, so its
      // description is still reachable — emit it under a derived label
      // rather than truncating the chain at a scope boundary.
      const doc = docs[parentId]?.documentation;
      if (doc) {
        chain.push({
          entityId: parentId,
          name: labelFromId(parentId),
          qualifiedName: parentId,
          kind: 'Unknown',
          filePath: '',
          line: 0,
          documentation: doc,
          depth: chain.length,
        });
      }
      break;
    }
    current = parent;
  }

  return chain;
}
