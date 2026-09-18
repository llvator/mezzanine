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
  /** Distance from the subject: 0 for the node itself, 1 for its parent,
   *  and so on. Child entries are always one level down and all carry 0 —
   *  the list is flat, and `buildChildEntries` never recurses. */
  depth: number;
  /**
   * Who wrote the description, when it is not the rung's own (UI-141).
   *
   * An entity's docstring belongs to the entity, so a code rung never sets
   * this. A *region* rung is a folder, which has no prose of its own — its
   * description is a spec entity's `d:` reaching it through a `cr:`, and
   * printing that unattributed would read as the folder describing itself.
   * See `regionChainEntries`.
   */
  attribution?: string;
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

/** One entry from a node the graph actually holds. */
function entryFor(node: D3Node, docs: DocLookup, depth: number): DescriptionEntry {
  return {
    entityId: node.original_id,
    name: node.display_label || node.name,
    qualifiedName: node.qualified_name,
    kind: node.kind_raw,
    filePath: node.file_path,
    line: node.line,
    documentation: docs[node.original_id]?.documentation ?? null,
    depth,
  };
}

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
    chain.push(entryFor(current, docs, depth));

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

/**
 * The direct children of `node`, one entry each.
 *
 * The chain answers "what is this *for*?" by climbing. Nothing answered
 * "what is *in* it?", and for an Elevator spec that is the more common
 * question: a Feature's meaning is largely the list of Functionalities
 * under it, each with its own `description:`, and the walk can never reach
 * them because it only ever goes up.
 *
 * Flat by design — one level, no recursion. A tree of descriptions is the
 * scope tree with prose attached, and that pane already exists.
 *
 * Same `parent_id` quirk the walk allows for: a child normally carries its
 * parent's `original_id`, but Rust impl blocks carry the bare type name. A
 * name match is only honoured when no entity actually owns that id, so a
 * type whose name collides with another entity's id cannot adopt its
 * children.
 *
 * Ordered by declaration site (file, then line), which for a `.elv` file is
 * the order the author wrote the entities in.
 */
export function buildChildEntries(
  node: D3Node,
  nodes: D3Node[],
  docs: DocLookup,
): DescriptionEntry[] {
  const ownsId = new Set(nodes.map((n) => n.original_id));
  const byName = node.name && !ownsId.has(node.name) ? node.name : null;

  return nodes
    .filter((n) => {
      if (n.original_id === node.original_id || !n.parent_id) return false;
      return n.parent_id === node.original_id || (byName != null && n.parent_id === byName);
    })
    .sort((a, b) => a.file_path.localeCompare(b.file_path) || a.line - b.line)
    .map((n) => entryFor(n, docs, 0));
}
