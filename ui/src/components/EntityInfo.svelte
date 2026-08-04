<script lang="ts">
  import ColorChip from './ColorChip.svelte';
  import type { D3Node } from '../types/graph';
  import { LINK_COLORS } from '../types/graph';
  import { graphData } from '../stores/graph';
  import { drillIn } from '../stores/scope';
  import { currentDetail, loadDetail, type EntityDetails } from '../stores/details';
  import { codeRefIndex, anchorState, isSpecEntity, revealSpecEntity, showImplementingCode } from '../stores/codeRefs';
  import {
    METRIC_EXPLANATIONS,
    SMELL_META,
    tierCognitive,
    tierFieldCount,
    tierMethodCount,
    tierPublicFieldRatio,
  } from '../stores/quality';
  import { diffActive, diffStatusMap, diffDeltaMap, diffSourceChangedMap, diffBaseIdMap, baseDetailsCache, DIFF_COLORS, normalizeEntityId, type ChangeStatus, type MetricDelta } from '../stores/diff';
  import { computeLineDiff, type DiffLine } from '../utils/lineDiff';

  export let entity: D3Node | null;
  export let showSource: boolean = true;
  export let showRelationships: boolean = true;
  export let compact: boolean = false;

  // Load details lazily when entity changes
  let detail: EntityDetails | null = null;
  $: if (entity) {
    // Use inline source_code/fields/impl_blocks if present on the node (HTML mode),
    // otherwise lazy-load from the sidecar file (JSON/dev mode)
    if (entity.source_code || entity.fields?.length || entity.impl_blocks?.length) {
      detail = {
        source_code: entity.source_code ?? undefined,
        fields: entity.fields,
        impl_blocks: entity.impl_blocks,
      };
    } else {
      detail = null;
      loadDetail(entity.original_id);
    }
  } else {
    detail = null;
  }
  // Only assign when the loaded detail actually belongs to the entity we're
  // displaying. Without this guard, switching from entity A to B picks up
  // A's still-resident detail before B's async load resolves — showing the
  // wrong source/fields until the user hovers a second time.
  $: if ($currentDetail && entity && !detail && $currentDetail.entityId === entity.original_id) {
    detail = $currentDetail.detail;
  }

  function getNodeName(nodeId: string): string {
    const n = $graphData.nodes.find((n) => n.id === nodeId);
    return n ? n.name : nodeId;
  }

  function getOutgoing(d: D3Node) {
    return $graphData.links.filter((l) => (typeof l.source === 'object' ? l.source.id : l.source) === d.id);
  }

  function getIncoming(d: D3Node) {
    return $graphData.links.filter((l) => (typeof l.target === 'object' ? l.target.id : l.target) === d.id);
  }

  function sortByOrder(links: any[]) {
    return [...links].sort((a, b) => {
      if (a.order != null && b.order != null) return a.order - b.order;
      if (a.order != null) return -1;
      if (b.order != null) return 1;
      return 0;
    });
  }

  function getLinkTarget(link: any, direction: 'out' | 'in'): string {
    if (direction === 'out') {
      return typeof link.target === 'object' ? link.target.id : link.target;
    }
    return typeof link.source === 'object' ? link.source.id : link.source;
  }

  // Thresholds roughly match common industry heuristics (e.g., SonarQube,
  // McCabe's original paper). "warn" is "worth a look", "bad" is "refactor".
  function tierComplexity(v?: number): 'ok' | 'warn' | 'bad' | 'na' {
    if (v == null) return 'na';
    if (v <= 10) return 'ok';
    if (v <= 20) return 'warn';
    return 'bad';
  }
  function tierNesting(v?: number): 'ok' | 'warn' | 'bad' | 'na' {
    if (v == null) return 'na';
    if (v <= 3) return 'ok';
    if (v <= 5) return 'warn';
    return 'bad';
  }
  function tierLoc(v: number, isCallable: boolean): 'ok' | 'warn' | 'bad' {
    const hi = isCallable ? 60 : 200;
    const warn = isCallable ? 30 : 100;
    if (v <= warn) return 'ok';
    if (v <= hi) return 'warn';
    return 'bad';
  }
  function tierFanOut(v: number): 'ok' | 'warn' | 'bad' {
    if (v <= 7) return 'ok';
    if (v <= 15) return 'warn';
    return 'bad';
  }
  function tierParams(v?: number): 'ok' | 'warn' | 'bad' | 'na' {
    if (v == null) return 'na';
    if (v <= 4) return 'ok';
    if (v <= 6) return 'warn';
    return 'bad';
  }

  // Diff mode: per-entity change status + metric deltas.
  // Use normalizeEntityId to strip temp worktree paths for stable matching.
  $: diffStatus = entity && $diffStatusMap ? $diffStatusMap.get(normalizeEntityId(entity.original_id)) : undefined;
  $: diffDeltas = entity && $diffDeltaMap ? $diffDeltaMap.get(normalizeEntityId(entity.original_id)) : undefined;
  $: diffIsCore = entity && $diffSourceChangedMap ? $diffSourceChangedMap.get(normalizeEntityId(entity.original_id)) : undefined;

  // Diff: look up base source code for side-by-side comparison
  $: baseEntityId = entity && $diffBaseIdMap ? $diffBaseIdMap.get(normalizeEntityId(entity.original_id)) : undefined;
  $: baseSource = baseEntityId && $baseDetailsCache ? $baseDetailsCache[baseEntityId]?.source_code : undefined;
  $: showSplit = $diffActive && diffStatus === 'modified' && baseSource && detail?.source_code;
  $: diffLines = showSplit ? computeLineDiff(baseSource!, detail!.source_code!) : [];

  $: isCallable = !!entity && (entity.kind_raw === 'Function' || entity.kind_raw === 'Method');
  $: isContainer = !!entity && !isCallable;
  $: isEnum = entity?.kind_raw === 'Enum';
  $: metrics = entity?.metrics;

  // ─── UI-003 / UI-006 — structured details + tag chips ───────────────
  //
  // The mapping table is the single source of truth for which parser-
  // emitted attribute prefix renders as which human-readable row, and
  // which boolean tag warrants a chip. New entries are one line each;
  // unknown prefixes fall through to a generic "Other" list so the
  // panel never silently drops information.

  /** Attribute prefix → human-readable label. Order is preserved in
   *  the rendered output. */
  const DETAIL_LABELS: Array<[string, string]> = [
    ['bean', 'Spring bean'],
    ['caught', 'Catches'],
    ['manager', 'Resource'],
    ['pattern', 'Matches'],
    ['condition', 'Condition'],
    // ansible-deploy K8sResource attributes.
    ['k8s_kind', 'Kind'],
    ['k8s_name', 'Name'],
    ['k8s_namespace', 'Namespace'],
    ['k8s_replicas', 'Replicas'],
    ['k8s_image', 'Image'],
    ['k8s_type', 'Service type'],
    ['k8s_storage', 'Storage'],
  ];

  /** Tags that warrant a small inline chip in the panel. Pure
   *  signal tags (`branch_node`, `try_arm`, etc.) are excluded —
   *  they're already implied by the entity's display label and would
   *  just clutter the chip row. */
  const TAG_CHIP_LABELS: Record<string, string> = {
    async: 'async',
    null_safe: 'null-safe',
    module_state: 'module state',
    field_annotation: '@Field',
    synthetic: 'synthetic',
    unresolved: 'unresolved',
    dynamic_sql: 'dynamic SQL',
    dynamic_impex: 'dynamic Impex',
    abstract: 'abstract',
    constructor: 'constructor',
  };

  $: structuredDetails = (() => {
    if (!entity?.details) return [] as Array<{ label: string; value: string }>;
    const rows: Array<{ label: string; value: string }> = [];
    for (const [key, label] of DETAIL_LABELS) {
      const v = entity.details[key];
      if (v) rows.push({ label, value: v });
    }
    return rows;
  })();

  $: otherDetails = (() => {
    if (!entity?.details) return [] as Array<{ label: string; value: string }>;
    const known = new Set(DETAIL_LABELS.map(([k]) => k));
    return Object.entries(entity.details)
      .filter(([k]) => !known.has(k))
      .map(([k, v]) => ({ label: k, value: v }));
  })();

  // ─── UI-026 / UI-029 / UI-030 — Elevator ↔ code pairing ─────────────
  //
  // Code refs are *declared* by the spec author, not derived from the
  // code, so the panel says "Declares" rather than asserting the code
  // is the implementation. Resolution runs against the full graph
  // (inside the index), so narrowing the visual scope never turns a
  // healthy ref into drift.
  $: refs = entity?.codeRefs ?? [];
  $: isSpec = entity ? isSpecEntity(entity) : false;
  $: refRows = refs.map((ref) => ({
    ...ref,
    resolved: $codeRefIndex.resolves(ref.path),
  }));
  $: anchor = entity ? anchorState(entity, $codeRefIndex) : 'anchored';
  // A spec entity that never declared a `cr:` is unanchored; one whose
  // refs point at nothing is stale. Different fixes, different badges.
  $: showUnanchored = isSpec && anchor === 'unanchored' && !$codeRefIndex.empty;
  // Which concepts claim this file — only asked of code entities; a
  // spec entity's own relationship to code is the `refs` list above.
  $: claims = entity && !isSpec && !$codeRefIndex.empty
    ? $codeRefIndex.claimsFor(entity.file_path)
    : [];

  $: tagChips = (entity?.tags ?? [])
    .filter((t) => Object.prototype.hasOwnProperty.call(TAG_CHIP_LABELS, t))
    .map((t) => TAG_CHIP_LABELS[t]);

  /** UI-003 — most-specific label (e.g. "catch IOException", "case 1")
   *  for synthetic Branch/Loop entities; falls back to the entity's
   *  display_label or kind. The transform.ts derivation already
   *  produced display_label; this just plumbs it through. */
  $: kindLabel = entity?.display_label ?? entity?.kind ?? '';

  /** Class modifier for the kind badge. `kind_raw` is PascalCase
   *  (`GroovyScript`); convert to kebab-case for CSS (`groovy-script`)
   *  so component styles don't have to enumerate every kind. */
  function badgeClass(kindRaw: string): string {
    return kindRaw
      .replace(/([a-z0-9])([A-Z])/g, '$1-$2')
      .toLowerCase();
  }
  // Public-field ratio is only meaningful when the struct also has methods;
  // for pure data records we render it as informational (no tier).
  $: showPfr = isContainer && metrics?.public_field_ratio != null;
  $: pfrScored = showPfr && (metrics?.method_count ?? 0) > 3;

  function explain(key: keyof typeof METRIC_EXPLANATIONS): string {
    const e = METRIC_EXPLANATIONS[key];
    return `${e.title} — ${e.body}`;
  }

  let copyFeedback = '';
  let copyPathFeedback = '';
  let copyPathLineFeedback = '';
  let copyPathRangeFeedback = '';
  async function copyPath() {
    if (!entity) return;
    try {
      await navigator.clipboard.writeText(entity.file_path);
      copyPathFeedback = 'Copied!';
    } catch {
      copyPathFeedback = 'Failed';
    }
    setTimeout(() => { copyPathFeedback = ''; }, 1500);
  }
  async function copyPathLine() {
    if (!entity) return;
    try {
      await navigator.clipboard.writeText(`${entity.file_path}:${entity.line}`);
      copyPathLineFeedback = 'Copied!';
    } catch {
      copyPathLineFeedback = 'Failed';
    }
    setTimeout(() => { copyPathLineFeedback = ''; }, 1500);
  }
  async function copyPathRange() {
    if (!entity) return;
    try {
      await navigator.clipboard.writeText(`${entity.file_path}:${entity.line}-${entity.end_line}`);
      copyPathRangeFeedback = 'Copied!';
    } catch {
      copyPathRangeFeedback = 'Failed';
    }
    setTimeout(() => { copyPathRangeFeedback = ''; }, 1500);
  }
  function fmtNum(v: number | null | undefined): string {
    return v == null ? '—' : String(v);
  }
  function buildMetricsMarkdown(): string {
    if (!entity || !metrics) return '';
    const pfr = metrics.public_field_ratio != null
      ? `${Math.round(metrics.public_field_ratio * 100)}%`
      : '—';
    const header = '| Name | Kind | CC | Nest | LOC | Params | Fan-in | Fan-out | Fields/Variants | Methods | Pub% | Cycle |';
    const sep = '| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |';
    const row = `| ${entity.name} | ${entity.kind} | ${fmtNum(metrics.cyclomatic)} | ${fmtNum(metrics.max_nesting)} | ${metrics.loc} | ${fmtNum(metrics.param_count)} | ${metrics.fan_in} | ${metrics.fan_out} | ${fmtNum(metrics.field_count)} | ${metrics.method_count} | ${pfr} | ${metrics.in_cycle ? 'yes' : 'no'} |`;
    return `${header}\n${sep}\n${row}`;
  }
  async function copyMetrics() {
    const md = buildMetricsMarkdown();
    if (!md) return;
    try {
      await navigator.clipboard.writeText(md);
      copyFeedback = 'Copied!';
    } catch {
      copyFeedback = 'Failed';
    }
    setTimeout(() => { copyFeedback = ''; }, 1500);
  }
</script>

{#if entity}
  <div class="entity-info">
    <div class="detail-row">
      <span class="badge badge-{badgeClass(entity.kind_raw)}">{kindLabel}</span>
      {#if entity.tags?.includes('ghost')}
        <span class="badge badge-ghost">
          {entity.tags.includes('ghost_stdlib') ? 'Stdlib' : 'External'}
        </span>
      {/if}
      {#each tagChips as chip}
        <span class="tag-chip">{chip}</span>
      {/each}
    </div>
    <!-- Drill-in is the panel's primary navigation verb. It used to sit in
         the row above at tag size, reading as one more chip; it now owns a
         row. Outline rather than filled: the accent is light in the nord
         theme, so a fixed foreground on a filled accent would fail there
         (UI-012). -->
    {#if !compact && (entity.kind_raw === 'File' || entity.kind_raw === 'Module')}
      <button
        type="button"
        class="drill-btn"
        title="Narrow the scope to this {entity.kind_raw.toLowerCase()} and show its entities"
        on:click={() => {
          console.log('[drill] EntityInfo "Drill in" button clicked', entity?.kind_raw, entity?.original_id);
          if (entity) void drillIn(entity.original_id);
        }}
      >Drill into this {entity.kind_raw.toLowerCase()} ↓</button>
    {/if}
    <div class="detail-row">
      <div class="detail-label">Name</div>
      <div class="detail-value code">{entity.name}</div>
    </div>
    {#if entity.tags?.includes('ghost')}
      <div class="detail-row">
        <div class="detail-label">Qualified Name</div>
        <div class="detail-value code">{entity.qualified_name}</div>
      </div>
    {:else}
      <div class="detail-row">
        <div class="detail-label-row">
          <div class="detail-label">File</div>
          <span class="copy-btn-group">
            <button type="button" class="copy-btn" on:click={copyPath} title="Copy file path">
              {copyPathFeedback || '📋 Path'}
            </button>
            <button type="button" class="copy-btn" on:click={copyPathLine} title="Copy file path with line number">
              {copyPathLineFeedback || '📋 Path:Line'}
            </button>
            <button type="button" class="copy-btn" on:click={copyPathRange} title="Copy file path with line range">
              {copyPathRangeFeedback || '📋 Path:L-L'}
            </button>
          </span>
        </div>
        <div class="detail-value">{entity.file_path}:{entity.line}-{entity.end_line}</div>
      </div>
    {/if}
    <div class="detail-row">
      <div class="detail-label">Visibility</div>
      <div class="detail-value">{entity.visibility}</div>
    </div>

    <!-- Diff mode: change status badge + metric deltas -->
    {#if $diffActive && diffStatus}
      <div class="detail-row">
        <span class="diff-badge diff-{diffStatus}">{diffStatus}</span>
        {#if diffStatus === 'modified'}
          <span class="diff-type-badge" class:core={diffIsCore} class:impact={!diffIsCore}>
            {diffIsCore ? 'core' : 'impact'}
          </span>
        {/if}
      </div>
      {#if diffDeltas && diffDeltas.length > 0}
        <div class="detail-row">
          <div class="detail-label">Changes</div>
          <div class="diff-deltas">
            {#each diffDeltas as d}
              <div class="diff-delta" class:positive={d.delta > 0} class:negative={d.delta < 0}>
                <span class="delta-name">{d.name}</span>
                <span class="delta-old">{d.old != null ? d.old : '—'}</span>
                <span class="delta-arrow">→</span>
                <span class="delta-new">{d.new != null ? d.new : '—'}</span>
                <span class="delta-change">({d.delta > 0 ? '+' : ''}{d.delta % 1 === 0 ? d.delta : d.delta.toFixed(2)})</span>
              </div>
            {/each}
          </div>
        </div>
      {/if}
    {/if}

    <!-- Quality metrics: precomputed server-side, just display. -->
    {#if metrics}
      <div class="detail-row">
        <div class="detail-label-row">
          <div class="detail-label">Quality</div>
          <button type="button" class="copy-btn" on:click={copyMetrics} title="Copy metrics as markdown table">
            {copyFeedback || '📋 Copy'}
          </button>
        </div>
        <div class="metrics-grid">
          {#if metrics.cyclomatic != null}
            <div class="metric metric-{tierComplexity(metrics.cyclomatic)} help" data-tip={explain('cc')} aria-label={explain('cc')}>
              <span class="metric-label">CC</span>
              <span class="metric-value">{metrics.cyclomatic}</span>
            </div>
          {/if}
          {#if metrics.cognitive_complexity != null}
            <div class="metric metric-{tierCognitive(metrics.cognitive_complexity)} help" data-tip={explain('cognitive')} aria-label={explain('cognitive')}>
              <span class="metric-label">Cog</span>
              <span class="metric-value">{metrics.cognitive_complexity}</span>
            </div>
          {/if}
          {#if metrics.max_nesting != null}
            <div class="metric metric-{tierNesting(metrics.max_nesting)} help" data-tip={explain('nest')} aria-label={explain('nest')}>
              <span class="metric-label">Nest</span>
              <span class="metric-value">{metrics.max_nesting}</span>
            </div>
          {/if}
          <div class="metric metric-{tierLoc(metrics.loc, isCallable)} help" data-tip={explain('loc')} aria-label={explain('loc')}>
            <span class="metric-label">LOC</span>
            <span class="metric-value">{metrics.loc}</span>
          </div>
          {#if metrics.param_count != null}
            <div class="metric metric-{tierParams(metrics.param_count)} help" data-tip={explain('params')} aria-label={explain('params')}>
              <span class="metric-label">Params</span>
              <span class="metric-value">{metrics.param_count}</span>
            </div>
          {/if}
          <div class="metric metric-ok help" data-tip={explain('fan_in')} aria-label={explain('fan_in')}>
            <span class="metric-label">Fan-in</span>
            <span class="metric-value">{metrics.fan_in}</span>
          </div>
          <div class="metric metric-{tierFanOut(metrics.fan_out)} help" data-tip={explain('fan_out')} aria-label={explain('fan_out')}>
            <span class="metric-label">Fan-out</span>
            <span class="metric-value">{metrics.fan_out}</span>
          </div>
          {#if metrics.in_cycle}
            <div class="metric metric-bad help" data-tip={explain('cycle')} aria-label={explain('cycle')}>
              <span class="metric-label">Cycle</span>
              <span class="metric-value">!</span>
            </div>
          {/if}
          {#if isContainer && metrics.field_count != null}
            <div class="metric metric-{tierFieldCount(metrics.field_count, isEnum)} help" data-tip={explain('field_count')} aria-label={explain('field_count')}>
              <span class="metric-label">{isEnum ? 'Variants' : 'Fields'}</span>
              <span class="metric-value">{metrics.field_count}</span>
            </div>
          {/if}
          {#if isContainer}
            <div class="metric metric-{tierMethodCount(metrics.method_count)} help" data-tip={explain('method_count')} aria-label={explain('method_count')}>
              <span class="metric-label">Methods</span>
              <span class="metric-value">{metrics.method_count}</span>
            </div>
          {/if}
          {#if showPfr}
            <div class="metric metric-{pfrScored ? tierPublicFieldRatio(metrics.public_field_ratio) : 'ok'} help" data-tip={explain('public_field_ratio')} aria-label={explain('public_field_ratio')}>
              <span class="metric-label">Pub %</span>
              <span class="metric-value">{Math.round((metrics.public_field_ratio ?? 0) * 100)}%</span>
            </div>
          {/if}
        </div>
        {#if metrics.smells?.length}
          <div class="smell-row">
            {#each metrics.smells as s}
              {@const meta = SMELL_META[s]}
              <span class="smell-badge" title={meta?.hint ?? s}>{meta?.label ?? s}</span>
            {/each}
          </div>
        {/if}
      </div>
    {/if}

    <!-- UI-006 — structured attribute rows for known prefixes
         (`bean:`, `caught:`, `manager:`, `pattern:`, `condition:`).
         Unknown prefixes fall through to the "Other" list below so
         the panel never silently drops a parser-emitted attribute. -->
    {#each structuredDetails as row}
      <div class="detail-row">
        <div class="detail-label">{row.label}</div>
        <div class="detail-value code">{row.value}</div>
      </div>
    {/each}
    {#if otherDetails.length > 0}
      <div class="detail-row">
        <div class="detail-label">Other</div>
        <div class="detail-value">
          {#each otherDetails as row, i}
            {#if i > 0}, {/if}<span class="other-detail"><strong>{row.label}:</strong> {row.value}</span>
          {/each}
        </div>
      </div>
    {/if}

    <!-- UI-026 — Elevator code references. Declared in the `.elv`
         spec, so they are leads, not proof: an unresolved one is
         flagged (UI-030) rather than silently rendering as fine. -->
    {#if refRows.length > 0}
      <div class="detail-row">
        <div class="detail-label-row">
          <div class="detail-label">Declares ({refRows.length})</div>
          <!-- UI-028 — pairing is expressed as scope, not as a fourth
               emphasis channel (ADR 0005). Disabled rather than hidden
               when every ref is stale, so the reason is visible. -->
          <button
            type="button"
            class="copy-btn pair-btn"
            disabled={!refRows.some((r) => r.resolved)}
            title={refRows.some((r) => r.resolved)
              ? 'Scope the graph to the code this entity declares'
              : 'Every declared path is unresolved — there is no code to show'}
            on:click={() => { if (entity) void showImplementingCode(entity); }}
          >Show code</button>
        </div>
        <div class="detail-value">
          {#each refRows as ref}
            <div class="code-ref" class:unresolved={!ref.resolved}>
              {#if ref.tag}<span class="code-ref-tag">{ref.tag}</span>{/if}
              <span class="code-ref-path code">{ref.path}</span>
              {#if !ref.resolved}
                <span class="code-ref-flag" title="No file or folder in the analysed tree matches this path — the spec has drifted from the code.">UNRESOLVED</span>
              {/if}
            </div>
          {/each}
        </div>
      </div>
    {/if}

    <!-- UI-030 — distinct from a stale ref: nobody ever said where
         this concept lives. -->
    {#if showUnanchored}
      <div class="detail-row">
        <div class="detail-label">Declares</div>
        <div class="detail-value">
          <span class="code-ref-flag warn" title="This spec entity declares no `cr:` code reference, so nothing ties it to the codebase.">UNANCHORED</span>
        </div>
      </div>
    {/if}

    <!-- UI-029 — the reverse direction: which domain concepts claim
         this file. Resolved through ancestor folders, most-specific
         claim first. -->
    {#if claims.length > 0}
      <div class="detail-row">
        <div class="detail-label">Claimed by ({claims.length})</div>
        <div class="detail-value">
          {#each claims as claim}
            <div class="code-ref">
              <button
                type="button"
                class="claim-btn"
                title="Show {claim.node.qualified_name} — declared via {claim.path}"
                on:click={() => void revealSpecEntity(claim.node)}
              >
                {claim.node.qualified_name}
              </button>
              {#if claim.tag}<span class="code-ref-tag">{claim.tag}</span>{/if}
              <span class="claim-path">{claim.path}</span>
              {#if claim.duplicate}
                <span class="code-ref-flag warn" title="Another spec entity declares this same path under the same `cr` kind — likely the same functionality named twice.">DUPLICATE</span>
              {/if}
            </div>
          {/each}
        </div>
      </div>
    {/if}

    {#if entity.extends && entity.extends.length > 0}
      <div class="detail-row">
        <div class="detail-label">Extends</div>
        <div class="detail-value">{entity.extends.join(', ')}</div>
      </div>
    {/if}

    {#if entity.implements.length > 0}
      <div class="detail-row">
        <div class="detail-label">{entity.tags.includes('trait_impl') ? 'Implements Trait' : 'Implements'}</div>
        <div class="detail-value">{entity.implements.join(', ')}</div>
      </div>
    {/if}

    <!-- Parameters -->
    {#if entity.parameters.length > 0}
      <div class="detail-row">
        <div class="detail-label">Parameters ({entity.parameters.length})</div>
      </div>
      <table class="fields-table">
        <thead><tr><th>Name</th><th>Type</th></tr></thead>
        <tbody>
          {#each entity.parameters as param}
            {@const parts = param.split(':')}
            <tr>
              <td class="field-name">{parts[0].trim()}</td>
              <td class="field-type">{parts.length > 1 ? parts.slice(1).join(':').trim() : ''}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}

    {#if entity.return_type}
      <div class="detail-row">
        <div class="detail-label">Returns</div>
        <div class="detail-value code">{entity.return_type}</div>
      </div>
    {/if}

    <!-- Fields (loaded from detail sidecar) -->
    {#if detail?.fields && detail.fields.length > 0}
      {@const fieldLabel = entity.kind_raw === 'Enum' ? 'Variants' : 'Fields'}
      {@const typeHeader = entity.kind_raw === 'Enum' ? 'Data' : 'Type'}
      <div class="detail-row">
        <div class="detail-label">{fieldLabel} ({detail.fields.length})</div>
      </div>
      <table class="fields-table">
        <thead><tr><th>Name</th><th>{typeHeader}</th></tr></thead>
        <tbody>
          {#each detail.fields as field}
            <tr>
              <td class="field-name">{field.name}</td>
              <td class="field-type">{field.type_name || ''}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}

    <!-- Documentation (Javadoc / Rustdoc / KDoc) -->
    {#if detail?.documentation}
      <div class="detail-row">
        <div class="detail-label">Documentation</div>
        <div class="doc-block"><pre>{detail.documentation}</pre></div>
      </div>
    {/if}

    <!-- Source code (loaded from detail sidecar) -->
    {#if showSource && detail?.source_code}
      <div class="detail-row">
        <div class="detail-label">{entity.kind_raw === 'File' ? 'Source' : 'Definition'}</div>
        {#if showSplit}
          <div class="diff-view" class:compact>
            <div class="diff-header">
              <span class="diff-header-label split-label-removed">Base</span>
              <span class="diff-header-label split-label-added">Head</span>
            </div>
            <div class="diff-lines">
              {#each diffLines as line}
                <div class="diff-line diff-line-{line.kind}">
                  <span class="diff-gutter diff-gutter-base">{line.baseLine ?? ''}</span>
                  <span class="diff-gutter diff-gutter-head">{line.headLine ?? ''}</span>
                  <span class="diff-marker">{line.kind === 'added' ? '+' : line.kind === 'removed' ? '-' : ' '}</span>
                  <pre class="diff-text">{line.text}</pre>
                </div>
              {/each}
            </div>
          </div>
        {:else if $diffActive && diffStatus === 'removed' && baseSource}
          <div class="source-code-block" class:compact><pre>{baseSource}</pre></div>
        {:else}
          <div class="source-code-block" class:compact><pre>{detail.source_code}</pre></div>
        {/if}
      </div>
    {:else if showSource && $diffActive && diffStatus === 'removed' && baseSource}
      <div class="detail-row">
        <div class="detail-label">Definition (removed)</div>
        <div class="source-code-block" class:compact><pre>{baseSource}</pre></div>
      </div>
    {/if}

    {#if showSource && detail?.impl_blocks && detail.impl_blocks.length > 0}
      {#each detail.impl_blocks as block, i}
        <div class="detail-row">
          <div class="detail-label">
            {detail.impl_blocks.length === 1 ? 'Implementation' : `Implementation (${i + 1}/${detail.impl_blocks.length})`}
          </div>
          <div class="source-code-block" class:compact><pre>{block}</pre></div>
        </div>
      {/each}
    {/if}

    <!-- Relationships -->
    {#if showRelationships}
      {@const outgoing = getOutgoing(entity)}
      {@const incoming = getIncoming(entity)}
      {#if outgoing.length > 0 || incoming.length > 0}
        <div class="detail-row">
          <div class="detail-label">Relationships ({outgoing.length + incoming.length})</div>
        </div>
        {#if outgoing.length > 0}
          <div class="rel-section-header">Outgoing ({outgoing.length})</div>
          <div class="rel-list">
            {#each sortByOrder(outgoing) as link}
              {@const targetId = getLinkTarget(link, 'out')}
              <div class="rel-item">
                {#if link.order != null}
                  <span class="rel-order">{link.order}</span>
                {/if}
                <span class="rel-arrow rel-out">&#10132;</span>
                <span class="rel-kind"><ColorChip color={LINK_COLORS[link.kind_raw] || 'var(--text-dim)'} label={link.kind} dim /></span>
                {#if link.tags && link.tags.length > 0}
                  {#each link.tags as t}
                    <span class="rel-tag rel-tag-{t}">{t.replaceAll('_', '-')}</span>
                  {/each}
                {/if}
                <span class="rel-target">{getNodeName(targetId)}</span>
                {#if link.binds_to}
                  <span class="rel-bind" title="Bound to local variable">→ {link.binds_to}{#if link.binds_type}: {link.binds_type}{/if}</span>
                {:else if link.rebinds_to}
                  <span class="rel-bind rel-rebind" title="Reassigned to existing variable">⟲ {link.rebinds_to}</span>
                {/if}
              </div>
            {/each}
          </div>
        {/if}
        {#if incoming.length > 0}
          <div class="rel-section-header">Incoming ({incoming.length})</div>
          <div class="rel-list">
            {#each sortByOrder(incoming) as link}
              {@const sourceId = getLinkTarget(link, 'in')}
              <div class="rel-item">
                {#if link.order != null}
                  <span class="rel-order">{link.order}</span>
                {/if}
                <span class="rel-arrow rel-in">&#11136;</span>
                <span class="rel-kind"><ColorChip color={LINK_COLORS[link.kind_raw] || 'var(--text-dim)'} label={link.incoming_kind || link.kind} dim /></span>
                {#if link.tags && link.tags.length > 0}
                  {#each link.tags as t}
                    <span class="rel-tag rel-tag-{t}">{t.replaceAll('_', '-')}</span>
                  {/each}
                {/if}
                <span class="rel-target">{getNodeName(sourceId)}</span>
                {#if link.binds_to}
                  <span class="rel-bind" title="Caller binds the result to a local variable">→ {link.binds_to}{#if link.binds_type}: {link.binds_type}{/if}</span>
                {:else if link.rebinds_to}
                  <span class="rel-bind rel-rebind" title="Caller reassigns to an existing variable">⟲ {link.rebinds_to}</span>
                {/if}
              </div>
            {/each}
          </div>
        {/if}
      {/if}
    {/if}
  </div>
{/if}

<style>
  /* Direction arrows were #2196F3 / #E91E63 — 3.12:1 on the light panel.
     They are decorative: the relationship label beside them already states
     the direction, so a theme token loses nothing (UI-023). */
  .rel-out { color: var(--text-muted); }
  .rel-in { color: var(--text-muted); }

  .entity-info {
    font-size: 0.9rem;
  }

  .detail-row {
    margin-bottom: 6px;
  }

  .detail-label {
    font-size: 0.75rem;
    color: var(--text-dim);
    text-transform: uppercase;
  }

  .detail-value {
    font-size: 0.9rem;
    color: var(--text);
    word-break: break-all;
  }

  .detail-value.code {
    font-family: 'Monaco', 'Menlo', monospace;
    background: var(--bg-hover);
    padding: 2px 6px;
    border-radius: 3px;
  }

  .badge {
    display: inline-block;
    padding: 2px 8px;
    border-radius: 12px;
    font-size: 0.75rem;
    font-weight: 500;
    color: var(--text);
  }

  .badge-ghost {
    background: rgba(158, 158, 158, 0.15);
    color: #BDBDBD;
    border: 1px solid rgba(158, 158, 158, 0.4);
    font-style: italic;
  }

  .drill-btn {
    display: block;
    width: 100%;
    margin: 4px 0 10px;
    padding: 7px 12px;
    border-radius: 6px;
    font: inherit;
    font-size: 0.85rem;
    font-weight: 600;
    border: 1px solid var(--accent);
    background: color-mix(in srgb, var(--accent) 12%, transparent);
    color: var(--accent);
    cursor: pointer;
  }
  .drill-btn:hover {
    background: color-mix(in srgb, var(--accent) 25%, transparent);
  }

  .diff-badge {
    display: inline-block;
    padding: 2px 10px;
    border-radius: 12px;
    font-size: 0.75rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .diff-added { background: rgba(76, 175, 80, 0.15); color: #A5D6A7; border: 1px solid rgba(76, 175, 80, 0.4); }
  .diff-removed { background: rgba(244, 67, 54, 0.15); color: #EF9A9A; border: 1px solid rgba(244, 67, 54, 0.4); }
  .diff-modified { background: rgba(255, 167, 38, 0.15); color: #FFCC80; border: 1px solid rgba(255, 167, 38, 0.4); }
  .diff-unchanged { background: rgba(158, 158, 158, 0.1); color: #BDBDBD; border: 1px solid rgba(158, 158, 158, 0.3); }

  .diff-type-badge {
    display: inline-block;
    padding: 2px 8px;
    border-radius: 10px;
    font-size: 0.65rem;
    font-weight: 500;
    text-transform: lowercase;
    margin-left: 6px;
  }
  .diff-type-badge.core {
    background: rgba(255, 167, 38, 0.2);
    color: #FFCC80;
    border: 1px solid rgba(255, 167, 38, 0.4);
  }
  .diff-type-badge.impact {
    background: rgba(100, 181, 246, 0.1);
    color: #90CAF9;
    border: 1px solid rgba(100, 181, 246, 0.3);
  }

  .diff-deltas {
    display: flex;
    flex-direction: column;
    gap: 3px;
    margin-top: 4px;
    font-family: 'Monaco', 'Menlo', monospace;
    font-size: 0.75rem;
  }
  .diff-delta {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 2px 6px;
    border-radius: 3px;
    background: var(--bg-surface-alt);
  }
  .diff-delta.positive .delta-change { color: #EF9A9A; }
  .diff-delta.negative .delta-change { color: #A5D6A7; }
  .delta-name { color: var(--text-dim); min-width: 90px; font-size: 0.7rem; text-transform: uppercase; }
  .delta-old { color: var(--text-muted); }
  .delta-arrow { color: var(--text-disabled); }
  .delta-new { color: var(--text); font-weight: 600; }
  .delta-change { font-weight: 700; font-size: 0.7rem; }

  .doc-block {
    background: var(--bg-deep);
    border: 1px solid var(--border);
    border-left: 3px solid #4CAF50;
    border-radius: 4px;
    padding: 10px 12px;
    margin-top: 6px;
    overflow-x: auto;
  }

  .doc-block pre {
    margin: 0;
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif;
    font-size: 0.78rem;
    line-height: 1.5;
    color: var(--text-secondary, #c9d1d9);
    white-space: pre-wrap;
    word-wrap: break-word;
  }

  .source-code-block {
    background: var(--bg-deep);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 12px;
    margin-top: 8px;
    overflow-x: auto;
    /* Source lines are wider than a 320px sidebar column and must not wrap
       (indentation carries meaning), so the block scrolls. macOS overlay
       scrollbars hide until you scroll, which left no cue that there was
       anything to the right — the listing simply looked truncated. Force a
       persistent scrollbar; the colours come from the global rule in
       app.css. This block used to paint its own thumb in --border-subtle,
       which sits *below* the surface colour on the darker themes and so
       read as no scrollbar at all. */
    scrollbar-width: thin;
  }

  .source-code-block::-webkit-scrollbar {
    height: 8px;
  }

  .source-code-block pre {
    margin: 0;
    font-family: 'Monaco', 'Menlo', 'Consolas', monospace;
    font-size: 0.75rem;
    line-height: 1.5;
    color: var(--text-secondary);
    white-space: pre;
    tab-size: 4;
  }

  /* Unified diff view */
  .diff-view {
    background: var(--bg-deep);
    border: 1px solid var(--border);
    border-radius: 6px;
    margin-top: 8px;
    overflow-x: auto;
  }

  .diff-view.compact {
    max-height: 300px;
    overflow-y: auto;
  }

  .diff-header {
    display: flex;
    gap: 0;
    border-bottom: 1px solid var(--border);
  }

  .diff-header-label {
    flex: 1;
    font-size: 0.65rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 4px 8px;
    text-align: center;
  }

  .split-label-removed {
    background: rgba(244, 67, 54, 0.15);
    color: #EF9A9A;
  }

  .split-label-added {
    background: rgba(76, 175, 80, 0.15);
    color: #A5D6A7;
  }

  .diff-lines {
    font-family: 'Monaco', 'Menlo', 'Consolas', monospace;
    font-size: 0.75rem;
    line-height: 1.5;
  }

  .diff-line {
    display: flex;
    align-items: stretch;
    min-height: 1.5em;
  }

  .diff-line-equal {
    color: var(--text-dim);
  }

  .diff-line-added {
    background: rgba(76, 175, 80, 0.1);
    color: #A5D6A7;
  }

  .diff-line-removed {
    background: rgba(244, 67, 54, 0.1);
    color: #EF9A9A;
  }

  .diff-gutter {
    flex-shrink: 0;
    width: 32px;
    text-align: right;
    padding: 0 4px;
    color: var(--text-dim);
    font-size: 0.65rem;
    opacity: 0.5;
    border-right: 1px solid var(--border-subtle);
    user-select: none;
  }

  .diff-marker {
    flex-shrink: 0;
    width: 16px;
    text-align: center;
    user-select: none;
    font-weight: 700;
  }

  .diff-line-added .diff-marker { color: #4CAF50; }
  .diff-line-removed .diff-marker { color: #F44336; }

  .diff-text {
    margin: 0;
    padding: 0 8px 0 4px;
    white-space: pre;
    tab-size: 4;
    flex: 1;
    min-width: 0;
  }

  .fields-table {
    width: 100%;
    border-collapse: collapse;
    margin-top: 4px;
    font-size: 0.8rem;
  }

  .fields-table th {
    text-align: left;
    font-size: 0.7rem;
    color: var(--text-dim);
    text-transform: uppercase;
    padding: 3px 6px;
    border-bottom: 1px solid var(--border);
  }

  .fields-table td {
    padding: 3px 6px;
    font-family: 'Monaco', 'Menlo', 'Consolas', monospace;
    font-size: 0.75rem;
  }

  .fields-table tr:hover {
    background: color-mix(in srgb, var(--bg-hover) 30%, transparent);
  }

  .fields-table .field-name { color: var(--text-secondary); }
  .fields-table .field-type { color: #81C784; }

  .rel-section-header {
    font-size: 0.75rem;
    color: var(--text-dim);
    text-transform: uppercase;
    margin-top: 8px;
    margin-bottom: 4px;
    padding-bottom: 2px;
    border-bottom: 1px solid color-mix(in srgb, var(--border) 50%, transparent);
  }

  .rel-item {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 6px;
    border-radius: 4px;
    font-size: 0.8rem;
    cursor: pointer;
    margin-bottom: 2px;
  }

  .rel-item:hover { background: var(--bg-hover); }

  .rel-order {
    font-size: 0.65rem;
    font-weight: 700;
    color: #FF9800;
    background: rgba(255, 152, 0, 0.15);
    border-radius: 3px;
    padding: 0 4px;
    min-width: 18px;
    text-align: center;
    flex-shrink: 0;
  }

  .rel-arrow {
    font-size: 0.7rem;
    flex-shrink: 0;
    width: 16px;
    text-align: center;
  }

  .rel-kind {
    font-size: 0.7rem;
    padding: 1px 5px;
    border-radius: 3px;
    background: color-mix(in srgb, var(--bg-hover) 70%, transparent);
    flex-shrink: 0;
  }

  .rel-target {
    /* Was #8cb4ff — 2.08:1 on the light panel. It reads as a link, so the
       accent carries the same meaning while tracking the theme (UI-023). */
    color: var(--accent);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .rel-bind {
    display: inline-block;
    margin-left: 4px;
    padding: 0 5px;
    font-size: 0.65rem;
    font-family: var(--font-mono, monospace);
    border-radius: 3px;
    background: rgba(76, 175, 80, 0.14);
    color: #A5D6A7;
    border: 1px solid rgba(76, 175, 80, 0.35);
    white-space: nowrap;
    flex-shrink: 0;
  }

  .rel-bind.rel-rebind {
    background: rgba(255, 167, 38, 0.14);
    color: #FFCC80;
    border-color: rgba(255, 167, 38, 0.4);
  }

  .detail-label-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }

  .copy-btn {
    background: color-mix(in srgb, var(--bg-hover) 70%, transparent);
    color: var(--text-secondary);
    border: 1px solid var(--border-subtle);
    border-radius: 4px;
    padding: 2px 8px;
    font-size: 0.7rem;
    font-family: inherit;
    cursor: pointer;
    transition: background 0.12s ease, border-color 0.12s ease;
  }

  .copy-btn-group {
    display: flex;
    gap: 4px;
  }

  .copy-btn:hover {
    background: var(--bg-hover);
    border-color: #4CAF50;
  }

  .metrics-grid {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin-top: 4px;
  }

  .metric {
    display: inline-flex;
    align-items: baseline;
    gap: 4px;
    padding: 2px 6px;
    border-radius: 4px;
    font-size: 0.7rem;
    font-family: 'Monaco', 'Menlo', monospace;
    border: 1px solid transparent;
  }

  .metric-label {
    color: var(--text-dim);
    text-transform: uppercase;
    font-size: 0.65rem;
  }

  .metric-value {
    font-weight: 700;
  }

  .metric-ok { background: rgba(76, 175, 80, 0.12); border-color: rgba(76, 175, 80, 0.4); color: var(--text); }
  .metric-warn { background: rgba(255, 152, 0, 0.15); border-color: rgba(255, 152, 0, 0.5); color: var(--text); }
  .metric-bad { background: rgba(244, 67, 54, 0.15); border-color: rgba(244, 67, 54, 0.5); color: var(--text); }
  .metric-na { background: rgba(158, 158, 158, 0.1); border-color: rgba(158, 158, 158, 0.3); color: var(--text); }

  .smell-row {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin-top: 6px;
  }
  .smell-badge {
    display: inline-block;
    padding: 2px 7px;
    border-radius: 3px;
    font-size: 0.7rem;
    background: rgba(244, 67, 54, 0.12);
    color: #EF9A9A;
    border: 1px solid rgba(244, 67, 54, 0.3);
    cursor: help;
  }

  /* Rich hover tooltip — mirrors QualityReport. */
  .help { position: relative; cursor: help; }
  .help::after {
    content: attr(data-tip);
    position: absolute;
    left: 0;
    top: calc(100% + 4px);
    z-index: 10;
    width: 240px;
    max-width: 85vw;
    padding: 8px 10px;
    background: var(--bg-deep);
    color: var(--text-secondary);
    border: 1px solid var(--border-subtle);
    border-radius: 4px;
    font-size: 0.72rem;
    font-family: -apple-system, BlinkMacSystemFont, sans-serif;
    font-weight: 400;
    line-height: 1.45;
    white-space: normal;
    text-align: left;
    pointer-events: none;
    opacity: 0;
    transform: translateY(-2px);
    transition: opacity 0.12s ease, transform 0.12s ease;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.4);
  }
  .help:hover::after, .help:focus::after {
    opacity: 1;
    transform: translateY(0);
  }

  /* UI-001 — Groovy script container badge. Olive accent ties to the
     NODE_COLORS entry so the graph node and the panel badge read as
     one family. */
  .badge-groovy-script {
    background: rgba(161, 181, 108, 0.18);
    color: #C5DEA0;
    border: 1px solid rgba(161, 181, 108, 0.45);
  }

  /* UI-002 — synthetic infrastructure entities. Lavender for beans
     (Spring infrastructure), amber for tables (data infrastructure).
     Both have border + background to survive light/dark themes. */
  .badge-bean {
    background: rgba(149, 117, 205, 0.18);
    color: #D1C4E9;
    border: 1px solid rgba(149, 117, 205, 0.45);
  }
  .badge-table {
    background: rgba(255, 183, 77, 0.18);
    color: #FFD180;
    border: 1px solid rgba(255, 183, 77, 0.45);
  }

  /* UI-006 — boolean-tag chips next to the kind badge. Smaller +
     more muted than the kind badge so they read as supplemental. */
  .tag-chip {
    display: inline-block;
    padding: 1px 7px;
    border-radius: 10px;
    font-size: 0.65rem;
    font-weight: 500;
    background: rgba(120, 144, 156, 0.18);
    color: #B0BEC5;
    border: 1px solid rgba(120, 144, 156, 0.4);
    text-transform: lowercase;
    letter-spacing: 0.02em;
  }

  /* UI-006 — fall-through "Other" attribute list. Subtle inline
     formatting so the list reads as one row, not a section. */
  .other-detail {
    color: var(--text-muted);
  }
  .other-detail strong {
    color: var(--text);
    font-weight: 500;
  }

  /* UI-026 / UI-029 / UI-030 — Elevator code references and the
     concepts claiming a file. Colours come from the tier tokens, which
     are already contrast-tuned per theme: stale reads as `bad`,
     unanchored and duplicate as `warn`. No literals — a fixed colour
     here would be wrong in at least one of the five themes (UI-025). */
  .code-ref {
    display: flex;
    align-items: baseline;
    flex-wrap: wrap;
    gap: 6px;
    padding: 1px 0;
  }
  .code-ref-path {
    color: var(--text);
    word-break: break-all;
  }
  .code-ref.unresolved .code-ref-path {
    color: var(--text-muted);
    text-decoration: line-through;
  }
  .code-ref-tag {
    flex: none;
    padding: 0 5px;
    border-radius: 3px;
    font-size: 0.6rem;
    font-weight: 600;
    color: var(--text-secondary);
    border: 1px solid var(--border);
    text-transform: lowercase;
  }
  /* Uppercase word, not a colour-only cue — readable when the theme
     flattens the palette and for colour-blind readers. */
  .code-ref-flag {
    flex: none;
    font-size: 0.6rem;
    font-weight: 700;
    letter-spacing: 0.04em;
    color: var(--tier-bad-fg);
  }
  /* Stale (`UNRESOLVED`) is a broken anchor and reads as `bad`;
     `UNANCHORED` and `DUPLICATE` are things to look at, not breakage. */
  .code-ref-flag.warn {
    color: var(--tier-warn-fg);
  }
  /* UI-028 — sits in the `Declares` label row, so it inherits the
     copy-btn chrome and only needs a disabled state of its own. */
  .pair-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .claim-btn {
    padding: 0;
    background: none;
    border: none;
    font: inherit;
    color: var(--accent);
    cursor: pointer;
    text-align: left;
  }
  .claim-btn:hover {
    text-decoration: underline;
  }
  .claim-path {
    color: var(--text-dim);
    font-size: 0.7rem;
    word-break: break-all;
  }

  /* UI-004 — relationship-tag badges. Default styling muted; named
     overrides give the reader a non-colour-only signal (font weight)
     plus a tag-specific accent for the most common ones. */
  .rel-tag {
    display: inline-block;
    padding: 0 5px;
    border-radius: 3px;
    font-size: 0.6rem;
    font-weight: 600;
    text-transform: lowercase;
    letter-spacing: 0.02em;
    background: rgba(120, 144, 156, 0.18);
    color: #90A4AE;
    border: 1px solid rgba(120, 144, 156, 0.4);
    margin-left: 2px;
  }
  .rel-tag-null_safe {
    background: rgba(38, 198, 218, 0.18);
    color: #80DEEA;
    border-color: rgba(38, 198, 218, 0.4);
  }
  .rel-tag-bean_lookup {
    background: rgba(149, 117, 205, 0.18);
    color: #D1C4E9;
    border-color: rgba(149, 117, 205, 0.4);
  }
  .rel-tag-spread, .rel-tag-spread_args {
    background: rgba(126, 87, 194, 0.18);
    color: #B39DDB;
    border-color: rgba(126, 87, 194, 0.4);
  }
  .rel-tag-dynamic_sql, .rel-tag-dynamic_impex, .rel-tag-unresolved {
    background: rgba(255, 167, 38, 0.18);
    color: #FFCC80;
    border-color: rgba(255, 167, 38, 0.45);
  }
</style>
