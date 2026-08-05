<script lang="ts">
  import { selectedNode, graphData, rawEntityGraph, hiddenFiles } from '../stores/graph';
  import {
    qualityRows,
    qualitySummary,
    fileRows,
    moduleRows,
    repoQuality,
    scopeHolds,
    METRIC_EXPLANATIONS,
    SMELL_META,
    SCORE_SCALE_LABEL,
    SUMMARY_METRIC_THRESHOLDS,
    thresholdStore,
    qualityAnalysisScope,
    currentEditorFile,
    type QualityAnalysisScope,
    type QualityRow,
    type ScopeRow,
    type Tier,
  } from '../stores/quality';
  import { clearHiddenFiles } from '../viewmodels/filterViewModel';
  import { diffData } from '../stores/diff';
  import { setScopes, selectedScopes } from '../stores/scope';
  import { fetchRefactorPrompt, spawnAgentTerminal } from '../viewmodels/contextScope';
  import { connection } from '../stores/connection';
  import { copyToClipboard } from '../utils/clipboard';
  import type { D3Node } from '../types/graph';
  import Scatter from './Scatter.svelte';

  // Look up a parent entity's display name by its parent_id. The parent_id
  // stored on a D3Node is the original (non-sanitized) entity id OR, for
  // Rust impl blocks, sometimes just the bare type name — try both.
  // Memoized per graph snapshot so we don't scan the node list per row.
  let parentLookup: Map<string, string> = new Map();
  $: {
    parentLookup = new Map();
    for (const n of $graphData.nodes) {
      parentLookup.set(n.original_id, n.name);
      if (!parentLookup.has(n.name)) parentLookup.set(n.name, n.name);
    }
  }
  function parentName(node: D3Node): string {
    if (!node.parent_id) return '';
    return parentLookup.get(node.parent_id) ?? node.parent_id;
  }
  function fileBase(path: string): string {
    const i = path.lastIndexOf('/');
    return i >= 0 ? path.slice(i + 1) : path;
  }
  function fileDir(path: string): string {
    const i = path.lastIndexOf('/');
    return i >= 0 ? path.slice(0, i) : '';
  }

  type Mode = 'entities' | 'files' | 'modules';
  let mode: Mode = 'entities';

  type SortKey =
    | 'score' | 'name' | 'cc' | 'cognitive' | 'nest' | 'loc' | 'params'
    | 'fan_in' | 'fan_out' | 'fields' | 'methods' | 'pub_ratio'
    | 'path' | 'entity_count' | 'callable_count' | 'scope_loc' | 'cohesion'
    | 'scope_fan_in' | 'scope_fan_out';
  let sortKey: SortKey = 'score';
  let sortDir: 'asc' | 'desc' = 'desc';

  // Filters
  let kindFilter: 'all' | 'callable' | 'container' = 'all';
  let severityFilter: 'all' | 'bad' | 'warnbad' | 'cycle' | 'smell' = 'all';
  let limit = 100;

  function toggleSort(key: SortKey) {
    if (sortKey === key) {
      sortDir = sortDir === 'asc' ? 'desc' : 'asc';
    } else {
      sortKey = key;
      // Sensible default: text asc, numbers desc.
      sortDir = key === 'name' ? 'asc' : 'desc';
    }
  }

  function rowValue(r: QualityRow, key: SortKey): number | string {
    const m = r.node.metrics!;
    switch (key) {
      case 'score': return r.score;
      case 'name': return r.node.name;
      case 'cc': return m.cyclomatic ?? -1;
      case 'cognitive': return m.cognitive_complexity ?? -1;
      case 'nest': return m.max_nesting ?? -1;
      case 'loc': return m.loc;
      case 'params': return m.param_count ?? -1;
      case 'fan_in': return m.fan_in;
      case 'fan_out': return m.fan_out;
      case 'fields': return m.field_count ?? -1;
      case 'methods': return m.method_count;
      case 'pub_ratio': return m.public_field_ratio ?? -1;
      default: return 0;
    }
  }

  // Inline both predicates directly in the reactive expression so Svelte's
  // compiler tracks `kindFilter` and `severityFilter` as dependencies. Calling
  // helper functions here would hide those reads from the static analysis and
  // the list would never re-filter on dropdown change (was the original bug).
  $: filtered = $qualityRows.filter((r) => {
    if (kindFilter !== 'all') {
      const callable = r.node.kind_raw === 'Function' || r.node.kind_raw === 'Method';
      if (kindFilter === 'callable' && !callable) return false;
      if (kindFilter === 'container' && callable) return false;
    }
    if (severityFilter === 'all') return true;
    if (severityFilter === 'cycle') return !!r.node.metrics?.in_cycle;
    if (severityFilter === 'smell') return (r.node.metrics?.smells?.length ?? 0) > 0;
    const tiers: Tier[] = [
      r.tiers.cc, r.tiers.cognitive, r.tiers.nest, r.tiers.loc, r.tiers.params, r.tiers.fanOut,
      r.tiers.fieldCount, r.tiers.methodCount, r.tiers.publicFieldRatio,
    ];
    if (severityFilter === 'bad') return tiers.includes('bad');
    return tiers.includes('bad') || tiers.includes('warn');
  });

  $: sorted = [...filtered].sort((a, b) => {
    const va = rowValue(a, sortKey);
    const vb = rowValue(b, sortKey);
    const cmp = typeof va === 'string' && typeof vb === 'string'
      ? va.localeCompare(vb)
      : (va as number) - (vb as number);
    return sortDir === 'asc' ? cmp : -cmp;
  });

  $: visible = sorted.slice(0, limit);

  // Scatter data: compute once per rows change. Points are tiny (just x,y,id,kind)
  // so even 10k rows is trivial to pass to SVG.
  interface ScatterPoint { x: number; y: number; r: QualityRow; }

  $: fanScatter = $qualityRows
    .filter((r) => r.node.metrics)
    .map((r): ScatterPoint => ({ x: r.node.metrics!.fan_in, y: r.node.metrics!.fan_out, r }));

  $: ccLocScatter = $qualityRows
    .filter((r) => r.node.metrics?.cyclomatic != null)
    .map((r): ScatterPoint => ({ x: r.node.metrics!.loc, y: r.node.metrics!.cyclomatic!, r }));

  // File-level fan-in × fan-out scatter (big-picture coupling view). Uses the
  // same ScopeRow tier colouring as the table for visual consistency.
  interface ScopeScatterPoint { x: number; y: number; r: ScopeRow; }
  $: fileFanScatter = $fileRows.map((r): ScopeScatterPoint => ({
    x: r.scope.fan_in,
    y: r.scope.fan_out,
    r,
  }));
  function scopePointColor(r: ScopeRow): string {
    if (r.scope.in_cycle) return '#F44336';
    const tiers = Object.values(r.tiers);
    if (tiers.includes('bad')) return '#EF5350';
    if (tiers.includes('warn')) return '#FFA726';
    return '#66BB6A';
  }

  function selectRow(node: D3Node) {
    selectedNode.set(node);
  }

  function tierClass(t: Tier): string {
    return `tier-${t}`;
  }

  function scorePct(s: number): number {
    // For the score bar. 1.0 == fully red threshold; cap visual at 100%.
    return Math.min(s * 100, 100);
  }

  // Pull tooltip text once per metric key; the object is frozen at module scope
  // so this is essentially free.
  function explain(key: keyof typeof METRIC_EXPLANATIONS): string {
    const e = METRIC_EXPLANATIONS[key];
    return `${e.title} — ${e.body}`;
  }

  // Scatter dims

  // --- Copy single row to clipboard as a Markdown table ---
  // Using Markdown table format because it pastes nicely into chat/docs
  // while staying perfectly readable as plain text. The set of columns
  // mirrors the leaderboard exactly (minus the copy button column).
  const COPY_HEADER = [
    'Score', 'Name', 'Kind', 'Parent', 'File', 'Path',
    'CC', 'Nest', 'LOC', 'Params',
    'Fan-in', 'Fan-out', 'Fields/Variants', 'Methods', 'Pub%', 'Cycle',
  ];

  function fmtNum(v: number | null | undefined): string {
    return v == null ? '—' : String(v);
  }
  function fmtPct(v: number | null | undefined): string {
    return v == null ? '—' : `${Math.round(v * 100)}%`;
  }

  function rowValues(r: QualityRow): string[] {
    const m = r.node.metrics!;
    return [
      r.score.toFixed(2),
      r.node.name,
      r.node.kind_raw,
      parentName(r.node) || '—',
      fileBase(r.node.file_path),
      fileDir(r.node.file_path) || '—',
      fmtNum(m.cyclomatic),
      fmtNum(m.max_nesting),
      fmtNum(m.loc),
      fmtNum(m.param_count),
      fmtNum(m.fan_in),
      fmtNum(m.fan_out),
      fmtNum(m.field_count),
      fmtNum(m.method_count),
      fmtPct(m.public_field_ratio),
      m.in_cycle ? 'yes' : 'no',
    ];
  }

  // Short-lived signal used to flash "✓" on the button that was just clicked.
  let copiedId: string | null = null;
  let copiedTimer: ReturnType<typeof setTimeout> | null = null;

  async function copyRow(e: Event, r: QualityRow) {
    // Stop the click from also selecting the row behind the button.
    e.stopPropagation();
    const header = `| ${COPY_HEADER.join(' | ')} |`;
    const sep = `| ${COPY_HEADER.map(() => '---').join(' | ')} |`;
    const row = `| ${rowValues(r).join(' | ')} |`;
    await copyToClipboard(`${header}\n${sep}\n${row}\n`);
    copiedId = r.node.id;
    if (copiedTimer) clearTimeout(copiedTimer);
    copiedTimer = setTimeout(() => { copiedId = null; }, 1200);
  }

  // --- Copy a ready-to-use refactoring prompt for one row ---
  // Unlike copyRow, this needs a round-trip: the prompt is assembled by the
  // engine from the entity's metrics plus its neighbours' source, none of
  // which the leaderboard holds. `promptState` drives per-row feedback so a
  // slow request doesn't look like a dead button.
  let promptState: { id: string; state: 'loading' | 'ok' | 'fail' } | null = null;
  let promptTimer: ReturnType<typeof setTimeout> | null = null;

  async function copyPrompt(e: Event, r: QualityRow) {
    e.stopPropagation();
    if (promptState?.state === 'loading') return;

    const id = r.node.id;
    promptState = { id, state: 'loading' };
    if (promptTimer) clearTimeout(promptTimer);

    const prompt = await fetchRefactorPrompt(r.node.original_id);
    const ok = prompt != null && (await copyToClipboard(prompt));

    promptState = { id, state: ok ? 'ok' : 'fail' };
    promptTimer = setTimeout(() => { promptState = null; }, 1600);
  }

  // Shown only when the engine says it can spawn (`--allow-agent-spawn`).
  // Advertised via /api/hello rather than probed: with the flag off the route
  // is unregistered and the static-UI fallback answers with 405, not 404.
  $: canSpawn = $connection?.kind === 'ok' && $connection.agentSpawn;

  let spawnState: { id: string; state: 'loading' | 'ok' | 'fail' } | null = null;
  let spawnTimer: ReturnType<typeof setTimeout> | null = null;
  // A failed launch used to go only to console.error, so the UI's answer to
  // "why did nothing happen?" was nothing. The reason belongs on screen.
  let spawnError: string | null = null;

  async function spawnAgent(e: Event, r: QualityRow) {
    e.stopPropagation();
    if (spawnState?.state === 'loading') return;
    const id = r.node.id;
    spawnState = { id, state: 'loading' };
    if (spawnTimer) clearTimeout(spawnTimer);

    const err = await spawnAgentTerminal(r.node.original_id);
    spawnState = { id, state: err ? 'fail' : 'ok' };
    spawnError = err ? `${r.node.name}: ${err}` : null;
    spawnTimer = setTimeout(() => { spawnState = null; }, 2500);
  }

  function spawnLabel(id: string): string {
    if (spawnState?.id !== id) return 'Refactor';
    return { loading: 'Opening…', ok: 'Opened', fail: 'Failed' }[spawnState.state];
  }

  function promptLabel(id: string): string {
    if (promptState?.id !== id) return 'Prompt';
    return { loading: '…', ok: 'Copied', fail: 'Failed' }[promptState.state];
  }

  function pointColor(r: QualityRow): string {
    if (r.node.metrics?.in_cycle) return '#F44336';
    const hasBad = Object.values(r.tiers).includes('bad');
    const hasWarn = Object.values(r.tiers).includes('warn');
    if (hasBad) return '#EF5350';
    if (hasWarn) return '#FFA726';
    return '#66BB6A';
  }

  // --- Scope (file/module) table: sort + filter + copy ---
  // Files and modules use the same table shape so we share one filter
  // pipeline and toggle the data source by mode.
  function scopeValue(r: ScopeRow, key: SortKey): number | string {
    const s = r.scope;
    switch (key) {
      case 'score': return r.score;
      case 'path': return s.path;
      case 'entity_count': return s.entity_count;
      case 'callable_count': return s.callable_count;
      case 'scope_loc': return s.loc;
      case 'cohesion': return s.cohesion ?? -1;
      case 'scope_fan_in': return s.fan_in;
      case 'scope_fan_out': return s.fan_out;
      default: return 0;
    }
  }

  $: activeScopeRows = (mode as Mode) === 'files' ? $fileRows : $moduleRows;

  $: scopeFiltered = activeScopeRows.filter((r) => {
    if (severityFilter === 'all') return true;
    if (severityFilter === 'cycle') return r.scope.in_cycle;
    const tiers: Tier[] = Object.values(r.tiers);
    if (severityFilter === 'bad') return tiers.includes('bad');
    return tiers.includes('bad') || tiers.includes('warn');
  });

  $: scopeSorted = [...scopeFiltered].sort((a, b) => {
    // Scope tables default to sorting by score if the sort key is an
    // entity-only one (e.g. after switching modes).
    const key: SortKey = (['score','path','entity_count','callable_count','scope_loc','cohesion','scope_fan_in','scope_fan_out'].includes(sortKey)
      ? sortKey
      : 'score') as SortKey;
    const va = scopeValue(a, key);
    const vb = scopeValue(b, key);
    const cmp = typeof va === 'string' && typeof vb === 'string'
      ? va.localeCompare(vb)
      : (va as number) - (vb as number);
    return sortDir === 'asc' ? cmp : -cmp;
  });

  $: scopeVisible = scopeSorted.slice(0, limit);

  const SCOPE_COPY_HEADER = [
    'Score', 'Path', 'Entities', 'Callables', 'Containers', 'LOC',
    'Internal', 'External', 'Cohesion', 'Fan-in', 'Fan-out', 'Cycle',
  ];
  function scopeRowValues(r: ScopeRow): string[] {
    const s = r.scope;
    return [
      r.score.toFixed(2),
      s.path || '(root)',
      String(s.entity_count),
      String(s.callable_count),
      String(s.container_count),
      String(s.loc),
      String(s.internal_edges),
      String(s.external_edges),
      s.cohesion != null ? `${Math.round(s.cohesion * 100)}%` : '—',
      String(s.fan_in),
      String(s.fan_out),
      s.in_cycle ? 'yes' : 'no',
    ];
  }
  let scopeCopiedId: string | null = null;
  let scopeCopiedTimer: ReturnType<typeof setTimeout> | null = null;
  async function copyScopeRow(e: Event, r: ScopeRow) {
    e.stopPropagation();
    const header = `| ${SCOPE_COPY_HEADER.join(' | ')} |`;
    const sep = `| ${SCOPE_COPY_HEADER.map(() => '---').join(' | ')} |`;
    const row = `| ${scopeRowValues(r).join(' | ')} |`;
    const text = `${header}\n${sep}\n${row}\n`;
    try { await navigator.clipboard.writeText(text); } catch { /* ignore */ }
    scopeCopiedId = r.scope.path;
    if (scopeCopiedTimer) clearTimeout(scopeCopiedTimer);
    scopeCopiedTimer = setTimeout(() => { scopeCopiedId = null; }, 1200);
  }

  /**
   * Select what a file or module row names.
   *
   * The canvas draws a File or Module node for the path whenever the level is
   * collapsed that far, and that node is the row's own subject — so it is what
   * a click should land on. Falling straight to "first entity in the file" was
   * the only rule before, and it is level-dependent in a way the reader is not:
   * at Module level no node carries a plain file path, and at Entity level no
   * node carries a directory path, so a module row's click resolved to nothing
   * and did nothing at all.
   */
  function selectScope(r: ScopeRow) {
    activeScopePath = r.scope.path;
    const isModule = (mode as Mode) === 'modules';
    const nodes = $graphData.nodes;
    const target =
      nodes.find((n) => n.original_id === r.scope.path && (n.kind_raw === 'File' || n.kind_raw === 'Module'))
      ?? nodes.find((n) => scopeHolds(r.scope.path, isModule, n.file_path));
    if (target) selectedNode.set(target);
  }

  /**
   * The scope row the reader last clicked.
   *
   * Tracked rather than read back off `selectedNode`, because the two only
   * coincide when the canvas happens to be collapsed to this row's level: at
   * Entity level no node carries a directory path, so a module row's click
   * lands on an entity *inside* the module and a "is the selection this path"
   * test says no. The row would then highlight or not depending on the
   * aggregation level, which is not something the reader changed.
   */
  let activeScopePath: string | null = null;
  // A path means one thing in the Files table and another in Modules, so it
  // does not survive the switch.
  $: if (mode) activeScopePath = null;

  /**
   * Narrow the canvas to one row's files — the post-filter, reached from the
   * number that made you want it.
   *
   * Writes `hiddenFiles` (via its complement) rather than the scope rules, on
   * ADR 0010's split: this is "get everything else off my screen", which is a
   * membership test in `displayPlan` and costs nothing, not "re-analyse this",
   * which refetches and moves every side panel. It leaves the population alone
   * on purpose — see the note on the Population selector — so the reader can
   * narrow the picture without the numbers they were reading shifting under
   * them, and pick 'what the canvas is drawing' when they want both to move.
   *
   * Paths come from `rawEntityGraph`, never from `graphData`: a collapsed
   * Module node carries a *directory* as its `file_path`, so filtering off the
   * drawn graph would write directories into a store that holds files, and the
   * filter would mean something different at each aggregation level. Written
   * as real file paths it means one thing everywhere — including being the
   * documented no-op at Module level, where there is no per-file node to hide.
   */
  /**
   * Why the last "Only" click could not narrow anything, or null.
   *
   * The population can be wider than the visual scope — that is the whole
   * point of having two — so a rollup row can legitimately name files the
   * canvas has never loaded. The visual filter only hides what is loaded, so
   * on such a row it has nothing to act on. Silence there reads as a broken
   * button; the reader needs to know it is the scope, not the click.
   */
  let filterNotice: string | null = null;

  /** Keep exactly `paths`, hide every other file the visual scope holds.
   *  Returns false when the scope holds none of them. */
  function filterToFiles(paths: Set<string>): boolean {
    const all = $rawEntityGraph.nodes.map((n) => n.file_path).filter((p) => !!p);
    const keep = all.filter((f) => paths.has(f));
    if (keep.length === 0) return false;
    hiddenFiles.set(new Set(all.filter((f) => !paths.has(f))));
    return true;
  }

  function outsideScope(what: string): string {
    return `${what} is outside the visual scope, so there is nothing on the canvas to narrow. `
      + `Widen the scope tree, or set the population to the scope tree selection.`;
  }

  function filterToScope(e: Event, r: ScopeRow) {
    e.stopPropagation();
    const isModule = (mode as Mode) === 'modules';
    const files = $rawEntityGraph.nodes
      .map((n) => n.file_path)
      .filter((p) => !!p && scopeHolds(r.scope.path, isModule, p));
    filterNotice = filterToFiles(new Set(files)) ? null : outsideScope(r.scope.path || '(root)');
  }

  function filterToRow(e: Event, r: QualityRow) {
    e.stopPropagation();
    if (!r.node.file_path) return;
    filterNotice = filterToFiles(new Set([r.node.file_path])) ? null : outsideScope(r.node.file_path);
  }

  /** How many files the visual filter is currently holding back. The one
   *  signal that an "Only" click actually did something, and the way back. */
  $: hiddenCount = $hiddenFiles.size;

  /**
   * Render a metric's ok/warn/bad boundaries in its own units.
   *
   * Values come from `thresholdStore`, which resolves whatever the analyser
   * serialised alongside the graph — never a copy retyped here, which would
   * drift from the engine within a release (UI-018).
   */
  function thresholdLabel(key: string): string {
    const spec = SUMMARY_METRIC_THRESHOLDS[key];
    if (!spec) return '';
    const th = $thresholdStore as Record<string, { warn: number; bad: number } | undefined>;
    const parts = spec.keys
      .map((k) => th[k])
      .filter((v): v is { warn: number; bad: number } => !!v)
      .map((v) => `≤${v.warn} / ≤${v.bad}`);
    if (parts.length === 0) return '';
    // Two variants (callable vs container) legitimately differ; show both
    // rather than pretending the row has one boundary.
    return `${[...new Set(parts)].join('  ·  ')} ${spec.unit}`;
  }

  /** Scope the graph to the files containing entities in dependency cycles. */
  async function showCycleEntities() {
    const files = new Set(
      $qualityRows.filter((r) => r.node.metrics?.in_cycle).map((r) => r.node.file_path),
    );
    if (files.size === 0) return;
    await setScopes([...files]);
  }

  /**
   * True when the quality summary covers a different population than the
   * canvas is drawing. Quality can be pinned to the whole repo while the
   * visual scope is one folder, and the two counts then look contradictory
   * with nothing saying why (UI-018).
   */
  /**
   * Which population the metrics describe.
   *
   * The count alone ("Analysed: 5,244 entities") never said *what* was
   * counted, and the answer moves with a selector whose default is the whole
   * repo — so narrowing the scope tree leaves this number unchanged and
   * nothing explained why (UI-051).
   *
   * The selector itself had no control here at all: the store was reachable
   * only from the VS Code host's `setQualityAnalysisScope` message and from
   * the one-way "Match it" link below, so a browser reader could be told the
   * population was wrong and had exactly one thing to do about it. Naming a
   * population you cannot change is a label, not a control.
   */
  const POPULATIONS: { value: QualityAnalysisScope; label: string }[] = [
    { value: 'scope', label: 'whole analysis scope' },
    { value: 'visualScope', label: 'the scope tree selection' },
    { value: 'visualSelection', label: 'what the canvas is drawing' },
    { value: 'selection', label: 'the current selection' },
    { value: 'currentFile', label: 'the file open in the editor' },
    { value: 'changedFiles', label: 'files changed in the diff' },
  ];

  /**
   * Populations whose signal is missing right now.
   *
   * `analysisGraph` falls back to the whole scope when the signal it needs is
   * absent, which is the right runtime behaviour and the wrong thing to leave
   * unsaid in a picker: choosing "the current selection" with nothing selected
   * would name a population and then quietly show a different one.
   */
  $: unavailable = new Set<QualityAnalysisScope>([
    ...($selectedNode ? [] : ['selection' as const]),
    ...($currentEditorFile ? [] : ['currentFile' as const]),
    ...($diffData ? [] : ['changedFiles' as const]),
  ]);

  $: populationLabel =
    POPULATIONS.find((p) => p.value === $qualityAnalysisScope)?.label ?? $qualityAnalysisScope;

  /** True when the panel is measuring a population the canvas is not drawing.
   *  Suppressed before a scope exists, when everything is empty anyway. */
  $: scopeMismatch =
    $qualityAnalysisScope !== 'visualSelection' && $selectedScopes.size > 0;

  function alignScopes() {
    qualityAnalysisScope.set('visualSelection');
  }
</script>

<div class="quality-report">
  {#if spawnError}
    <!-- A launch that fails must say why. The engine's message is specific
         (no token, claude not on PATH, no terminal found); swallowing it is
         what made this look like a dead button. -->
    <div class="spawn-error" role="alert">
      <span>Could not launch an agent — {spawnError}</span>
      <button type="button" on:click={() => (spawnError = null)} aria-label="Dismiss">×</button>
    </div>
  {/if}
  {#if filterNotice}
    <div class="filter-notice" role="status" data-probe="filter-notice">
      <span>{filterNotice}</span>
      <button type="button" on:click={() => (filterNotice = null)} aria-label="Dismiss">×</button>
    </div>
  {/if}
  <!-- Repo-level health banner: always visible, aggregates all entity scores. -->
  {#if $repoQuality.entityCount > 0}
    {@const rq = $repoQuality}
    <section class="repo-banner tier-bg-{rq.tier}">
      <div class="repo-score help" data-tip="Average entity composite score across the entire scope. Green ≤0.5 (most entities healthy), amber ≤1.0 (some trouble), red >1.0 (widespread issues). Each entity's score is a weighted blend of CC, cognitive complexity, fan-out, LOC, params, and nesting." aria-label="Repo quality score explanation">
        <span class="repo-score-value tier-{rq.tier}">{rq.avgScore.toFixed(2)}</span>
        <span class="repo-score-label">repo score</span>
        <!-- A bare decimal says nothing: no scale, no units, no direction.
             (UI-018) -->
        <span class="repo-score-scale" data-probe="score-scale">{SCORE_SCALE_LABEL}</span>
      </div>
      <div class="repo-stats">
        <span class="repo-stat"><strong>{rq.entityCount}</strong> entities</span>
        <span class="repo-stat t-ok">{rq.okCount} ok</span>
        <span class="repo-stat t-warn">{rq.warnCount} warn</span>
        <span class="repo-stat t-bad">{rq.badCount} bad</span>
        {#if rq.cycleCount > 0}
          <span class="repo-stat t-bad">{rq.cycleCount} in cycle</span>
        {/if}
      </div>
      <div class="repo-bar">
        <div class="repo-bar-ok" style="width: {rq.entityCount ? (rq.okCount / rq.entityCount * 100) : 0}%"></div>
        <div class="repo-bar-warn" style="width: {rq.entityCount ? (rq.warnCount / rq.entityCount * 100) : 0}%"></div>
        <div class="repo-bar-bad" style="width: {rq.entityCount ? (rq.badCount / rq.entityCount * 100) : 0}%"></div>
      </div>
    </section>
  {/if}

  <!-- Above the mode bar, not inside a tab: it decides what every tab below
       is measuring, so a reader on Files or Modules needs it as much as one
       on Entities — and while it lived in the Entities branch they could not
       reach it at all without switching tabs first. -->
  <label class="population-picker">
    <span>Population</span>
    <select
      data-probe="quality-population-picker"
      value={$qualityAnalysisScope}
      on:change={(e) => qualityAnalysisScope.set(e.currentTarget.value as QualityAnalysisScope)}
    >
      {#each POPULATIONS as p}
        <option value={p.value} disabled={unavailable.has(p.value)}>
          {p.label}{unavailable.has(p.value) ? ' (unavailable)' : ''}
        </option>
      {/each}
    </select>
  </label>

  <!-- Mode selector: Entities / Files / Modules.
       Each mode reuses the same severity filter + sort + copy pipeline. -->
  <div class="mode-bar">
    <button type="button" class="mode-btn" class:active={mode === 'entities'} on:click={() => (mode = 'entities')}>Entities</button>
    <button type="button" class="mode-btn" class:active={mode === 'files'} on:click={() => (mode = 'files')}>Files</button>
    <button type="button" class="mode-btn" class:active={mode === 'modules'} on:click={() => (mode = 'modules')}>Modules</button>
  </div>

  {#if mode === 'entities'}
  <!-- Summary -->
  {#if $qualitySummary.total === 0}
    <div class="empty">Select a scope to see quality metrics.</div>
  {:else}
    <!-- Score, summary and charts share a capped scroll region so the
         ranked table below always gets height. Before this the table was
         last in normal flow behind ~1500px of content, so it showed 9 of
         109 rows in a 588px pane (UI-011). -->
    <div class="qr-head">
    <section class="summary">
      <!-- Names the population, so a repo-wide figure sitting next to a
           scoped canvas count stops reading as a contradiction (UI-010).
           The align action sits inline rather than in its own banner: the
           default analysis scope is repo-wide, so a mismatch with the visual
           scope is the normal state, not an error, and a bordered warning
           that is on almost always reads as noise (UI-018). -->
      <h3 data-probe="quality-population" title="Full-detail entities behind the quality metrics — includes parameters, branches and fields, which the canvas never draws.">
        Analysed: {$qualitySummary.total.toLocaleString()} entities
        <span class="population-source" data-probe="quality-population-source">· {populationLabel}</span>
        {#if scopeMismatch}
          <span class="scope-mismatch" data-probe="scope-mismatch">
            — not the canvas selection.
            <button type="button" class="align-btn" on:click={alignScopes}>Match it</button>
          </span>
        {/if}
      </h3>
      <table class="summary-table">
        <thead>
          <tr><th></th><th class="t-ok">OK</th><th class="t-warn">Warn</th><th class="t-bad">Bad</th></tr>
        </thead>
        <tbody>
          <tr><th class="help" data-tip={explain('cc')} aria-label={explain('cc')}>CC<span class="th-limits" data-probe="metric-threshold">{thresholdLabel('cc')}</span></th><td>{$qualitySummary.cc.ok}</td><td>{$qualitySummary.cc.warn}</td><td>{$qualitySummary.cc.bad}</td></tr>
          <tr><th class="help" data-tip={explain('cognitive')} aria-label={explain('cognitive')}>Cog<span class="th-limits" data-probe="metric-threshold">{thresholdLabel('cognitive')}</span></th><td>{$qualitySummary.cognitive.ok}</td><td>{$qualitySummary.cognitive.warn}</td><td>{$qualitySummary.cognitive.bad}</td></tr>
          <tr><th class="help" data-tip={explain('nest')} aria-label={explain('nest')}>Nest<span class="th-limits" data-probe="metric-threshold">{thresholdLabel('nest')}</span></th><td>{$qualitySummary.nest.ok}</td><td>{$qualitySummary.nest.warn}</td><td>{$qualitySummary.nest.bad}</td></tr>
          <tr><th class="help" data-tip={explain('loc')} aria-label={explain('loc')}>LOC<span class="th-limits" data-probe="metric-threshold">{thresholdLabel('loc')}</span></th><td>{$qualitySummary.loc.ok}</td><td>{$qualitySummary.loc.warn}</td><td>{$qualitySummary.loc.bad}</td></tr>
          <tr><th class="help" data-tip={explain('params')} aria-label={explain('params')}>Params<span class="th-limits" data-probe="metric-threshold">{thresholdLabel('params')}</span></th><td>{$qualitySummary.params.ok}</td><td>{$qualitySummary.params.warn}</td><td>{$qualitySummary.params.bad}</td></tr>
          <tr><th class="help" data-tip={explain('fan_out')} aria-label={explain('fan_out')}>Fan-out<span class="th-limits" data-probe="metric-threshold">{thresholdLabel('fanOut')}</span></th><td>{$qualitySummary.fanOut.ok}</td><td>{$qualitySummary.fanOut.warn}</td><td>{$qualitySummary.fanOut.bad}</td></tr>
          <tr><th class="help" data-tip={explain('field_count')} aria-label={explain('field_count')}>Fields/Var<span class="th-limits" data-probe="metric-threshold">{thresholdLabel('fieldCount')}</span></th><td>{$qualitySummary.fieldCount.ok}</td><td>{$qualitySummary.fieldCount.warn}</td><td>{$qualitySummary.fieldCount.bad}</td></tr>
          <tr><th class="help" data-tip={explain('method_count')} aria-label={explain('method_count')}>Methods<span class="th-limits" data-probe="metric-threshold">{thresholdLabel('methodCount')}</span></th><td>{$qualitySummary.methodCount.ok}</td><td>{$qualitySummary.methodCount.warn}</td><td>{$qualitySummary.methodCount.bad}</td></tr>
          <tr><th class="help" data-tip={explain('public_field_ratio')} aria-label={explain('public_field_ratio')}>Pub&nbsp;%<span class="th-limits" data-probe="metric-threshold">{thresholdLabel('publicFieldRatio')}</span></th><td>{$qualitySummary.publicFieldRatio.ok}</td><td>{$qualitySummary.publicFieldRatio.warn}</td><td>{$qualitySummary.publicFieldRatio.bad}</td></tr>
        </tbody>
      </table>
      <p class="threshold-source">
        Thresholds come from the analyser and are served with the graph, so
        they cannot drift from the engine. CI enforces a separate, laxer hard
        ceiling — CC ≤ 15, Cognitive ≤ 22, Nesting ≤ 4 — which sits between
        these warn and bad bands on purpose: warn is advice, the ceiling is a
        gate.
      </p>

      {#if $qualitySummary.inCycle > 0}
        <!-- The most actionable line in the panel used to be inert text: it
             said 18 entities were in cycles and gave no way to see which
             (UI-018). -->
        <button
          type="button"
          class="cycle-banner help"
          data-probe="cycle-action"
          data-tip={explain('cycle')}
          aria-label="Show the entities in dependency cycles"
          on:click={showCycleEntities}
        >
          <strong>{$qualitySummary.inCycle}</strong> entities participate in dependency cycles.
          <span class="cycle-cta">Show them →</span>
        </button>
      {/if}
    </section>

    <!-- Scatters -->
    <section class="scatters">
      <Scatter
        points={fanScatter.map((p) => ({ x: p.x, y: p.y, datum: p.r }))}
        xTitle="fan-in"
        yTitle="fan-out"
        colorOf={(d) => pointColor(d as QualityRow)}
        inCycleOf={(d) => !!(d as QualityRow).node.metrics?.in_cycle}
        labelOf={(d, x, y) => `${(d as QualityRow).node.name} — fan-in=${x}, fan-out=${y}`}
        onSelect={(d) => selectRow((d as QualityRow).node)}
      />
      <Scatter
        points={ccLocScatter.map((p) => ({ x: p.x, y: p.y, datum: p.r }))}
        xTitle="loc"
        yTitle="cc"
        colorOf={(d) => pointColor(d as QualityRow)}
        inCycleOf={(d) => !!(d as QualityRow).node.metrics?.in_cycle}
        labelOf={(d, x, y) => `${(d as QualityRow).node.name} — loc=${x}, cc=${y}`}
        onSelect={(d) => selectRow((d as QualityRow).node)}
      />
    </section>

    <!-- Filters -->
    <section class="filters">
      <label>
        Kind
        <select bind:value={kindFilter}>
          <option value="all">all</option>
          <option value="callable">functions / methods</option>
          <option value="container">structs / modules</option>
        </select>
      </label>
      <label>
        Severity
        <select bind:value={severityFilter}>
          <option value="all">all</option>
          <option value="warnbad">amber + red</option>
          <option value="bad">red only</option>
          <option value="cycle">in cycle</option>
          <option value="smell">has smell</option>
        </select>
      </label>
      <label>
        Limit
        <select bind:value={limit}>
          <option value={50}>50</option>
          <option value={100}>100</option>
          <option value={250}>250</option>
          <option value={1000}>1000</option>
        </select>
      </label>
      {#if hiddenCount > 0}
        <!-- A row's "Only" click leaves nothing on screen saying it happened
             beyond a smaller canvas, which is indistinguishable from a scope
             change. State it, and put the way back next to it. -->
        <span class="filter-state" data-probe="visual-filter-state">
          Canvas filtered · {hiddenCount} {hiddenCount === 1 ? 'file' : 'files'} hidden
          <button type="button" class="align-btn" on:click={clearHiddenFiles}>Show all</button>
        </span>
      {/if}
    </section>

    </div>

    <!-- Leaderboard table -->
    <section class="table-section" data-probe="quality-table">
      <h3>Problem entities ({filtered.length} shown, top {visible.length} rendered)</h3>
      <table class="metrics-table">
        <thead>
          <tr>
            <th class="sortable help" data-tip={explain('score')} aria-label={explain('score')} on:click={() => toggleSort('score')}>Score {sortKey === 'score' ? (sortDir === 'desc' ? '▼' : '▲') : ''}</th>
            <th class="sortable" on:click={() => toggleSort('name')}>Name {sortKey === 'name' ? (sortDir === 'desc' ? '▼' : '▲') : ''}</th>
            <th>Kind</th>
            <th>Parent</th>
            <th>File</th>
            <th>Path</th>
            <th class="sortable num help" data-tip={explain('cc')} aria-label={explain('cc')} on:click={() => toggleSort('cc')}>CC</th>
            <th class="sortable num help" data-tip={explain('cognitive')} aria-label={explain('cognitive')} on:click={() => toggleSort('cognitive')}>Cog</th>
            <th class="sortable num help" data-tip={explain('nest')} aria-label={explain('nest')} on:click={() => toggleSort('nest')}>Nest</th>
            <th class="sortable num help" data-tip={explain('loc')} aria-label={explain('loc')} on:click={() => toggleSort('loc')}>LOC</th>
            <th class="sortable num help" data-tip={explain('params')} aria-label={explain('params')} on:click={() => toggleSort('params')}>P</th>
            <th class="sortable num help" data-tip={explain('fan_in')} aria-label={explain('fan_in')} on:click={() => toggleSort('fan_in')}>Fin</th>
            <th class="sortable num help" data-tip={explain('fan_out')} aria-label={explain('fan_out')} on:click={() => toggleSort('fan_out')}>Fout</th>
            <th class="sortable num help" data-tip={explain('field_count')} aria-label={explain('field_count')} on:click={() => toggleSort('fields')}>F/V</th>
            <th class="sortable num help" data-tip={explain('method_count')} aria-label={explain('method_count')} on:click={() => toggleSort('methods')}>M</th>
            <th class="sortable num help" data-tip={explain('public_field_ratio')} aria-label={explain('public_field_ratio')} on:click={() => toggleSort('pub_ratio')}>Pub%</th>
            <th class="help" data-tip={explain('cycle')} aria-label={explain('cycle')}>Cyc</th>
            <th class="help" data-tip={explain('smells')} aria-label={explain('smells')}>Smells</th>
            <th aria-label="Copy row"></th>
          </tr>
        </thead>
        <tbody>
          {#each visible as r (r.node.id)}
            {@const m = r.node.metrics}
            <tr class:selected={$selectedNode?.id === r.node.id} on:click={() => selectRow(r.node)}>
              <td class="score-cell">
                <div class="score-bar" style="width: {scorePct(r.score)}%"></div>
                <span class="score-val">{r.score.toFixed(2)}</span>
              </td>
              <td class="name-cell" title={r.node.file_path}>{r.node.name}</td>
              <td class="kind-cell">{r.node.kind_raw}</td>
              <td class="parent-cell" title={parentName(r.node)}>{parentName(r.node) || '—'}</td>
              <td class="file-cell" title={r.node.file_path}>{fileBase(r.node.file_path)}</td>
              <td class="path-cell" title={r.node.file_path}>{fileDir(r.node.file_path) || '—'}</td>
              <td class="num {tierClass(r.tiers.cc)}">{m?.cyclomatic ?? '—'}</td>
              <td class="num {tierClass(r.tiers.cognitive)}">{m?.cognitive_complexity ?? '—'}</td>
              <td class="num {tierClass(r.tiers.nest)}">{m?.max_nesting ?? '—'}</td>
              <td class="num {tierClass(r.tiers.loc)}">{m?.loc}</td>
              <td class="num {tierClass(r.tiers.params)}">{m?.param_count ?? '—'}</td>
              <td class="num">{m?.fan_in}</td>
              <td class="num {tierClass(r.tiers.fanOut)}">{m?.fan_out}</td>
              <td class="num {tierClass(r.tiers.fieldCount)}">{m?.field_count ?? '—'}</td>
              <td class="num {tierClass(r.tiers.methodCount)}">{m?.method_count ?? '—'}</td>
              <td class="num {tierClass(r.tiers.publicFieldRatio)}">{m?.public_field_ratio != null ? `${Math.round(m.public_field_ratio * 100)}%` : '—'}</td>
              <td class="num">{m?.in_cycle ? '●' : ''}</td>
              <td class="smell-cell">
                {#if m?.smells?.length}
                  {#each m.smells as s}
                    {@const meta = SMELL_META[s]}
                    <span class="smell-badge" title={meta?.hint ?? s}>{meta?.label ?? s}</span>
                  {/each}
                {/if}
              </td>
              <td class="copy-cell">
                <button
                  type="button"
                  class="copy-btn"
                  title="Draw only this entity's file on the canvas — a visual filter, so the metrics above stay on the population you picked"
                  aria-label="Filter the canvas to this entity's file"
                  on:click={(e) => filterToRow(e, r)}
                >Only</button>
                {#if canSpawn}
                  <button
                    type="button"
                    class="copy-btn spawn-btn"
                    class:copied={spawnState?.id === r.node.id && spawnState.state === 'ok'}
                    class:failed={spawnState?.id === r.node.id && spawnState.state === 'fail'}
                    title="Launch a Claude Code agent against this entity — opens a terminal on the engine's machine"
                    aria-label="Refactor with Claude Code in a terminal"
                    on:click={(e) => spawnAgent(e, r)}
                  >{spawnLabel(r.node.id)}</button>
                {/if}
                <button
                  type="button"
                  class="copy-btn"
                  class:copied={promptState?.id === r.node.id && promptState.state === 'ok'}
                  class:failed={promptState?.id === r.node.id && promptState.state === 'fail'}
                  title="Copy a ready-to-use refactoring prompt for this entity to the clipboard"
                  aria-label="Copy refactoring prompt"
                  on:click={(e) => copyPrompt(e, r)}
                >{promptLabel(r.node.id)}</button>
                <button
                  type="button"
                  class="copy-btn"
                  class:copied={copiedId === r.node.id}
                  title="Copy this row\u2019s metrics as a Markdown table"
                  aria-label="Copy row as Markdown table"
                  on:click={(e) => copyRow(e, r)}
                >{copiedId === r.node.id ? 'Copied' : 'Copy'}</button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </section>
  {/if}
  {:else}
    <!-- ---- FILES / MODULES ---- -->
    {#if activeScopeRows.length === 0}
      <div class="empty">No {mode} in the current scope.</div>
    {:else}
      <div class="qr-head">
      <section class="summary">
        <h3>{mode === 'files' ? 'Files' : 'Modules'} ({activeScopeRows.length})</h3>
      </section>

      {#if mode === 'files'}
        <section class="scatters">
          <Scatter
            points={fileFanScatter.map((p) => ({ x: p.x, y: p.y, datum: p.r }))}
            xTitle="fan-in"
            yTitle="fan-out"
            colorOf={(d) => scopePointColor(d as ScopeRow)}
            inCycleOf={(d) => !!(d as ScopeRow).scope.in_cycle}
            labelOf={(d, x, y) => `${(d as ScopeRow).scope.path} — fan-in=${x}, fan-out=${y}`}
            onSelect={(d) => selectScope(d as ScopeRow)}
            emptyMessage="No files in the current scope."
          />
        </section>
      {/if}

      <section class="filters">
        <label>
          Severity
          <select bind:value={severityFilter}>
            <option value="all">all</option>
            <option value="warnbad">amber + red</option>
            <option value="bad">red only</option>
            <option value="cycle">in cycle</option>
          </select>
        </label>
        <label>
          Limit
          <select bind:value={limit}>
            <option value={50}>50</option>
            <option value={100}>100</option>
            <option value={250}>250</option>
            <option value={1000}>1000</option>
          </select>
        </label>
        {#if hiddenCount > 0}
          <span class="filter-state" data-probe="visual-filter-state">
            Canvas filtered · {hiddenCount} {hiddenCount === 1 ? 'file' : 'files'} hidden
            <button type="button" class="align-btn" on:click={clearHiddenFiles}>Show all</button>
          </span>
        {/if}
      </section>

      </div>

      <section class="table-section" data-probe="quality-table">
        <h3>
          Problem {mode === 'files' ? 'files' : 'modules'}
          ({scopeFiltered.length} shown, top {scopeVisible.length} rendered)
        </h3>
        <table class="metrics-table">
          <thead>
            <tr>
              <th class="sortable help" data-tip={explain('score')} aria-label={explain('score')} on:click={() => toggleSort('score')}>Score {sortKey === 'score' ? (sortDir === 'desc' ? '▼' : '▲') : ''}</th>
              <th class="help" data-tip={explain('aggregated_quality')} aria-label={explain('aggregated_quality')}>Qual</th>
              <th class="sortable" on:click={() => toggleSort('path')}>Path {sortKey === 'path' ? (sortDir === 'desc' ? '▼' : '▲') : ''}</th>
              <th class="sortable num help" data-tip={explain('entity_count')} aria-label={explain('entity_count')} on:click={() => toggleSort('entity_count')}>Ent</th>
              <th class="sortable num" on:click={() => toggleSort('callable_count')}>Call</th>
              <th class="sortable num help" data-tip={explain('scope_loc')} aria-label={explain('scope_loc')} on:click={() => toggleSort('scope_loc')}>LOC</th>
              <th class="sortable num help" data-tip={explain('cohesion')} aria-label={explain('cohesion')} on:click={() => toggleSort('cohesion')}>Coh</th>
              <th class="sortable num help" data-tip={explain('scope_fan_in')} aria-label={explain('scope_fan_in')} on:click={() => toggleSort('scope_fan_in')}>Fin</th>
              <th class="sortable num help" data-tip={explain('scope_fan_out')} aria-label={explain('scope_fan_out')} on:click={() => toggleSort('scope_fan_out')}>Fout</th>
              <th class="help" data-tip={explain('scope_cycle')} aria-label={explain('scope_cycle')}>Cyc</th>
              <th aria-label="Copy row"></th>
            </tr>
          </thead>
          <tbody>
            {#each scopeVisible as r (r.scope.path)}
              {@const agg = r.aggregate}
              <tr class:selected={activeScopePath === r.scope.path} on:click={() => selectScope(r)}>
                <td class="score-cell">
                  <div class="score-bar" style="width: {scorePct(r.score)}%"></div>
                  <span class="score-val">{r.score.toFixed(2)}</span>
                </td>
                <td class="num agg-cell" title={agg.entityCount > 0 ? `avg ${agg.avgScore.toFixed(2)} across ${agg.entityCount} entities in this population · ${agg.okCount} ok / ${agg.warnCount} warn / ${agg.badCount} bad` : 'No scored entities in this population'}>
                  {#if agg.entityCount > 0}
                    <span class="tier-{agg.tier}">{agg.avgScore.toFixed(2)}</span>
                    <span class="agg-dist">
                      <span class="t-ok">{agg.okCount}</span>/<span class="t-warn">{agg.warnCount}</span>/<span class="t-bad">{agg.badCount}</span>
                    </span>
                  {:else}
                    —
                  {/if}
                </td>
                <td class="path-cell" title={r.scope.path}>{r.scope.path || '(root)'}</td>
                <td class="num {tierClass(r.tiers.entity)}">{r.scope.entity_count}</td>
                <td class="num">{r.scope.callable_count}</td>
                <td class="num {tierClass(r.tiers.loc)}">{r.scope.loc}</td>
                <td class="num {tierClass(r.tiers.cohesion)}">{r.scope.cohesion != null ? `${Math.round(r.scope.cohesion * 100)}%` : '—'}</td>
                <td class="num">{r.scope.fan_in}</td>
                <td class="num {tierClass(r.tiers.fanOut)}">{r.scope.fan_out}</td>
                <td class="num">{r.scope.in_cycle ? '●' : ''}</td>
                <td class="copy-cell">
                  <button
                    type="button"
                    class="copy-btn"
                    title="Draw only this {mode === 'files' ? 'file' : 'folder'} on the canvas — a visual filter, so the metrics stay on the population you picked"
                    aria-label="Filter the canvas to this scope"
                    on:click={(e) => filterToScope(e, r)}
                  >Only</button>
                  <button
                    type="button"
                    class="copy-btn"
                    class:copied={scopeCopiedId === r.scope.path}
                    title="Copy this row as a Markdown table"
                    aria-label="Copy row as Markdown table"
                    on:click={(e) => copyScopeRow(e, r)}
                  >{scopeCopiedId === r.scope.path ? '✓' : '⧉'}</button>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </section>
    {/if}
  {/if}
</div>

<style>
  .quality-report {
    font-size: 0.85rem;
    color: var(--text-secondary);
    /* Fill the pane and split it: capped head, table takes the rest. */
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }

  .repo-score-scale {
    display: block;
    font-size: 0.62rem;
    color: var(--text-dim);
    letter-spacing: 0.02em;
  }

  .threshold-source {
    margin: 6px 0 0;
    font-size: 0.68rem;
    line-height: 1.45;
    color: var(--text-dim);
  }

  /* Under the metric name rather than in a fifth column: five columns did
     not fit a 360px sidebar and the boundaries were being clipped mid-value
     ("≤30 / ≤60 · ≤100 / ≤20"). The name cell is the widest and has vertical
     room to spare. */
  .th-limits {
    display: block;
    font-weight: 400;
    font-size: 0.62rem;
    line-height: 1.3;
    color: var(--text-dim);
    white-space: nowrap;
  }

  .cycle-cta { color: var(--accent); font-weight: 600; white-space: nowrap; }

  .population-source {
    font-weight: normal;
    color: var(--text-dim);
  }

  .scope-mismatch {
    font-size: 0.72rem;
    font-weight: 400;
    color: var(--text-dim);
    white-space: nowrap;
  }

  /* Above the tabs, not in the filter row: the filters narrow what one table
     lists, this decides what every number in the panel is measured over, and
     stacking the two reads as one group of equals. */
  .population-picker {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 0 0 10px;
    font-size: 0.7rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-dim);
  }
  .population-picker select {
    flex: 1 1 auto;
    min-width: 0;
    background: var(--bg-deep);
    color: var(--text-secondary);
    border: 1px solid var(--border);
    border-radius: 3px;
    padding: 2px 4px;
    font-size: 0.78rem;
    font-family: inherit;
    text-transform: none;
    letter-spacing: normal;
  }

  .filter-state {
    display: flex;
    align-items: center;
    gap: 6px;
    align-self: flex-end;
    padding-bottom: 2px;
    font-size: 0.7rem;
    color: var(--text-dim);
    white-space: nowrap;
  }

  .align-btn {
    padding: 2px 8px;
    border-radius: 10px;
    border: 1px solid var(--accent);
    background: transparent;
    color: var(--accent);
    font: inherit;
    font-size: 0.7rem;
    cursor: pointer;
  }

  .qr-head {
    flex: 0 1 auto;
    min-height: 0;
    /* 45% left the ranked table 248px — 9 rows at 27px, one short of
       useful. 34% gives it ~340px / 12 rows in a 910px pane. */
    max-height: 34%;
    overflow-y: auto;
  }

  .table-section {
    flex: 1 1 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }

  /* The table itself scrolls; its heading stays put. */
  .table-section :global(table) {
    display: block;
    overflow-y: auto;
    flex: 1 1 0;
    min-height: 0;
  }

  /* Repo-level health banner */
  .repo-banner {
    padding: 10px 12px;
    border-radius: 6px;
    margin-bottom: 10px;
    border: 1px solid var(--border);
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 12px;
  }
  .tier-bg-ok { background: rgba(76, 175, 80, 0.08); border-color: rgba(76, 175, 80, 0.3); }
  .tier-bg-warn { background: rgba(255, 152, 0, 0.08); border-color: rgba(255, 152, 0, 0.3); }
  .tier-bg-bad { background: rgba(244, 67, 54, 0.08); border-color: rgba(244, 67, 54, 0.3); }
  .tier-bg-na { background: var(--bg-deep); }
  .repo-score {
    display: flex;
    flex-direction: column;
    align-items: center;
    min-width: 64px;
  }
  .repo-score-value {
    font-size: 1.5rem;
    font-weight: 700;
    font-family: 'Monaco', 'Menlo', monospace;
    line-height: 1;
  }
  .repo-score-label {
    font-size: 0.65rem;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-dim);
    margin-top: 2px;
  }
  .repo-stats {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    font-size: 0.78rem;
  }
  .repo-stat { color: var(--text-secondary); }
  .repo-bar {
    flex: 1 1 100%;
    display: flex;
    height: 4px;
    border-radius: 2px;
    overflow: hidden;
    background: var(--bg-deep);
  }
  .repo-bar-ok { background: #66BB6A; }
  .repo-bar-warn { background: #FFA726; }
  .repo-bar-bad { background: #EF5350; }

  /* Aggregated quality cell in scope tables */
  .agg-cell {
    white-space: nowrap;
  }
  .agg-dist {
    font-size: 0.65rem;
    margin-left: 3px;
    opacity: 0.7;
  }

  /* Smell badges */
  .smell-cell {
    white-space: nowrap;
  }
  .smell-badge {
    display: inline-block;
    padding: 1px 5px;
    margin: 1px 2px;
    border-radius: 3px;
    font-size: 0.65rem;
    font-family: -apple-system, BlinkMacSystemFont, sans-serif;
    background: rgba(244, 67, 54, 0.12);
    color: var(--tier-bad-fg);
    border: 1px solid rgba(244, 67, 54, 0.3);
    cursor: help;
  }

  .empty {
    color: var(--text-disabled);
    font-style: italic;
    padding: 8px 0;
  }

  section {
    margin-bottom: 14px;
  }

  h3 {
    font-size: 0.8rem;
    text-transform: uppercase;
    color: var(--text-dim);
    margin: 4px 0 6px;
    letter-spacing: 0.04em;
  }

  /* Summary */
  .summary-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.8rem;
  }
  .summary-table th, .summary-table td {
    padding: 3px 6px;
    text-align: right;
  }
  .summary-table thead th {
    color: var(--text-dim);
    font-weight: 500;
    font-size: 0.7rem;
    text-transform: uppercase;
    border-bottom: 1px solid var(--border);
  }
  .summary-table tbody th {
    text-align: left;
    color: var(--text-secondary);
    font-weight: 500;
  }
  .t-ok { color: var(--tier-ok-fg); }
  .t-warn { color: var(--tier-warn-fg); }
  .t-bad { color: var(--tier-bad-fg); }

  .cycle-banner {
    margin-top: 6px;
    padding: 6px 8px;
    background: rgba(244, 67, 54, 0.12);
    border: 1px solid rgba(244, 67, 54, 0.4);
    border-radius: 4px;
    font-size: 0.75rem;
    color: var(--tier-bad-fg);
  }

  /* Scatters */
  .scatter {
    width: 100%;
    height: auto;
    background: var(--bg-deep);
    border: 1px solid var(--border);
    border-radius: 4px;
    margin-bottom: 8px;
  }
  .axis { stroke: var(--border-subtle); stroke-width: 1; }
  .axis-label { fill: var(--text-disabled); font-size: 8px; font-family: monospace; }
  .dot { cursor: pointer; }
  .dot:hover { r: 4; stroke: var(--text); stroke-width: 1; }

  /* Filters */
  .filters {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  .filters label {
    display: flex;
    flex-direction: column;
    font-size: 0.7rem;
    color: var(--text-dim);
    text-transform: uppercase;
  }
  .filters select {
    background: var(--bg-deep);
    color: var(--text-secondary);
    border: 1px solid var(--border);
    border-radius: 3px;
    padding: 2px 4px;
    font-size: 0.8rem;
    margin-top: 2px;
  }

  /* Table */
  .metrics-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.75rem;
    font-family: 'Monaco', 'Menlo', monospace;
  }
  .metrics-table thead th {
    position: sticky;
    top: 0;
    background: var(--bg-body);
    color: var(--text-dim);
    font-weight: 500;
    font-size: 0.68rem;
    text-transform: uppercase;
    text-align: left;
    padding: 4px 4px;
    border-bottom: 1px solid var(--border);
    z-index: 1;
  }
  .metrics-table th.num, .metrics-table td.num { text-align: right; }
  .metrics-table th.sortable { cursor: pointer; user-select: none; }
  .metrics-table th.sortable:hover { color: var(--text); }

  .metrics-table tbody tr {
    cursor: pointer;
    border-bottom: 1px solid color-mix(in srgb, var(--border) 30%, transparent);
  }
  .metrics-table tbody tr:hover { background: color-mix(in srgb, var(--bg-hover) 40%, transparent); }
  .metrics-table tbody tr.selected { background: color-mix(in srgb, var(--accent) 12%, transparent); }

  .metrics-table td {
    padding: 4px 4px;
    vertical-align: middle;
  }

  .name-cell {
    color: var(--text-secondary);
    max-width: 160px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .kind-cell {
    color: var(--text-muted);
    font-size: 0.7rem;
  }
  .parent-cell {
    color: var(--text-secondary);
    font-size: 0.72rem;
    max-width: 110px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .file-cell {
    color: var(--text-muted);
    font-size: 0.7rem;
    max-width: 120px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .path-cell {
    color: var(--text-disabled);
    font-size: 0.68rem;
    max-width: 140px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    direction: rtl;
    text-align: left;
  }
  .score-cell {
    position: relative;
    min-width: 60px;
  }
  .score-bar {
    position: absolute;
    left: 0;
    top: 0;
    bottom: 0;
    background: linear-gradient(90deg, rgba(76, 175, 80, 0.2), rgba(244, 67, 54, 0.35));
    pointer-events: none;
    z-index: 0;
  }
  .score-val {
    position: relative;
    z-index: 1;
    font-weight: 700;
  }

  .tier-ok { color: var(--tier-ok-fg); }
  .tier-warn { color: var(--tier-warn-fg); }
  .tier-bad { color: var(--tier-bad-fg); font-weight: 700; }
  .tier-na { color: var(--text-disabled); }

  .mode-bar {
    display: flex;
    gap: 4px;
    margin-bottom: 10px;
    border-bottom: 1px solid var(--border);
  }
  .mode-btn {
    background: transparent;
    color: var(--text-dim);
    border: none;
    border-bottom: 2px solid transparent;
    padding: 6px 12px;
    font-size: 0.78rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    cursor: pointer;
    font-family: inherit;
  }
  .mode-btn:hover { color: var(--text-secondary); }
  .mode-btn.active {
    color: var(--accent);
    border-bottom-color: var(--accent);
  }

  .spawn-error {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    margin: 6px 10px;
    padding: 6px 10px;
    border: 1px solid rgba(244, 67, 54, 0.5);
    border-radius: 4px;
    background: rgba(244, 67, 54, 0.1);
    color: #EF9A9A;
    font-size: 0.75rem;
    line-height: 1.4;
  }
  /* Not an error — the click was reasonable and the scope was too narrow for
     it, which is information rather than a fault. Amber, not red. */
  .filter-notice {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    margin: 6px 10px;
    padding: 6px 10px;
    border: 1px solid rgba(255, 152, 0, 0.45);
    border-radius: 4px;
    background: rgba(255, 152, 0, 0.1);
    color: var(--tier-warn-fg);
    font-size: 0.75rem;
    line-height: 1.4;
  }
  .filter-notice span { flex: 1; word-break: break-word; }
  .filter-notice button {
    background: transparent; border: none; color: inherit;
    cursor: pointer; font-size: 1rem; line-height: 1; padding: 0 2px;
  }

  .spawn-error span { flex: 1; word-break: break-word; }
  .spawn-error button {
    background: transparent; border: none; color: inherit;
    cursor: pointer; font-size: 1rem; line-height: 1; padding: 0 2px;
  }

  .copy-cell { width: 1%; white-space: nowrap; text-align: right; padding: 0 6px; }
  .copy-btn {
    background: transparent;
    color: var(--text-dim);
    border: 1px solid var(--border-subtle);
    border-radius: 3px;
    padding: 1px 6px;
    margin-left: 3px;
    font-size: 0.72rem;
    line-height: 1.5;
    cursor: pointer;
    font-family: inherit;
    transition: color 0.1s, border-color 0.1s, background 0.1s;
  }
  .copy-btn:hover {
    color: var(--text-secondary);
    border-color: var(--border-subtle);
    background: color-mix(in srgb, var(--bg-hover) 50%, transparent);
  }
  .copy-btn.copied {
    color: #A5D6A7;
    border-color: rgba(76, 175, 80, 0.5);
    background: rgba(76, 175, 80, 0.1);
  }
  /* A prompt copy can fail where a row copy can't — it needs the engine.
     Say so on the button rather than failing silently. */
  /* The one action that starts work on the host rather than copying text —
     it spends money and edits files, so it should not look like its
     neighbours. */
  .spawn-btn {
    color: var(--accent);
    border-color: color-mix(in srgb, var(--accent) 45%, transparent);
    font-weight: 600;
  }
  .spawn-btn:hover { background: color-mix(in srgb, var(--accent) 15%, transparent); }
  .copy-btn.failed {
    color: #EF9A9A;
    border-color: rgba(244, 67, 54, 0.5);
    background: rgba(244, 67, 54, 0.1);
  }

  .help {
    position: relative;
    cursor: help;
    border-bottom: 1px dotted var(--text-disabled);
  }
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
    letter-spacing: normal;
    text-transform: none;
    white-space: normal;
    text-align: left;
    pointer-events: none;
    opacity: 0;
    transform: translateY(-2px);
    transition: opacity 0.12s ease, transform 0.12s ease;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.4);
  }
  .help:hover::after,
  .help:focus::after {
    opacity: 1;
    transform: translateY(0);
  }
  .metrics-table th.help:nth-last-child(-n+3)::after {
    left: auto;
    right: 0;
  }
</style>
