import type { GraphData, D3Node, D3Link, CodeRef } from './types/graph';

interface AnalysisEntity {
  id: string;
  name: string;
  qualified_name: string;
  kind: string;
  visibility: string;
  file_path: string;
  span: { start: { line: number }; end: { line: number } };
  parent_id: string | null;
  parameters: { name: string; type_name: string | null; default_value: string | null }[];
  return_type: string | null;
  implements: string[];
  extends: string[] | null;
  tags: string[];
  /** Parser-emitted `key:value` strings (e.g. `caught:Foo`,
   * `pattern:1`, `bean:jdbcTemplate`). Surfaced as `details` on the
   * D3Node so renderers don't have to re-parse the strings. */
  attributes?: string[];
  documentation?: string | null;
  source_code: string | null;
  fields: { name: string; type_name: string | null }[];
  impl_blocks: string[];
  metrics?: {
    cyclomatic?: number;
    cognitive_complexity?: number;
    max_nesting?: number;
    loc: number;
    param_count?: number;
    fan_in: number;
    fan_out: number;
    in_cycle: boolean;
    field_count?: number;
    method_count: number;
    public_field_ratio?: number;
    composite_score: number;
    instability?: number;
    return_complexity?: number;
    smells?: string[];
  };
}

interface AnalysisRelationship {
  source_id: string;
  target_id: string;
  kind: string;
  /** Language-aware display label from the backend. */
  label?: string;
  /** Passive form for incoming edges (e.g., "defined in"). */
  incoming_label?: string;
  metadata: Record<string, string>;
}

interface AnalysisScopeMetrics {
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
  avg_quality: number;
  max_quality: number;
  quality_ok: number;
  quality_warn: number;
  quality_bad: number;
  composite_score?: number;
}

interface AnalysisJson {
  entities: AnalysisEntity[];
  relationships: AnalysisRelationship[];
  files?: AnalysisScopeMetrics[];
  modules?: AnalysisScopeMetrics[];
  thresholds?: Record<string, unknown>;
}

function sanitizeId(id: string): string {
  return id.replace(/[^a-zA-Z0-9_]/g, '_');
}

const KIND_MAP: Record<string, string> = {
  file: 'File', module: 'Module', class: 'Class',
  dataclass: 'Dataclass',
  'abstract class': 'AbstractClass',
  struct: 'Struct',
  interface: 'Interface', trait: 'Trait', enum: 'Enum', type: 'TypeAlias',
  function: 'Function', method: 'Method', constant: 'Constant',
  variable: 'Variable', property: 'Property', macro: 'Macro',
  parameter: 'Parameter', branch: 'Branch', loop: 'Loop',
  service: 'Service', import: 'Import',
  // UI-002 — forward-looking. The backend `EntityKind` enum doesn't
  // yet include these (Bean lands with GR-010, Table with GR-011a);
  // mapping them here is a no-op until then and avoids a UI follow-up
  // PR the moment the parser starts emitting them.
  bean: 'Bean', table: 'Table', view: 'View',
  // Elevator (`.elv`) — domain-level spec language nodes.
  extension: 'Extension',
  category: 'Category', feature: 'Feature', functionality: 'Functionality',
  concept: 'Concept', ui_page: 'UiPage',
  // ansible-deploy — k8s deploy-topology nodes. Keys are the backend's
  // display_name() strings (spaces, not serde snake_case), matching the
  // `kind` field the JSON actually carries — e.g. TemplateFile's
  // display name is "template".
  playbook: 'Playbook', role: 'Role',
  'host group': 'HostGroup',
  'deployment set': 'DeploymentSet',
  'deployment entry': 'DeploymentEntry',
  template: 'TemplateFile',
  'k8s resource': 'K8sResource',
  'helm chart': 'HelmChart',
  unknown: 'Unknown',
};

const EXT_LANGUAGE_MAP: Record<string, string> = {
  rs: 'Rust', py: 'Python', pyi: 'Python',
  js: 'JavaScript', mjs: 'JavaScript', cjs: 'JavaScript', jsx: 'JavaScript',
  ts: 'TypeScript', tsx: 'TypeScript',
  java: 'Java', go: 'Go', cs: 'C#',
  cpp: 'C++', cc: 'C++', cxx: 'C++', hpp: 'C++', hxx: 'C++',
  c: 'C', h: 'C', rb: 'Ruby', swift: 'Swift',
  kt: 'Kotlin', kts: 'Kotlin', scala: 'Scala', sc: 'Scala', php: 'PHP',
  groovy: 'Groovy', gradle: 'Groovy',
  elv: 'Elevator',
  sql: 'SQL',
  // ansible-deploy files (var files, playbooks, Jinja templates). Only
  // reached for nodes the backend already classified as ansible-deploy,
  // so labelling these extensions here is display-only and safe.
  yml: 'Ansible Deploy', yaml: 'Ansible Deploy', j2: 'Ansible Deploy',
};

function normalizeExtends(value: string | string[] | null | undefined): string[] {
  if (Array.isArray(value)) return value;
  if (value) return [value];
  return [];
}

function languageFromPath(filePath: string): string {
  const ext = filePath.split('.').pop()?.toLowerCase() || '';
  return EXT_LANGUAGE_MAP[ext] || 'Unknown';
}

const REL_KIND_MAP: Record<string, { raw: string; label: string }> = {
  contains: { raw: 'Contains', label: 'contains' },
  imports: { raw: 'Imports', label: 'imports' },
  calls: { raw: 'Calls', label: 'calls' },
  inherits: { raw: 'Inherits', label: 'inherits' },
  implements: { raw: 'Implements', label: 'implements' },
  depends_on: { raw: 'DependsOn', label: 'depends on' },
  uses_type: { raw: 'UsesType', label: 'uses type' },
  returns: { raw: 'Returns', label: 'returns' },
  takes_param: { raw: 'TakesParam', label: 'takes param' },
  // ansible-deploy edges.
  renders_from: { raw: 'RendersFrom', label: 'renders from' },
  includes: { raw: 'Includes', label: 'includes' },
  provides: { raw: 'Provides', label: 'provides' },
  requires: { raw: 'Requires', label: 'requires' },
  // Inter-resource references (Ingress→Service, Deployment→Secret/…).
  // The per-edge label carries the verb (mounts / routes to / …), so
  // this generic label is only a fallback.
  references: { raw: 'References', label: 'references' },
  // Templating dimension: template → the {{ variable }} it uses.
  interpolates: { raw: 'Interpolates', label: 'interpolates' },
};

/** The PascalCase spelling `LINK_COLORS` keys on, for a kind that arrives as
 *  the snake_case the backend serializes — diff.json's relationship deltas,
 *  which never pass through the link transform below. */
export function relKindRaw(kind: string): string {
  return REL_KIND_MAP[kind]?.raw ?? kind;
}

/**
 * Boolean-truthy metadata keys the parsers use as semantic
 * relationship tags. The parser writes `metadata.null_safe = "true"`;
 * the UI reads it as a tag string. Kept as a Set rather than a
 * regex so adding a new tag is a one-line edit and the lookup is
 * O(1) for every relationship in the graph.
 *
 * UI-004 — see GR-007 (`null_safe`), GR-008 (`spread`, `spread_args`),
 * GR-010 (`bean_lookup`), GR-011 (`dynamic_sql`), GR-012
 * (`dynamic_impex`). Tags ship as their parsers land; the rendering
 * is in place for whichever are on the wire today.
 */
const RELATIONSHIP_TAG_KEYS = new Set([
  'null_safe',
  'spread',
  'spread_args',
  'bean_lookup',
  'dynamic_sql',
  'dynamic_impex',
  'unresolved',
]);

/** Pull tag-flag keys out of a relationship's metadata. Treats
 *  `"true"` / `"1"` as truthy and ignores anything else, so a stray
 *  metadata key with a different shape (e.g. `branch=c1`) doesn't
 *  leak into the tag set. */
function extractRelationshipTags(metadata: Record<string, string> | undefined): string[] {
  if (!metadata) return [];
  const tags: string[] = [];
  for (const key of RELATIONSHIP_TAG_KEYS) {
    const v = metadata[key];
    if (v === 'true' || v === '1') tags.push(key);
  }
  return tags;
}

/**
 * True for attribute keys carrying an Elevator code reference: the
 * untagged `cr` or a layer-partitioned `cr.<tag>` (`cr.fe`, `cr.be`).
 * The one place that decides what counts as a code-ref key, shared by
 * `extractDetails` (which skips them) and `extractCodeRefs` (which
 * claims them). UI-026.
 */
function isCodeRefKey(key: string): boolean {
  return key === 'cr' || key.startsWith('cr.');
}

/**
 * Parse the Elevator code references out of an entity's attributes.
 *
 * Mirrors `parse_cr_attr` in `src/output/elevator_code_map.rs` so the
 * Rust renderers and the UI agree on what a code reference is: the
 * key is `cr` or `cr.<tag>`, the value is a repo-relative file or
 * folder path. Repeated refs stay distinct entries — an entity
 * declaring two paths implements two places, and collapsing them into
 * one string loses that.
 *
 * UI-026.
 */
function extractCodeRefs(attributes: string[] | undefined): CodeRef[] {
  if (!attributes || attributes.length === 0) return [];
  const out: CodeRef[] = [];
  for (const raw of attributes) {
    const idx = raw.indexOf(':');
    if (idx <= 0) continue;
    const key = raw.slice(0, idx).trim();
    const path = raw.slice(idx + 1).trim();
    if (!path || !isCodeRefKey(key)) continue;
    const tag = key === 'cr' ? '' : key.slice('cr.'.length);
    if (key !== 'cr' && !tag) continue;
    out.push({ tag, path });
  }
  return out;
}

/**
 * Parse parser-emitted `key:value` attribute strings into a
 * `Record<key, value>`. Unknown prefixes pass through; the UI's
 * label table (in EntityInfo) decides which keys get a human-readable
 * row and which fall through to a generic "Other" list.
 *
 * Multi-valued keys (one entity carrying two `caught:` attributes,
 * which Groovy doesn't currently produce but Python's multi-except
 * could) join with ` | `, matching how the parsers already render
 * multi-catch caught types.
 *
 * Elevator code references (`cr:` / `cr.<tag>:`) are skipped — they
 * have a structured home in `codeRefs` and joining their paths into a
 * display string would only force every consumer to re-split it
 * (UI-026).
 *
 * A markdown note's `stem:` is skipped for the opposite reason: it is
 * resolution plumbing, not a fact about the document. The analyzer needs
 * it to match `[[wikilinks]]` written against a filename rather than a
 * title, and by the time a note reaches the panel the reader can already
 * see the filename on the FILE row.
 *
 * UI-006.
 */
const PLUMBING_KEYS = new Set(['stem']);

function extractDetails(attributes: string[] | undefined): Record<string, string> {
  if (!attributes || attributes.length === 0) return {};
  const out: Record<string, string> = {};
  for (const raw of attributes) {
    const idx = raw.indexOf(':');
    if (idx <= 0) continue;
    const key = raw.slice(0, idx).trim();
    const value = raw.slice(idx + 1).trim();
    if (!key || !value) continue;
    if (isCodeRefKey(key) || PLUMBING_KEYS.has(key)) continue;
    out[key] = out[key] ? `${out[key]} | ${value}` : value;
  }
  return out;
}

/**
 * Tag-aware label for synthetic Branch / Loop entities. Mirrors the
 * label table in UI-003 — most-specific tag wins, with the
 * accompanying attribute (caught type, manager expression, pattern,
 * condition) inlined when present. Falls back to `null` for entities
 * the table doesn't recognise so callers can decide whether to use
 * the entity's `name` instead.
 *
 * The table-driven approach keeps the label set in lock-step with
 * the parsers' tag output: adding a new tag family is one new entry,
 * not a component change.
 */
function deriveDisplayLabel(
  kindRaw: string,
  tags: string[],
  details: Record<string, string>,
): string | null {
  if (kindRaw !== 'Branch' && kindRaw !== 'Loop') return null;
  const tagSet = new Set(tags);

  const specifier = (key: string, fallback: string): string => {
    const v = details[key];
    if (!v) return fallback;
    const max = 40;
    return v.length > max ? `${fallback} ${v.slice(0, max - 1)}…` : `${fallback} ${v}`;
  };

  if (tagSet.has('try_body_arm')) return 'try';
  if (tagSet.has('catch_arm')) return specifier('caught', 'catch');
  if (tagSet.has('finally_arm')) return 'finally';
  if (tagSet.has('else_arm')) return 'else';
  if (tagSet.has('with_node')) return specifier('manager', 'with');
  if (tagSet.has('case_arm')) return specifier('pattern', 'case');
  if (tagSet.has('if_node')) return specifier('condition', 'if');
  if (tagSet.has('loop_node')) return 'loop';
  return null;
}

/** Override `kind_raw` for entities whose `tags` mark them as a
 *  rendering-only sub-kind (e.g. a Groovy script container is still
 *  `EntityKind::File` on the wire — no new backend kind — but renders
 *  as `GroovyScript` in the UI). Returns the original kind when no
 *  override applies. UI-001. */
function deriveKindRaw(rawFromKindMap: string, tags: string[]): string {
  if (rawFromKindMap === 'File' && tags.includes('groovy_script')) {
    return 'GroovyScript';
  }
  return rawFromKindMap;
}

export function transformAnalysisJson(analysis: AnalysisJson): GraphData {
  const nodeIds = new Set<string>();
  const nameToId = new Map<string, string>();

  const nodes: D3Node[] = analysis.entities.map((e) => {
    const id = sanitizeId(e.id);
    nodeIds.add(id);
    const sanitizedName = sanitizeId(e.name);
    if (!nameToId.has(sanitizedName)) nameToId.set(sanitizedName, id);
    const sanitizedQualified = sanitizeId(e.qualified_name);
    if (!nameToId.has(sanitizedQualified)) nameToId.set(sanitizedQualified, id);

    const tags = e.tags || [];
    const baseKindRaw =
      KIND_MAP[e.kind] || e.kind.charAt(0).toUpperCase() + e.kind.slice(1);
    const kindRaw = deriveKindRaw(baseKindRaw, tags);
    const details = extractDetails(e.attributes);
    const codeRefs = extractCodeRefs(e.attributes);
    const displayLabel = deriveDisplayLabel(kindRaw, tags, details);
    return {
      id,
      original_id: e.id,
      name: e.name,
      qualified_name: e.qualified_name,
      kind: e.kind,
      kind_raw: kindRaw,
      file_path: e.file_path,
      line: e.span.start.line + 1,
      end_line: e.span.end.line + 1,
      visibility: e.visibility.charAt(0).toUpperCase() + e.visibility.slice(1),
      parent_id: e.parent_id ?? null,
      parameters: e.parameters.map((p) => p.type_name ? `${p.name}: ${p.type_name}` : p.name),
      return_type: e.return_type,
      extends: normalizeExtends(e.extends),
      implements: e.implements,
      tags,
      source_code: e.source_code,
      fields: e.fields || [],
      impl_blocks: e.impl_blocks || [],
      language: languageFromPath(e.file_path),
      metrics: e.metrics,
      display_label: displayLabel ?? undefined,
      details: Object.keys(details).length ? details : undefined,
      codeRefs: codeRefs.length ? codeRefs : undefined,
    };
  });

  const links: D3Link[] = analysis.relationships
    .map((r) => {
      const sourceSanitized = sanitizeId(r.source_id);
      const targetSanitized = sanitizeId(r.target_id);

      const source = nodeIds.has(sourceSanitized)
        ? sourceSanitized
        : nameToId.get(sourceSanitized);
      const target = nodeIds.has(targetSanitized)
        ? targetSanitized
        : nameToId.get(targetSanitized);

      if (!source || !target) return null;

      const relInfo = REL_KIND_MAP[r.kind] || { raw: r.kind, label: r.kind };
      // Prefer the backend's language-aware label (e.g., "defines" for Rust,
      // "declares" for Java) over the generic REL_KIND_MAP label.
      let label = r.label || relInfo.label;
      const order = r.metadata?.order ? parseInt(r.metadata.order, 10) : null;

      let incomingLabel = r.incoming_label || label;

      // TakesParam's canonical data direction is param → function (the
      // param node "belongs to" the function). Both its generic forward
      // label ("takes param") and its passive form are awkward because
      // the real subject of the relationship is the function, not the
      // param. Rewrite both labels so whichever direction we render —
      // including cases where the hierarchy rule or focus rule doesn't
      // flip — has a clean reading.
      //
      //   forward (source=param, target=func):   param → param of → func
      //   passive (flipped, func is subject):    func  → has param → param
      if (relInfo.raw === 'TakesParam') {
        label = 'param of';
        incomingLabel = 'has param';
      }

      const linkTags = extractRelationshipTags(r.metadata);
      const link: D3Link = {
        source,
        target,
        kind: label,
        kind_raw: relInfo.raw,
        incoming_kind: incomingLabel,
        order: isNaN(order as number) ? null : order,
        tags: linkTags.length ? linkTags : undefined,
        binds_to: r.metadata?.binds_to || undefined,
        binds_type: r.metadata?.binds_type || undefined,
        rebinds_to: r.metadata?.rebinds_to || undefined,
      };
      return link;
    })
    .filter((l): l is D3Link => l !== null);

  return {
    nodes,
    links,
    files: analysis.files ?? [],
    modules: analysis.modules ?? [],
    thresholds: analysis.thresholds as GraphData['thresholds'],
  };
}

/** Check if data is already in D3 format or needs transformation */
export function loadGraphData(data: any): GraphData {
  if (data.nodes && data.links) {
    return data as GraphData;
  }
  if (data.entities && data.relationships) {
    return transformAnalysisJson(data);
  }
  throw new Error('Unrecognized data format');
}
