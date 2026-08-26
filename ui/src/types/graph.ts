export interface D3Field {
  name: string;
  type_name: string | null;
}

export interface EntityMetrics {
  cyclomatic?: number;
  cognitive_complexity?: number;
  max_nesting?: number;
  loc: number;
  param_count?: number;
  fan_in: number;
  fan_out: number;
  in_cycle: boolean;
  /** Number of fields (structs) or variants (enums). */
  field_count?: number;
  /** Number of methods/functions contained directly by this entity. */
  method_count: number;
  /** Fraction [0, 1] of fields marked pub — structs only. */
  public_field_ratio?: number;
  /** Composite "refactor pressure" score computed by the backend.
   *  Optional for backwards compatibility with older analysis data. */
  composite_score?: number;
  /** Instability index: fan_out / (fan_in + fan_out). 0 = stable, 1 = unstable. */
  instability?: number;
  /** Number of elements in the return type's outermost tuple. */
  return_complexity?: number;
  /** Weighted Methods per Class — sum of CC of directly-contained
   *  callables. Populated for containers only. */
  wmc?: number;
  /** Longest outbound call-chain depth (hops, capped). High values flag
   *  Law-of-Demeter violations or missing abstractions. */
  chain_depth?: number;
  /** PageRank centrality on the dependency subgraph. Normalised so the
   *  sum across the graph is 1.0 — use relatively (rank), not absolutely. */
  pagerank?: number;
  /** Detected code smells (anti-pattern signals). */
  smells?: string[];
}

/** An Elevator (`.elv`) code reference: the file or folder an entity
 *  declares as its implementation. `tag` is the layer partition from
 *  `cr.<tag>:` (`fe`, `be`, …) and is the empty string for a bare
 *  `cr:`. Authored in the spec, not derived from the code — treat a
 *  path as a declaration, not proof. UI-026. */
export interface CodeRef {
  tag: string;
  path: string;
}

export interface D3Node {
  id: string;
  /** Original (unsanitized) entity ID for sidecar detail lookups */
  original_id: string;
  name: string;
  qualified_name: string;
  kind: string;
  kind_raw: string;
  file_path: string;
  line: number;
  end_line: number;
  visibility: string;
  /** Original (non-sanitized) id of the parent entity, when applicable.
   * Resolved via `original_id` or `name` lookups in components that want
   * to display the enclosing type / module. */
  parent_id: string | null;
  parameters: string[];
  return_type: string | null;
  extends: string[];
  implements: string[];
  tags: string[];
  source_code: string | null;
  fields: D3Field[];
  impl_blocks: string[];
  language: string;
  metrics?: EntityMetrics;
  /** Present only on File/Folder nodes produced by `collapseGraph`: the raw
   *  scope rollup, kept alongside the promoted `metrics` so severity scoring
   *  can reach fields (`cohesion`, `entity_count`, …) that `EntityMetrics`
   *  has no home for. `collapseGraph` cannot score these itself without
   *  importing a store and closing an import cycle — see its header. */
  scope_metrics?: ScopeMetrics;
  /** Tag-aware display label for synthetic entities (Branch / Loop).
   * Derived in `transform.ts` from `tags` + `details` so renderers
   * (EntityInfo, ScopeTree, Sidebar) all read the same string —
   * UI-003. Falls back to `name` when no specific tag is recognised. */
  display_label?: string;
  /** Structured per-entity attributes derived from the parser-emitted
   * `attributes: ["caught:Foo", "pattern:1", "bean:foo", …]` strings.
   * Keyed by attribute prefix (`caught`, `pattern`, `bean`, `manager`,
   * `condition`); extra keys are preserved as-is. UI-006.
   *
   * Elevator `cr:` / `cr.<tag>:` attributes are *not* here — they are
   * surfaced structurally as `codeRefs` instead. */
  details?: Record<string, string>;
  /** Elevator code references declared by this entity, parsed out of
   * the same `attributes` array `details` comes from. Undefined (not
   * `[]`) when the entity declares none, matching `details` — read it
   * as `node.codeRefs ?? []`. UI-026. */
  codeRefs?: CodeRef[];

  /**
   * `original_id` of the callable whose *body* this entity lives in, or
   * undefined when it is declared at the surface of its file (UI-113).
   *
   * Stamped by `liftBodies` from the `parent_id` ancestry, walking out
   * through any number of branches and loops — so a call nested five arms
   * deep in `parse()` carries `parse`, the same value its parameters do.
   * Absent, never null, for a surface entity: a struct's field, an
   * interface's property and an `impl` method all have parents and are all
   * declarations, which is exactly the distinction `parent_id` cannot make.
   *
   * `original_id`, not `id`, so it compares directly against
   * `selectedNode.original_id` — one equality reopens one callable's body.
   */
  body_of?: string;

  // D3 simulation properties (added at runtime)
  x?: number;
  y?: number;
  fx?: number | null;
  fy?: number | null;
  index?: number;
}

export interface D3Link {
  source: string | D3Node;
  target: string | D3Node;
  kind: string;
  kind_raw: string;
  /** Passive form for incoming edges (e.g., "defined in"). */
  incoming_kind: string;
  order: number | null;
  /** Number of underlying entity-level edges this link represents. Always
   * 1 in entity mode; populated by `collapseGraph` at file/folder level
   * to drive stroke-width and the hover breakdown. */
  weight?: number;
  /** Per-kind counts of the underlying edges at scope level. Populated
   * alongside `weight` when `collapseGraph` runs. */
  breakdown?: Record<string, number>;
  /** Semantic tags attached to the relationship by the parsers — e.g.
   * `null_safe` (GR-007), `spread` (GR-008), `bean_lookup` (GR-010),
   * `dynamic_sql` (GR-011), `dynamic_impex` (GR-012). Derived in
   * `transform.ts` from boolean-truthy values in the relationship's
   * `metadata` map. UI-004. */
  tags?: string[];
  /** Local variable receiving this call/instantiation's result, when
   *  the call is the RHS of a Java declaration `Type x = call()`. */
  binds_to?: string;
  /** Declared type of `binds_to`, when known (declarations only —
   *  reassignments omit it because the type isn't restated). */
  binds_type?: string;
  /** Variable being reassigned (`x = call()`, plain `=` only). Kept
   *  separate from `binds_to` so the renderer can distinguish a fresh
   *  binding from an update. */
  rebinds_to?: string;

  /**
   * Ids this edge was re-routed off, when it is the lifted twin of an edge
   * with an end inside a body (UI-113). Absent on every real edge.
   *
   * A call written inside an `if` hangs off the `Branch` node, not off the
   * function — so hiding bodies would drop it. `liftBodies` adds a twin
   * anchored at the enclosing callable and records the bypassed ends here,
   * which is what lets the twin stand down again the moment the reader opens
   * that body and the real edge becomes drawable.
   */
  lifted_from?: string[];
}

export interface GraphData {
  nodes: D3Node[];
  links: D3Link[];
  /** Per-file rollup metrics. Populated by the analyzer; scope-filtered
   * alongside nodes so only in-scope files show up in the Quality report. */
  files?: ScopeMetrics[];
  /** Per-directory rollup metrics. Same scoping rules as `files`. */
  folders?: ScopeMetrics[];
  /** Quality thresholds from the backend. Single source of truth for
   *  tier classification and scoring. */
  thresholds?: BackendThresholds;
}

/** Warn/bad threshold pair from the backend. */
export interface WarnBad {
  warn: number;
  bad: number;
}

/** All quality thresholds, serialized by the backend alongside the graph. */
export interface BackendThresholds {
  cc: WarnBad;
  cognitive: WarnBad;
  nest: WarnBad;
  loc_callable: WarnBad;
  loc_container: WarnBad;
  params: WarnBad;
  fan_out: WarnBad;
  fields: WarnBad;
  variants: WarnBad;
  method_count: WarnBad;
  public_field_ratio: WarnBad;
  file_entity_count: WarnBad;
  file_loc: WarnBad;
  file_fan_out: WarnBad;
  folder_entity_count: WarnBad;
  folder_loc: WarnBad;
  folder_fan_out: WarnBad;
  cohesion: WarnBad;
  [key: string]: unknown;
}

/** The four organisation patterns a folder's drawn graph falls into,
 *  worst first. Mirrors `ShapePattern` on the Rust side. */
export type ShapePattern = 'cyclic' | 'tangled' | 'hierarchical' | 'fractal';

/** How legible the picture a folder draws is.
 *
 *  Scored over what the canvas renders when collapsed to that folder — its
 *  immediate children, each subfolder standing as one node. Every score
 *  runs 0–1 with **higher meaning better**, the opposite of the refactor
 *  pressure score beside it: the two answer different questions and must
 *  not be read on one scale. Organisation, not code quality. */
export interface FolderShape {
  pattern: ShapePattern;
  /** Weighted mean of whichever sub-scores below are defined. */
  compliance: number;
  /** Share of children outside any dependency loop. */
  acyclicity: number;
  /** Share of edges stepping exactly one level down. Absent when the
   *  children have no edges between them — unmeasured, not perfect. */
  layering?: number;
  /** Share of the drawn edges that branch rather than merge — one arriving
   *  at each child. Absent on the same condition `layering` is, the two
   *  being ratios over the same edges.
   *
   *  Gates `fractal` and is deliberately NOT a term in `compliance`
   *  (ADR 0013), so a folder can blend well and still be held back by it.
   *  Reads against `layering` on the same scale and disagrees with it on
   *  purpose: a shared helper steps one level cleanly and still merges. */
  arborescence?: number;
  /** Share of the traffic arriving from outside that lands on one file.
   *  Absent when nothing outside depends on the folder. */
  entry_concentration?: number;
  /** Share of the dependencies leaving the folder that start at a leaf — a
   *  child with no outgoing edge inside the folder — or at its door. The
   *  mirror of `entry_concentration`: that one grades what arrives, this what
   *  leaves, and together they make a collapsed folder honest in both
   *  directions. Only the near end is graded; where an exit lands stays its
   *  target's business (ADR 0031). Gates `fractal` and is NOT a term in
   *  `compliance`, exactly as `arborescence` is not. Absent when the folder
   *  depends on nothing outside itself. */
  egress?: number;
  /** Mean compliance of the subfolders inside. Absent when there are none. */
  child_compliance?: number;
  /** Immediate children, files and subfolders together. Reported, not scored. */
  child_count: number;
  /** Why this folder is not one tier higher. Absent only for `fractal`. */
  blocker?: ShapeBlocker;
}

/** Which gate capped a folder's tier, and the measurement that failed it.
 *  Mirrors `ShapeBlocker` on the Rust side, serialised as a tagged union.
 *
 *  Computed there rather than re-derived here on purpose: the thresholds
 *  are configurable and live in `Thresholds`, so a second copy of the
 *  comparisons in the panel would be a second answer waiting to disagree
 *  with the tier it sits next to. */
export type ShapeBlocker =
  /** Children sit in a dependency loop; `value` is `acyclicity`. */
  | { gate: 'cycles'; value: number }
  /** Edges skip levels; `value` is `layering`. */
  | { gate: 'layering'; value: number }
  /** Children lean on the same sibling; `value` is `arborescence`. */
  | { gate: 'merges'; value: number }
  /** Outsiders reach in at many points; `value` is `entry_concentration`. */
  | { gate: 'entry'; value: number }
  /** Children reach outside from the folder's middle; `value` is `egress`. */
  | { gate: 'egress'; value: number }
  /** A subfolder is itself below hierarchical; `value` is its pattern. */
  | { gate: 'child_pattern'; value: ShapePattern }
  /** Subfolders broadly out of order; `value` is `child_compliance`. */
  | { gate: 'child_compliance'; value: number }
  /** Children with no edges between them — nothing to be similar to. */
  | { gate: 'unstructured' }
  /** More children than a reader takes in at once; `value` is the count. */
  | { gate: 'breadth'; value: number }
  /** Every gate passed and the blend still fell short; `value` is `compliance`. */
  | { gate: 'compliance'; value: number };

/**
 * The graph one folder draws, with a verdict on every part of it — the
 * evidence behind its `FolderShape`. Mirrors `FolderPicture` on the Rust
 * side, served by `GET /api/shape`.
 *
 * Produced by the pass that does the scoring rather than rebuilt here, for
 * the reason `ShapeBlocker` is: a second derivation is a second answer, and
 * this one would be free to draw a picture the number beside it denies.
 */
export interface FolderPicture {
  folder: string;
  /** Immediate children — files directly inside, each subfolder as one
   *  node. Exactly what the canvas draws when collapsed here. */
  children: PictureChild[];
  /** Exactly the arrows the scores were computed over. `erased` sits
   *  beside this list and deliberately not in it. */
  edges: PictureEdge[];
  /** The arrows the build erases — every import behind each of them is an
   *  `import type` or a Python `if TYPE_CHECKING:` import, so none of them
   *  is counted in any score (ADR 0026). Drawn all the same: the type is
   *  still something a reader has to go and find. Absent on a picture
   *  served by an older binary. */
  erased?: ErasedEdge[];
  /** One hop out, never transitively: the files elsewhere in the repo that
   *  touch this folder, and nothing else. */
  outside: OutsideEdge[];
  /** The busiest file(s) depended on from outside — the folder's front
   *  doors, and the numerator of `entry_concentration`. */
  doors: string[];
}

export interface PictureChild {
  path: string;
  kind: 'file' | 'folder';
  /** The row it draws on: longest path from a source. Children in one
   *  dependency loop share a level, a loop having no order to lay out. */
  level: number;
  /** Dependencies arriving from outside the folder into this child. */
  inbound: number;
  is_door: boolean;
}

/** How an edge between two children reads. `step` is the shape you want;
 *  the other two are what `layering` and `acyclicity` charge for. */
export type EdgeVerdict = 'step' | 'skip' | 'back';

export interface PictureEdge {
  from: string;
  to: string;
  verdict: EdgeVerdict;
}

/** One discounted arrow. No verdict: `step`, `skip` and `back` are readings
 *  of the levels, and the levels are assigned over the graph this arrow was
 *  taken out of. */
export interface ErasedEdge {
  from: string;
  to: string;
}

/** How a dependency crossing the folder's boundary reads. `exit` is never a
 *  defect — depending outward is what a folder is for. */
export type OutsideVerdict = 'entry' | 'breach' | 'exit';

export interface OutsideEdge {
  /** The file on the far side, wherever in the repo it lives. */
  outside: string;
  /** The file inside the folder at this end — what a breach names. */
  inside: string;
  /** The immediate child holding `inside` — the circle the line attaches
   *  to, which for a nested file is not the same thing. */
  child: string;
  verdict: OutsideVerdict;
}

/** What `GET /api/shape` returns: the verdict and the graph it was
 *  computed over, together, so no reader can pair one analysis's picture
 *  with another's number. */
export interface ShapeResponse {
  path: string;
  shape?: FolderShape;
  picture: FolderPicture;
}

/** File- or folder-level quality rollup (shape shared with Rust side). */
export interface ScopeMetrics {
  path: string;
  entity_count: number;
  callable_count: number;
  container_count: number;
  loc: number;
  internal_edges: number;
  external_edges: number;
  cohesion?: number;
  fan_in: number;
  fan_out: number;
  in_cycle: boolean;
  /** Instability index at scope level. */
  instability?: number;
  /** UI-091 — distinct scopes referencing this one over `References` edges,
   *  counted apart from `fan_in` because references are not coupling. Lets a
   *  scope that was never measured be told apart from a decoupled one. */
  ref_fan_in?: number;
  /** Distinct scopes this one references. See `ref_fan_in`. */
  ref_fan_out?: number;
  /** Mean composite score of entities in this scope. */
  avg_quality?: number;
  /** Worst composite score in this scope. */
  max_quality?: number;
  /** Entities with composite score ≤ 0.5 (healthy). */
  quality_ok?: number;
  /** Entities with composite score in (0.5, 1.0] (amber). */
  quality_warn?: number;
  /** Entities with composite score > 1.0 (red). */
  quality_bad?: number;
  /** Composite scope score computed by the backend. */
  composite_score?: number;
  /** How legible a picture this folder draws. Folders only — a file has
   *  no children to draw a graph of, so it never carries one. */
  shape?: FolderShape;
}

/**
 * What the canvas is drawing.
 *
 * `graph` is the force simulation, `tree` the BFS spanning tree around a
 * selection, and `shape` one folder's own graph laid out by level — the
 * picture `FolderShape` scores, which neither of the other two draws:
 * `graph` shows the whole hairball and `tree` treats every relationship as
 * the same kind of thing, which is exactly the distinction shape is about.
 */
export type ViewMode = 'graph' | 'tree' | 'shape';

/** Aggregation level for the graph view: show every entity, collapse to
 * one node per file, or collapse to one node per directory.
 *
 * `'folder'` was spelled `'module'` until it collided once too often with
 * `EntityKind::Module` — the language construct a `mod` or a Python module
 * parses to, which reaches the UI as `kind_raw: 'Module'`. Nothing here
 * ever meant that: the level has always grouped by the directory holding
 * the file. Stored views written before the rename are migrated on read —
 * see `normalizeState` in `savedViews.ts`. */
export type GraphLevel = 'entity' | 'file' | 'folder';

export type TriState = 'general' | 'on' | 'off';

export interface LevelOverrides {
  enabled: boolean;
  entityTypes: Record<string, TriState>;
  relTypes: Record<string, TriState>;
  outgoing: TriState;
  incoming: TriState;
  /**
   * Show edges between two nodes at this same level. When `false`, only
   * cross-level expansion edges and (for level 1) direct edges to the
   * selected node are drawn; same-level peer edges are hidden.
   * Default: true — preserves the behavior of pre-peer-filter builds.
   */
  peerEdges: boolean;
}

/**
 * Whether a node belongs to the Elevator spec layer rather than to the code.
 *
 * Lives with the node model because three layers ask it — the display plan,
 * the search-result block classifier, and the spec pane's own view model —
 * and the answer must be the same for all three or the split view draws a
 * node in both panes, or in neither.
 *
 * Keyed on `language`, matching the Elevator closure pass in `stores/scope.ts`.
 * The `elevator` *tag* is a near-synonym but not the same predicate:
 * `stores/codeRefs.ts` deliberately widens its own test to markdown notes,
 * which claim code the same way a `cr:` does but are not nodes in the spec
 * hierarchy.
 */
export function isSpecNode(node: D3Node): boolean {
  return node.language === 'Elevator' && !node.tags?.includes('ghost');
}

export const NODE_COLORS: Record<string, string> = {
  Class: '#2196F3',
  Dataclass: '#7E57C2',
  AbstractClass: '#1565C0',
  Struct: '#4CAF50',
  Interface: '#FF9800',
  Trait: '#FF5722',
  Function: '#9C27B0',
  Method: '#9C27B0',
  Module: '#607D8B',
  /** The directory rollup `collapseGraph` emits, not a `mod` declaration —
   *  those are `Module` above. Same slate the rollup drew in when the two
   *  shared one key, so the canvas is unchanged by the split. */
  Folder: '#607D8B',
  Enum: '#E91E63',
  File: '#795548',
  Constant: '#00BCD4',
  Variable: '#8BC34A',
  Property: '#BA68C8',
  Service: '#26A69A',
  TypeAlias: '#3F51B5',
  Macro: '#FFEB3B',
  Parameter: '#66BB6A',
  Branch: '#B0BEC5',
  Loop: '#90A4AE',
  Import: '#A1887F',
  /** Synthetic File container emitted by the Groovy parser for script
   * files (top-level statements, no enclosing class). UI-001. */
  GroovyScript: '#A1B56C',
  /** Synthetic Spring-bean entity emitted by GR-010. Lavender accent
   * pairs with the bean-lookup edge styling in UI-004. UI-002. */
  Bean: '#9575CD',
  /** Database table. Amber accent to read as data infrastructure rather
   * than code. Declared for GR-011 under UI-002; first emitted by the
   * `.sql` parser (SQL-001). */
  Table: '#FFB74D',
  /** Database view. A desaturated sibling of Table's amber — same family,
   * because a view is a table-shaped thing, but dimmer because its rows are
   * derived rather than stored. */
  View: '#C99A5B',
  /** Markdown document. Slate blue-grey — prose is the layer code is
   * discussed *about*, so it reads as background material next to the
   * saturated code kinds rather than competing with them. */
  Note: '#7986CB',
  /** Elevator: widest grouping above Category (optional). Deep
   * indigo — sits one tier above Category visually, signals
   * "structural / top of hierarchy". */
  Extension: '#1A237E',
  /** Elevator: top-level grouping (the onboarding "ground floor").
   * Deep purple — sits above Features in the visual hierarchy and
   * doesn't collide with the amber/cyan/pink Feature family below. */
  Category: '#5E35B1',
  /** Elevator: top-level capability. Amber 600 — the "elevator
   * button" yellow communicates "this is an entry point". */
  Feature: '#FFC107',
  /** Elevator: verb on a Feature. Lighter amber so it reads as a
   * derivative of its Feature in the hierarchy. */
  Functionality: '#FFE082',
  /** Elevator: cross-cutting domain concept. Cyan to break out of
   * the amber family — visually flags "this isn't in the tree". */
  Concept: '#26C6DA',
  /** Elevator: UI page. Pink to signal "user-facing surface",
   * distinct from the structural amber family. */
  UiPage: '#EC407A',
  /** ansible-deploy: orchestration + topology kinds. Playbook/Role in
   * the Ansible red family (top of the chain); the k8s resource in the
   * Kubernetes brand blue so the actual primitive stands out. */
  Playbook: '#D32F2F',
  Role: '#FF7043',
  HostGroup: '#455A64',
  DeploymentSet: '#78909C',
  DeploymentEntry: '#FFB300',
  TemplateFile: '#8D6E63',
  K8sResource: '#326CE5',
  HelmChart: '#3949AB',
  Unknown: '#9E9E9E',
};

export const LINK_COLORS: Record<string, string> = {
  Contains: '#607D8B',
  Imports: '#9E9E9E',
  Calls: '#FF9800',
  Inherits: '#2196F3',
  Implements: '#4CAF50',
  DependsOn: '#B39DDB',
  UsesType: '#00BCD4',
  Returns: '#CE93D8',
  TakesParam: '#66BB6A',
  /** ansible-deploy edges. RendersFrom is the load-bearing
   * entry→template link — Kubernetes blue to match the resources it
   * reaches. Provides/Requires bind the orchestration side to the
   * host-group side through the shared variable hub. */
  RendersFrom: '#326CE5',
  Includes: '#EF6C00',
  Provides: '#66BB6A',
  Requires: '#AB47BC',
  /** Inter-resource wiring (Ingress→Service, Deployment→Secret/ConfigMap/
   * PVC). Teal to read as "runtime wiring", distinct from the structural
   * deploy edges above. */
  References: '#009688',
  /** Templating dimension: template → {{ variable }}. Muted light indigo
   * so this high-volume layer recedes behind the structural topology. */
  Interpolates: '#9FA8DA',
};

export const LANGUAGE_COLORS: Record<string, string> = {
  Rust: '#DEA584',
  Python: '#3572A5',
  JavaScript: '#F7DF1E',
  TypeScript: '#3178C6',
  Java: '#B07219',
  Go: '#00ADD8',
  'C#': '#178600',
  'C++': '#F34B7D',
  C: '#555555',
  Ruby: '#CC342D',
  Swift: '#F05138',
  Kotlin: '#A97BFF',
  /** Dart — the language's own brand teal, well clear of Go's cyan
   * so a repo with both reads correctly in the language filter. */
  Dart: '#00B4AB',
  Scala: '#DC322F',
  PHP: '#4F5D95',
  /** Apache Groovy — official brand colour. Distinct from Java's
   * brown so a mixed Java + Groovy codebase reads correctly in the
   * language filter / scope tree. */
  Groovy: '#4298B4',
  /** Elevator (`.elv`) — domain-level spec language. Same amber as
   * the Feature kind (its dominant node) so the language identity
   * stays consistent across language-filter and node-color views. */
  Elevator: '#FFC107',
  /** ansible-deploy — Ansible brand red, so a deploy repo reads as its
   * own language in the filter / scope tree. */
  'Ansible Deploy': '#D32F2F',
  /** Markdown — the same slate as the Note kind (its only node), so the
   * doc layer looks the same whether the reader is colouring by language
   * or by kind. */
  Markdown: '#7986CB',
  Unknown: '#9E9E9E',
};

export const KIND_CODES: Record<string, string> = {
  Class: 'Cl',
  Dataclass: 'Dc',
  AbstractClass: 'AbCl',
  Struct: 'St',
  Interface: 'In',
  Trait: 'Tr',
  Function: 'Fn',
  Method: 'Me',
  Module: 'Md',
  /** Was `Md` while the rollup shared a key with `EntityKind::Module` —
   *  which is what put a "module" badge on a directory and made the two
   *  hard to tell apart on the canvas in the first place. */
  Folder: 'Fo',
  Enum: 'En',
  File: 'Fi',
  Constant: 'Const',
  TypeAlias: 'Ty',
  Macro: 'Ma',
  Variable: 'Va',
  Property: 'Pr',
  Service: 'Sv',
  Parameter: 'Pa',
  Branch: 'Br',
  Loop: 'Lp',
  Import: 'Im',
  /** UI-001 — distinguishes a Groovy script container from a generic
   * `File`. Two letters keep the visual budget the same as `Fi`. */
  GroovyScript: 'Gv',
  /** UI-002 — synthetic Spring-bean entity. Inert until GR-010
   * lands; the entry is forward-looking so the UI doesn't need a
   * second PR when the parser starts emitting the kind. */
  Bean: 'Bn',
  /** Database objects (SQL-001). `Vw` avoids colliding with `Va`/Variable. */
  Table: 'Tb',
  View: 'Vw',
  /** Markdown document. `Nt` — `No` would read as a negation. */
  Note: 'Nt',
  /** Elevator (`.elv`) entity-kind codes. */
  Extension: 'Ex',
  Category: 'Ca',
  Feature: 'Ft',
  Functionality: 'Fu',
  Concept: 'Cn',
  UiPage: 'Up',
  /** ansible-deploy entity-kind codes. */
  Playbook: 'Pb',
  Role: 'Ro',
  HostGroup: 'Hg',
  DeploymentSet: 'DS',
  DeploymentEntry: 'De',
  TemplateFile: 'Tpl',
  K8sResource: 'K8s',
  HelmChart: 'Hlm',
  Unknown: '??',
};
