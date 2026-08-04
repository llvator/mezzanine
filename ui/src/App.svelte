<script lang="ts">
  import './app.css';
  import Sidebar from './components/Sidebar.svelte';
  import BuildStamp from './components/BuildStamp.svelte';
  import GraphView from './components/GraphView.svelte';
  import {
    graphData, selectedNode, viewMode, graphLevel,
    showLabels, showKindLabels, showLinkLabels,
    treeDensity, hoverDepth, hoverLocked,
  } from './stores/graph';
  import type { GraphLevel } from './types/graph';
  import type { TreeDensity } from './stores/graph';

  import { setTreeDepth, levelOverrides, showGhostNodes, showBuiltinGhosts, showTemplateVars } from './stores/graph';
  import { activeTheme, applyTheme, autoFitView } from './stores/settings';
  import CanvasToolbar from './components/CanvasToolbar.svelte';
  import { publishGraph } from './viewmodels/filterViewModel';
  import { displayPlan } from './viewmodels/displayPlan';
  import {
    connectLiveReload, liveConnected, liveReloading, liveStatus,
    liveIsBroken, reconnectLiveReload, stopLiveReload,
  } from './stores/liveReload';
  import { loadDiff, diffActive, diffData, diffChangesOnly, diffCoreOnly, diffDimOpacity } from './stores/diff';
  import { refreshData, refreshing } from './stores/scope';
  import CommitPicker from './components/CommitPicker.svelte';

  function toggleLiveMode() {
    if ($liveConnected) {
      stopLiveReload();
    } else {
      // Always a fresh start: the button is also how you recover from a
      // stream that gave up, and resuming a spent backoff would do nothing.
      reconnectLiveReload();
    }
  }

  /** What the mode indicator says when the stream isn't running.
   *  A refused origin is the one failure a user can't diagnose from the
   *  browser, and the remedy is a flag on a command they already ran — so
   *  the label names it. */
  const LIVE_STOP_LABEL: Record<string, string> = {
    refused: 'Not live — engine refused this page',
    token: 'Not live — pairing token needed',
    unreachable: 'Not live — no engine answering',
    'no-stream': 'Not live — no event stream',
  };

  const LIVE_STOP_HINT: Record<string, string> = {
    refused: 'Restart the engine with --allow-origin ' + window.location.origin,
    token: "Paste the pairing token from the engine's startup banner",
    unreachable: 'Nothing answered on this endpoint. Click to try again.',
    'no-stream': 'The API answers but /events does not. Click to try again.',
  };

  async function manualRefresh() {
    await refreshData();
  }
  import {
    loadIndex, indexData, indexLoadError, selectedScopes, selectionStats,
    graphLoading, graphLoadError, scopeOversized, ENTITY_THRESHOLD,
  } from './stores/scope';
  import { loadGraphData } from './transform';
  import RepoPicker from './components/RepoPicker.svelte';
  import {
    serveMode, activeRepo, enterServeMode, selectRepo, slugFromHash, backToPicker,
  } from './stores/serveMode';
  import ConnectScreen from './components/ConnectScreen.svelte';
  import { checkCurrentEndpoint, connection, type Connection } from './stores/connection';
  import { endpoint, forgetEndpoint } from './endpoint';

  let graphView: GraphView;

  /** True once boot has decided between picker and graph, so we don't
   *  flash the graph shell during the `/api/repos` probe. */
  let booted = false;

  // Panel state. Three columns flank the canvas: Filters/Quality on the
  // left, Details and Description on the right. Details moved out of the
  // sidebar's bottom half, where it was taking 35% of the height the scope
  // tree and the quality table needed (UI-011).
  let leftCollapsed = false;
  let leftWidth = 360;

  /** Window width, so the layout can decide whether a column still fits. */
  let winWidth = typeof window !== 'undefined' ? window.innerWidth : 1600;

  // A window can't always hold three columns, and squeezing the canvas past
  // the point where the graph is readable defeats the purpose of having any
  // of them. So the right-hand side gets a budget — whatever is left after
  // the sidebar, the three 20px toggle strips and a canvas floor — and the
  // columns claim it in priority order.
  //
  // Details goes first: it's the one you keep an eye on while working, where
  // Description is read in bursts. It also shrinks to its minimum before it
  // gives up, so the narrow case is a narrower pane rather than no pane.
  // All three want ~1620px; below that Description drops out, and below
  // ~1220px so does Details. Collapsing the sidebar buys back its width.
  $: leftUsed = leftCollapsed ? 0 : leftWidth;
  $: rightRoom = winWidth - leftUsed - 60 - MIN_CANVAS_WIDTH;

  $: detailsShownWidth = Math.min($detailsWidth, rightRoom);
  $: showDetails = $detailsPaneOpen && detailsShownWidth >= DETAILS_MIN_WIDTH;
  $: showDescription = $describePaneOpen
    && rightRoom - (showDetails ? detailsShownWidth : 0) >= DESCRIPTION_WIDTH;

  /** Was a pane hidden by the window rather than by the user? The toggle
   *  says so, so a button that does nothing visible still explains itself. */
  $: detailsSquashed = $detailsPaneOpen && !showDetails;
  $: descriptionSquashed = $describePaneOpen && !showDescription;

  /** Details outranks Description in the budget, so asking for Description
   *  when Details has eaten the room has to close Details — otherwise the
   *  click produces nothing on screen. The reverse needs no help: opening
   *  Details already takes its share first. */
  function toggleDescription() {
    const next = !$describePaneOpen;
    describePaneOpen.set(next);
    if (next && rightRoom - (showDetails ? detailsShownWidth : 0) < DESCRIPTION_WIDTH) {
      detailsPaneOpen.set(false);
    }
  }

  // Load data at startup: prefer embedded __GRAPH_DATA__ (HTML mode), otherwise
  // load the lightweight index so the user can pick a scope.
  // In HTML mode, seed filters before graphData.set so GraphView's initial
  // applyFilters sees the correct values (same reason as in scope.ts).
  // For scope-driven mode, applySelection() handles the filter seeding itself.
  import { onMount, onDestroy } from 'svelte';
  import {
    isVscode, onFocusFile, onFocusCursor, onSetScopes, onDrillIn, onCommand,
    reportSelection, reportQuality, reportFilters, reportLevelFilters, reportDiff, reportScopes, reportAnalysisScopes,
    reportDescription,
  } from './vscodeAdapter';
  import { description, describeOnHover } from './stores/description';
  import {
    detailsPaneOpen, describePaneOpen, detailsWidth,
    DETAILS_MIN_WIDTH, DETAILS_MAX_WIDTH, DESCRIPTION_WIDTH, MIN_CANVAS_WIDTH,
  } from './stores/panes';
  import DescriptionPanel from './components/DescriptionPanel.svelte';
  import DetailsPanel from './components/DetailsPanel.svelte';
  import { focusScope, setScopes, drillIn, analysisScopes, setAnalysisScopes, ensureFullData, autoLevel } from './stores/scope';
  import { qualityRows, repoQuality, qualityAnalysisScope, qualitySortBy, currentEditorFile, tierFromScore } from './stores/quality';
  import type { QualityAnalysisScope, QualitySortKey } from './stores/quality';
  import {
    diffComputing, diffApiError, baseDetailsCache, triggerDiff, diffFiltersEnabled,
  } from './stores/diff';
  import { derived as svelteDerived, get } from 'svelte/store';
  import {
    generalEntityTypes, generalRelTypes, generalOutgoing, generalIncoming, generalLanguages,
    allEntityTypes, allRelTypes, allLanguages,
    toggleEntityType, toggleRelType, toggleLanguage,
    toggleLevelEnabled, toggleLevelPeerEdges,
    cycleEntityTypeTriState, cycleRelTypeTriState, cycleDirectionTriState,
    showDirectEdges, showCrossLevelEdges,
  } from './viewmodels/filterViewModel';

  /**
   * The largest top-level folder that will actually render, for the
   * empty-state card.
   *
   * The card used to say "check the box next to one or more folders/files in
   * the Scope panel" while the panel showed 3 of 7 folders with the rest
   * below the fold — instructions pointing at something not on screen
   * (UI-017). Naming a concrete folder, and offering to select it, gives the
   * user a first graph to learn from.
   *
   * Deliberately skips anything at or above ENTITY_THRESHOLD: those render as
   * the oversized-scope overlay, which would be a worse first experience than
   * the card it replaced.
   */
  $: suggestedScope = (() => {
    const idx = $indexData;
    if (!idx) return null;
    const roots = Object.values(idx.nodes).filter(
      (n) => n.type === 'folder' && n.path !== '' && !n.path.includes('/'),
    );
    const renderable = roots.filter((n) => n.entity_count < ENTITY_THRESHOLD);
    if (renderable.length === 0) return null;
    return renderable.reduce((a, b) => (b.entity_count > a.entity_count ? b : a));
  })();

  async function startHere() {
    if (!suggestedScope) return;
    await setScopes([suggestedScope.path]);
  }

  /** Bring the scope tree into view and flash it, so the card's instruction
   *  points at something the user can actually see. */
  function revealScopeTree() {
    const el = document.querySelector('[data-probe="scope-tree"]') as HTMLElement | null;
    if (!el) return;
    el.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
    el.classList.add('scope-tree-flash');
    setTimeout(() => el.classList.remove('scope-tree-flash'), 1600);
  }

  /** Find the best node for a cursor position: the innermost entity whose
   *  span covers the given line, scoped to the matching file. */
  function findNodeAtLine(
    nodes: readonly import('./types/graph').D3Node[],
    relativePath: string,
    line: number,
  ): import('./types/graph').D3Node | undefined {
    const pathMatchedNodes = nodes.filter((n) => {
      const fp = n.file_path;
      return (
        fp === relativePath ||
        fp.endsWith('/' + relativePath) ||
        relativePath.endsWith('/' + fp)
      );
    });
    // Synthetic entities (class fields, function parameters) inherit the
    // full span of their parent because the Rust-side Parameter model
    // doesn't carry a per-item span. Letting them compete as candidates
    // produces tie-breaking surprises: cursor on `class X:` would match
    // both X and every one of X's fields, and the smallest-span tie-break
    // can then pick a field — whose tree-view neighbourhood (one incoming
    // hop to the class) hides the class's actual methods. Exclude them
    // from cursor-follow so the container wins; they remain click-
    // selectable on the canvas.
    const realCandidates = pathMatchedNodes.filter(
      (n) =>
        n.kind_raw !== 'Parameter' &&
        n.kind_raw !== 'Branch' &&
        n.kind_raw !== 'Loop' &&
        !n.tags?.includes('class_field'),
    );
    const candidates = realCandidates.filter(
      (n) => n.line <= line && line <= n.end_line,
    );
    console.log(
      `[cursor-sync/svelte] findNodeAtLine relativePath=${relativePath} line=${line} totalNodes=${nodes.length} pathMatched=${pathMatchedNodes.length} real=${realCandidates.length} spanMatched=${candidates.length}`,
    );
    if (pathMatchedNodes.length === 0 && nodes.length > 0) {
      const samplePaths = Array.from(new Set(nodes.slice(0, 50).map((n) => n.file_path)));
      console.log(
        `[cursor-sync/svelte]   no path match — sample node.file_path values:`,
        samplePaths.slice(0, 8),
      );
    } else if (pathMatchedNodes.length > 0 && candidates.length === 0) {
      const spans = pathMatchedNodes.map((n) => `${n.name}[${n.line}..${n.end_line}]`);
      console.log(
        `[cursor-sync/svelte]   path matched ${pathMatchedNodes.length} node(s) but line ${line} is outside all spans:`,
        spans.slice(0, 10),
      );
    }
    if (candidates.length === 0) return undefined;
    // Pick the smallest span (innermost/most specific entity)
    const chosen = candidates.reduce((a, b) =>
      b.end_line - b.line < a.end_line - a.line ? b : a,
    );
    console.log(
      `[cursor-sync/svelte]   chose node ${chosen.name} (id=${chosen.id}, span=${chosen.line}..${chosen.end_line})`,
    );
    return chosen;
  }

  // Apply theme on startup and whenever the user changes it.
  activeTheme.subscribe((id) => applyTheme(id));

  let unsubFocusFile: (() => void) | undefined;
  let unsubFocusCursor: (() => void) | undefined;
  let unsubSetScopes: (() => void) | undefined;
  let unsubDrillIn: (() => void) | undefined;
  let unsubQuality: (() => void) | undefined;
  let unsubFilters: (() => void) | undefined;
  let unsubLevelFilters: (() => void) | undefined;
  let unsubDiff: (() => void) | undefined;
  let unsubScopes: (() => void) | undefined;
  let unsubAnalysisScopes: (() => void) | undefined;
  let unsubCommand: (() => void) | undefined;
  let unsubSelection: (() => void) | undefined;
  let unsubDescription: (() => void) | undefined;
  /** Coalesces the hover firehose into one post per settled pointer
   *  position — a fast drag across the canvas crosses dozens of nodes and
   *  each one would otherwise be a webview→host round trip. */
  let descriptionTimer: ReturnType<typeof setTimeout> | undefined;

  onMount(async () => {
    // Attach VS Code listeners FIRST so any messages the extension sends
    // (setCompact, focusFile, setScopes) while we're loading the index are
    // queued and processed correctly. The extension buffers messages until
    // we post 'ready' below.
    if (isVscode()) {
      // Focus-file: scope to a single file.
      //   • If the cursor is inside an entity → tree view rooted on that entity
      //   • Else (cursor at file scope: imports, between entities, module-level) →
      //     graph view of the whole file, no specific selection
      unsubFocusFile = onFocusFile(async ({ relativePath, line }) => {
        currentEditorFile.set(relativePath);
        const ok = await focusScope(relativePath);
        if (!ok) return;
        const match =
          line !== undefined ? findNodeAtLine($graphData.nodes, relativePath, line) : undefined;
        if (match) {
          viewMode.set('tree');
          selectedNode.set(match);
        } else {
          viewMode.set('graph');
          selectedNode.set(null);
        }
      });

      // Focus-cursor: user moved the cursor. Try to select the matching
      // entity in the current view; if the view is collapsed (file/module
      // level) and doesn't contain the cursor's entity, re-scope to the
      // cursor's file so `applySelection` re-picks the level — the new
      // (single-file) scope almost always fits under RENDER_BUDGET and
      // comes back at entity level, at which point we can select.
      unsubFocusCursor = onFocusCursor(async ({ relativePath, line }) => {
        console.log(
          `[cursor-sync/svelte] onFocusCursor fired relativePath=${relativePath} line=${line} currentSelected=${$selectedNode?.id ?? '(none)'} viewMode=${$viewMode} graphDataNodes=${$graphData.nodes.length}`,
        );
        currentEditorFile.set(relativePath);
        let match = findNodeAtLine($graphData.nodes, relativePath, line);
        if (!match) {
          // Check whether the current view is collapsed (no entity node
          // anywhere in this file). If yes, auto-drill so the user sees
          // entities without having to click "Drill in" manually.
          const hasEntitiesForFile = $graphData.nodes.some(
            (n) => n.kind_raw !== 'File' && n.kind_raw !== 'Module' && (
              n.file_path === relativePath ||
              n.file_path.endsWith('/' + relativePath) ||
              relativePath.endsWith('/' + n.file_path)
            ),
          );
          if (!hasEntitiesForFile) {
            console.log(
              `[cursor-sync/svelte]   → view is collapsed for this file; auto-drilling scope to ${relativePath}`,
            );
            const ok = await focusScope(relativePath);
            if (ok) match = findNodeAtLine($graphData.nodes, relativePath, line);
          }
        }
        if (match) {
          if (match.id !== $selectedNode?.id) {
            console.log(
              `[cursor-sync/svelte]   → switching to tree + selecting ${match.name} (id=${match.id})`,
            );
            viewMode.set('tree');
            selectedNode.set(match);
          } else {
            console.log(`[cursor-sync/svelte]   → same node already selected, no-op`);
          }
        } else if ($selectedNode) {
          // Cursor left the last entity — drop back to the whole-file graph.
          console.log(
            `[cursor-sync/svelte]   → no match; clearing selection and dropping to graph view`,
          );
          viewMode.set('graph');
          selectedNode.set(null);
        } else {
          console.log(`[cursor-sync/svelte]   → no match and no prior selection, no-op`);
        }
      });

      // Scope changes from the native TreeView: replace the selected scopes
      // and re-render the graph.
      unsubSetScopes = onSetScopes((paths) => {
        void setScopes(paths);
      });

      // Drill-in from the native Selection panel: same as double-clicking
      // a collapsed node on the canvas — narrow AND reset auto-level so
      // the view expands to whichever level fits the new (smaller) scope.
      unsubDrillIn = onDrillIn((path) => {
        void drillIn(path);
      });

      // Generic commands from the native "View Options" panel.
      unsubCommand = onCommand((command, value) => {
        switch (command) {
          case 'setViewMode':
            viewMode.set(value as 'graph' | 'tree');
            break;
          case 'setGraphLevel':
            // Manual level pick — pin it so applySelection stops auto-escalating.
            autoLevel.set(false);
            graphLevel.set(value as GraphLevel);
            break;
          case 'setTreeDepth':
            setTreeDepth(value as number);
            break;
          case 'setTreeDensity':
            treeDensity.set(value as TreeDensity);
            break;
          case 'setHoverDepth':
            hoverDepth.set(value as number);
            break;
          case 'setDescribeOnHover':
            describeOnHover.set(!!value);
            break;
          case 'setShowLabels':
            showLabels.set(!!value);
            break;
          case 'setShowKindLabels':
            showKindLabels.set(!!value);
            break;
          case 'setShowLinkLabels':
            showLinkLabels.set(!!value);
            break;
          case 'setAutoFit':
            autoFitView.set(!!value);
            break;
          case 'setShowGhosts':
            showGhostNodes.set(!!value);
            break;
          case 'setShowBuiltinGhosts':
            showBuiltinGhosts.set(!!value);
            break;
          case 'setShowTemplateVars':
            showTemplateVars.set(!!value);
            break;
          case 'clearSelection':
            selectedNode.set(null);
            break;
          case 'zoomIn':
            graphView?.zoomIn();
            break;
          case 'zoomOut':
            graphView?.zoomOut();
            break;
          case 'resetZoom':
            graphView?.resetZoom();
            break;
          case 'fitView':
            graphView?.fitView();
            break;
          case 'fitWidth':
            graphView?.fitWidth();
            break;
          case 'toggleViewMode':
            graphView?.toggleViewMode();
            break;
          case 'selectEntityById': {
            const id = value as string;
            const match = $graphData.nodes.find((n) => n.id === id);
            if (match) selectedNode.set(match);
            break;
          }
          case 'toggleEntityType': {
            const v = value as { name: string; enabled: boolean };
            toggleEntityType(v.name, v.enabled);
            break;
          }
          case 'toggleRelType': {
            const v = value as { name: string; enabled: boolean };
            toggleRelType(v.name, v.enabled);
            break;
          }
          case 'toggleLanguage': {
            const v = value as { name: string; enabled: boolean };
            toggleLanguage(v.name, v.enabled);
            break;
          }
          case 'setOutgoing':
            generalOutgoing.set(!!value);
            break;
          case 'setIncoming':
            generalIncoming.set(!!value);
            break;
          case 'selectAllEntityTypes':
            generalEntityTypes.set(new Set(value as string[]));
            break;
          case 'clearAllEntityTypes':
            generalEntityTypes.set(new Set());
            break;
          case 'selectAllRelTypes':
            generalRelTypes.set(new Set(value as string[]));
            break;
          case 'clearAllRelTypes':
            generalRelTypes.set(new Set());
            break;
          case 'toggleLevelEnabled':
            toggleLevelEnabled(value as number);
            break;
          case 'toggleLevelPeerEdges':
            toggleLevelPeerEdges(value as number);
            break;
          case 'cycleLevelEntityType': {
            const v = value as { level: number; key: string };
            cycleEntityTypeTriState(v.level, v.key);
            break;
          }
          case 'cycleLevelRelType': {
            const v = value as { level: number; key: string };
            cycleRelTypeTriState(v.level, v.key);
            break;
          }
          case 'cycleLevelDirection': {
            const v = value as { level: number; dir: 'outgoing' | 'incoming' };
            cycleDirectionTriState(v.level, v.dir);
            break;
          }
          case 'setShowDirectEdges':
            showDirectEdges.set(!!value);
            break;
          case 'setShowCrossLevelEdges':
            showCrossLevelEdges.set(!!value);
            break;
          case 'triggerDiff': {
            const v = value as { fromRef: string; toRef: string };
            void triggerDiff(v.fromRef, v.toRef);
            break;
          }
          case 'clearDiff':
            diffActive.set(false);
            diffData.set(null);
            baseDetailsCache.set(null);
            diffApiError.set(null);
            break;
          case 'scopeToChangedFiles': {
            // Replace the current scope with files that contain entities
            // the user actually changed (source-level edits + adds + removes).
            //
            // IMPORTANT: we do NOT include files whose only changes are
            // "impact" (fan-in/fan-out shifts caused by ripples from other
            // files). A small edit in one file can mark dozens of unrelated
            // files as having impact modifications, blowing the scope up to
            // almost the whole repo.
            //
            // Folder paths and ghosts are also dropped — `minimizeSelection`
            // treats any folder as covering every file under it, which would
            // silently widen the scope to the whole repo.
            const data = $diffData;
            if (!data) break;
            const idx = $indexData;
            const coreChanges = data.entities.filter((e) =>
              e.status === 'added' || e.status === 'removed'
              || (e.status === 'modified' && e.source_changed === true)
            );
            const rawPaths = Array.from(new Set(
              coreChanges
                .map((e) => e.file_path)
                .filter((p) => typeof p === 'string' && p.length > 0),
            ));
            const leafFiles = idx
              ? rawPaths.filter((p) => idx.nodes[p]?.type === 'file')
              : rawPaths;
            const droppedNonLeaf = rawPaths.length - leafFiles.length;
            const droppedImpactOnly = data.entities.length - coreChanges.length;
            console.log(
              `[nao] scopeToChangedFiles: ${coreChanges.length} core-changed entities across ${leafFiles.length} files`,
              leafFiles,
            );
            if (droppedNonLeaf > 0) {
              console.log(`[nao] scopeToChangedFiles: dropped ${droppedNonLeaf} non-file paths (folders / ghosts):`,
                rawPaths.filter((p) => !leafFiles.includes(p)));
            }
            if (droppedImpactOnly > 0) {
              console.log(`[nao] scopeToChangedFiles: ignored ${droppedImpactOnly} impact-only / unchanged entities (not scoped)`);
            }
            diffChangesOnly.set(true);
            diffFiltersEnabled.set(true);
            void setScopes(leafFiles, { force: true });
            break;
          }
          case 'setDiffChangesOnly':
            diffChangesOnly.set(!!value);
            break;
          case 'setDiffCoreOnly':
            diffCoreOnly.set(!!value);
            break;
          case 'setDiffDimOpacity':
            diffDimOpacity.set(Number(value));
            break;
          case 'setDiffFiltersEnabled':
            diffFiltersEnabled.set(!!value);
            break;
          case 'setQualityAnalysisScope':
            console.log('[nao] setQualityAnalysisScope →', value);
            qualityAnalysisScope.set(value as QualityAnalysisScope);
            break;
          case 'setQualitySortBy':
            qualitySortBy.set(value as QualitySortKey);
            break;
          case 'setAnalysisScopes':
            console.log('[nao] setAnalysisScopes →', value);
            setAnalysisScopes(value as string[]);
            break;
          case 'setCurrentFile':
            console.log('[nao] setCurrentFile →', value);
            currentEditorFile.set(value as string);
            break;
        }
      });

      // Broadcast node selection to the extension host so the native
      // Selection side view stays in sync with the graph.
      unsubSelection = selectedNode.subscribe((node) => {
        if (!node) {
          reportSelection(null);
          return;
        }
        reportSelection({
          entityId: node.id,
          originalId: node.original_id,
          name: node.name,
          qualifiedName: node.qualified_name,
          kind: node.kind,
          filePath: node.file_path,
          line: node.line,
          endLine: node.end_line,
          language: node.language,
          sourceCode: node.source_code,
          parameters: node.parameters,
          returnType: node.return_type,
          metrics: node.metrics as unknown as Record<string, unknown> | undefined,
        });
      });

      // Broadcast the hovered/selected node's description chain to the
      // native Description side view.
      unsubDescription = description.subscribe((state) => {
        clearTimeout(descriptionTimer);
        descriptionTimer = setTimeout(() => {
          reportDescription(state ? { source: state.source, chain: state.chain } : null);
        }, 90);
      });

      // Broadcast the current filter state to the native Filters view
      // whenever any of the filter stores change. svelteDerived coalesces
      // updates so we only fire once per "settled" state, not N times.
      const filterState = svelteDerived(
        [generalEntityTypes, generalRelTypes, generalOutgoing, generalIncoming, generalLanguages,
         allEntityTypes, allRelTypes, allLanguages, showGhostNodes, showBuiltinGhosts],
        ([$eTypes, $rTypes, $out, $in, $langs, $allE, $allR, $allL, $ghosts, $builtinGhosts]) => ({
          entityTypes: ($allE as string[]).map((name) => ({ name, enabled: ($eTypes as Set<string>).has(name) })),
          relTypes: ($allR as string[]).map((name) => ({ name, enabled: ($rTypes as Set<string>).has(name) })),
          directions: { outgoing: $out as boolean, incoming: $in as boolean },
          languages: ($allL as string[]).map((name) => ({ name, enabled: ($langs as Set<string>).has(name) })),
          showGhosts: $ghosts as boolean,
          showBuiltinGhosts: $builtinGhosts as boolean,
        }),
      );
      unsubFilters = filterState.subscribe((state) => reportFilters(state));

      // Broadcast per-level filter state (depth-1/2/3 tri-state overrides,
      // enabled/peerEdges, plus the Edge Display toggles).
      const levelState = svelteDerived(
        [levelOverrides, allEntityTypes, allRelTypes, showDirectEdges, showCrossLevelEdges],
        ([$lo, $allE, $allR, $direct, $cross]) => ({
          allEntityTypes: $allE,
          allRelTypes: $allR,
          levels: {
            1: { ...$lo[1] },
            2: { ...$lo[2] },
            3: { ...$lo[3] },
          },
          showDirectEdges: $direct,
          showCrossLevelEdges: $cross,
        }),
      );
      unsubLevelFilters = levelState.subscribe((state) => reportLevelFilters(state));

      // Broadcast diff state — active flag, refs, summary counts, filter
      // toggles, compute/error status. The native Diff view mirrors this.
      const diffState = svelteDerived(
        [diffActive, diffData, diffChangesOnly, diffCoreOnly, diffDimOpacity,
         diffComputing, diffApiError, selectedScopes, diffFiltersEnabled, selectedNode],
        ([$act, $data, $chg, $core, $dim, $comp, $err, $sel, $filtEn, $selNode]) => {
          // Match the filter used by `scopeToChangedFiles` — files with
          // real (core) changes only, not impact-only ripples.
          const changedFiles = $data
            ? new Set(
                $data.entities
                  .filter((e) =>
                    e.status === 'added' || e.status === 'removed'
                    || (e.status === 'modified' && e.source_changed === true)
                  )
                  .map((e) => e.file_path)
                  .filter((p) => typeof p === 'string' && p.length > 0),
              )
            : new Set<string>();
          return {
            active: $act,
            fromRef: $data?.from_ref,
            toRef: $data?.to_ref,
            summary: $data
              ? {
                  added: $data.summary.added,
                  removed: $data.summary.removed,
                  modified: $data.summary.modified,
                  modifiedSource: $data.summary.modified_source,
                  modifiedImpact: $data.summary.modified_impact,
                  unchanged: $data.summary.unchanged,
                }
              : undefined,
            changesOnly: $chg,
            coreOnly: $core,
            dimOpacity: $dim,
            computing: $comp,
            error: $err,
            hasScope: $sel.size > 0,
            changedFileCount: changedFiles.size,
            filtersEnabled: $filtEn,
            hasSelection: !!$selNode,
          };
        },
      );
      unsubDiff = diffState.subscribe((state) => reportDiff(state));

      // Broadcast the selected scope paths so the native Scopes tree can
      // sync its checkbox state. Needed whenever scope changes from outside
      // the tree (e.g. "Scope to changes" in the Diff view).
      unsubScopes = selectedScopes.subscribe((set) => reportScopes([...set]));

      // Same pattern for the separate analysis scope that drives Quality.
      unsubAnalysisScopes = analysisScopes.subscribe((set) => reportAnalysisScopes([...set]));

      // Broadcast scope-level quality (aggregate summary + top refactor
      // candidates) whenever the scoped graph changes. Combining the two in
      // one derived store guarantees the summary and rows arrive together.
      const qualityPayload = svelteDerived(
        [qualityRows, repoQuality, qualityAnalysisScope, qualitySortBy, currentEditorFile, diffData, selectedScopes, displayPlan],
        ([$rows, $summary, $analysisScope, $sortBy, $curFile, $diff, $selectedScopes, $plan]) => {
          // Descending sort by the chosen metric. Missing values sink to
          // the bottom so they don't crowd the top of the list.
          const MINUS_INFINITY = -1;
          const metricOf = (r: any, key: QualitySortKey): number => {
            if (key === 'score') return r.score ?? MINUS_INFINITY;
            const m = r.node?.metrics;
            if (!m) return MINUS_INFINITY;
            switch (key) {
              case 'pagerank':    return m.pagerank ?? MINUS_INFINITY;
              case 'cc':          return m.cyclomatic ?? MINUS_INFINITY;
              case 'cognitive':   return m.cognitive_complexity ?? MINUS_INFINITY;
              case 'nesting':     return m.max_nesting ?? MINUS_INFINITY;
              case 'loc':         return m.loc ?? MINUS_INFINITY;
              case 'params':      return m.param_count ?? MINUS_INFINITY;
              case 'fanIn':       return m.fan_in ?? MINUS_INFINITY;
              case 'fanOut':      return m.fan_out ?? MINUS_INFINITY;
              case 'wmc':         return m.wmc ?? MINUS_INFINITY;
              case 'chainDepth':  return m.chain_depth ?? MINUS_INFINITY;
              case 'methodCount': return m.method_count ?? MINUS_INFINITY;
              case 'fieldCount':  return m.field_count ?? MINUS_INFINITY;
              default:            return MINUS_INFINITY;
            }
          };
          const top = $rows
            .slice()
            .sort((a, b) => metricOf(b, $sortBy) - metricOf(a, $sortBy))
            .slice(0, 100)
            .map((r) => {
              const m = r.node.metrics!;
              return {
                id: r.node.id,
                name: r.node.name,
                kind: r.node.kind,
                file: r.node.file_path,
                line: r.node.line,
                score: r.score,
                tier: tierFromScore(r.score),
                isGhost: r.node.tags?.includes('ghost') ?? false,
                metrics: {
                  loc: m.loc,
                  cc: m.cyclomatic,
                  cognitive: m.cognitive_complexity,
                  nesting: m.max_nesting,
                  params: m.param_count,
                  fanIn: m.fan_in,
                  fanOut: m.fan_out,
                  fieldCount: m.field_count,
                  methodCount: m.method_count,
                  wmc: m.wmc,
                  chainDepth: m.chain_depth,
                  pagerank: m.pagerank,
                  inCycle: m.in_cycle,
                  smells: m.smells,
                },
                tiers: r.tiers,
              };
            });
          // Report which analysis-scope options the Quality view should
          // offer (so it can disable the ones whose prerequisites aren't met).
          const availableScopes: QualityAnalysisScope[] = ['scope'];
          if ($selectedScopes.size > 0) availableScopes.push('visualScope');
          if (($plan?.visibleNodeIds?.size ?? 0) > 0) availableScopes.push('visualSelection');
          if ($curFile) availableScopes.push('currentFile');
          if ($diff) availableScopes.push('changedFiles');
          return {
            summary: $summary as unknown as Record<string, unknown>,
            rows: top,
            analysisScope: $analysisScope,
            availableScopes,
            currentFile: $curFile,
            sortBy: $sortBy,
          };
        },
      );
      unsubQuality = qualityPayload.subscribe((payload: any) => {
        console.log('[nao] qualityPayload → broadcast: analysisScope=', payload.analysisScope, 'rows=', payload.rows.length);
        reportQuality(payload);
      });

      // Listeners attached — tell the extension to flush queued messages.
      (window as any).__NAO_VSCODE__?.postMessage({ type: 'ready' });
    }

    // Which engine, and does it answer (UI-034)? Skipped in the webview,
    // where the extension has already supplied the endpoint and a picker
    // would be a regression. Same-origin is probed like any other: a page
    // the engine served passes in one request, and a page on a static host
    // with no engine behind it is exactly the visitor the screen is for.
    if (!isVscode()) {
      const verdict = await checkCurrentEndpoint();
      if (verdict.kind !== 'ok') {
        booted = true;
        return;
      }

      // Serve mode (UI-007): the backend hosts several repos, so one has to
      // be chosen before any data endpoint is meaningful. Which mode it is
      // came back with the handshake above rather than from a second probe.
      if (verdict.mode === 'serve') {
        await enterServeMode();
        const slug = slugFromHash();
        // A deep link to a slug the server doesn't have falls back to the
        // picker rather than firing a page of 404s against it.
        if (!slug || !selectRepo(slug)) {
          booted = true;
          return;
        }
      }
    }

    // Reveal the shell before loading data — `bootData` awaits the index,
    // and blocking the whole UI on it would regress watch mode, which
    // renders its "Loading…" overlay immediately today.
    booted = true;
    await bootData();
  });

  // Browser back/forward across `#/repo/{slug}`. `backToPicker` already
  // reloads; this catches the history-navigation case, where the hash
  // changes under us and the loaded data no longer matches the URL.
  function onHashChange() {
    if (!get(serveMode)) return;
    if (slugFromHash() !== (get(activeRepo)?.slug ?? null)) window.location.reload();
  }

  /** Load the active repo's data. In serve mode this runs after the user
   *  picks a repo; otherwise it's the ordinary watch/static boot. */
  async function bootData() {
    if ((window as any).__GRAPH_DATA__) {
      publishGraph(loadGraphData((window as any).__GRAPH_DATA__));
    } else {
      await loadIndex();
      // Eagerly populate the full-graph store so Quality's analysis scope
      // (which defaults to the whole repo) works before any visual scope
      // is picked. Silent best-effort; errors surface via graphLoadError.
      void ensureFullData().catch(() => {});
    }
    // Try to connect to the watch server's SSE endpoint for live reload.
    // No-ops in serve mode, which has no `/events`.
    connectLiveReload();
    // Try to load a diff overlay (from `nao diff`). Silent no-op if
    // diff.json doesn't exist, and skipped entirely in serve mode.
    loadDiff();
  }

  /**
   * A new endpoint was accepted. Reload rather than resuming in place.
   *
   * Same reason `backToPicker` reloads: every store derived from the old
   * engine — the index, the details cache, the memoized full-graph promise,
   * the filter sets seeded from the previous graph's kinds — would otherwise
   * need individual teardown, and one missed reset shows up as another
   * repo's data bleeding into the view.
   */
  function onEndpointChosen() {
    window.location.reload();
  }

  /** Drop the stored endpoint and go back to the connect screen. */
  function disconnectEndpoint() {
    forgetEndpoint();
    window.location.reload();
  }

  /** Picker selection: adopt the repo, then run the normal data boot. */
  async function onPickRepo(slug: string) {
    if (!selectRepo(slug)) return;
    await bootData();
  }

  onDestroy(() => {
    unsubFocusFile?.();
    unsubFocusCursor?.();
    unsubSetScopes?.();
    unsubDrillIn?.();
    unsubCommand?.();
    unsubSelection?.();
    unsubDescription?.();
    clearTimeout(descriptionTimer);
    unsubQuality?.();
    unsubFilters?.();
    unsubLevelFilters?.();
    unsubDiff?.();
    unsubScopes?.();
    unsubAnalysisScopes?.();
  });

  // Panel resize
  function startResize(e: MouseEvent) {
    e.preventDefault();
    const startX = e.clientX;
    const startWidth = leftWidth;
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';

    const onMove = (e: MouseEvent) => {
      leftWidth = Math.max(200, startWidth + (e.clientX - startX));
    };

    const onUp = () => {
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      document.removeEventListener('mousemove', onMove);
      document.removeEventListener('mouseup', onUp);
    };

    document.addEventListener('mousemove', onMove);
    document.addEventListener('mouseup', onUp);
  }

  /** The Details column grows leftwards, so its handle is on its left edge
   *  and a drag towards the canvas widens it. */
  function startDetailsResize(e: MouseEvent) {
    e.preventDefault();
    const startX = e.clientX;
    const startWidth = $detailsWidth;
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';

    const onMove = (e: MouseEvent) => {
      const next = startWidth - (e.clientX - startX);
      // Stops at the canvas floor as well as the pane's own maximum, so the
      // handle can't be dragged somewhere the pane won't actually render.
      const ceiling = Math.min(DETAILS_MAX_WIDTH, rightRoom);
      detailsWidth.set(Math.min(ceiling, Math.max(DETAILS_MIN_WIDTH, next)));
    };

    const onUp = () => {
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      document.removeEventListener('mousemove', onMove);
      document.removeEventListener('mouseup', onUp);
    };

    document.addEventListener('mousemove', onMove);
    document.addEventListener('mouseup', onUp);
  }

  /** `L` freezes the hover preview so the cursor can leave the graph without
   *  the Details and Description panes resetting. Bound at the window rather
   *  than in the panel, so closing the panel doesn't take the key with it. */
  function onKeydown(e: KeyboardEvent) {
    if (e.key !== 'l' && e.key !== 'L') return;
    if (e.target instanceof HTMLElement
        && (e.target.tagName === 'INPUT' || e.target.tagName === 'TEXTAREA')) return;
    hoverLocked.update((v) => !v);
  }
</script>

<svelte:window
  bind:innerWidth={winWidth}
  on:hashchange={onHashChange}
  on:keydown={onKeydown}
/>

{#if !booted}
  <!-- Deciding between connect screen, picker and graph. One local request;
       a spinner here would flash for longer than the wait itself. -->
  <div class="boot"></div>
{:else if $connection && $connection.kind !== 'ok'}
  <!-- Nothing answered on the resolved endpoint. A stranger gets a question
       they can act on rather than an empty graph and a console error. -->
  <ConnectScreen
    connection={$connection}
    attemptedBase={endpoint().base}
    onConnected={onEndpointChosen}
  />
{:else if $serveMode && !$activeRepo}
  <RepoPicker onSelect={onPickRepo} />
{:else}
<BuildStamp />

<div class="app-root">
  <!-- Left sidebar (standalone mode only — in VS Code the "Scopes" and
       "View Options" native views replace it). -->
  {#if !isVscode()}
    <div class="panel left-panel" class:collapsed={leftCollapsed} style="width: {leftCollapsed ? 0 : leftWidth}px">
      {#if !leftCollapsed}
        <Sidebar />
        <div class="resize-handle right" on:mousedown={startResize}></div>
      {/if}
    </div>
    <button class="panel-toggle left" class:collapsed={leftCollapsed}
      on:click={() => (leftCollapsed = !leftCollapsed)}>
      {leftCollapsed ? '\u25B6' : '\u25C0'}
    </button>
  {/if}

  <!-- Graph -->
  <div class="canvas-column">
  <!-- Toolbar lives above the canvas, not on top of it. As an overlay it
       covered nodes at small window sizes, and fitView measures
       .graph-container — so the area under the toolbar counted as usable
       and the fit put nodes there. In flow, both problems go away. -->
  {#if !isVscode()}
    <CanvasToolbar {graphView} />
  {/if}

  <GraphView bind:this={graphView}>
    {#if !isVscode()}
    <div class="stats">
      <!-- Commit picker drives `POST /api/diff`, which serve mode doesn't
           expose. Hidden there rather than offering a button that 404s. -->
      {#if !$serveMode}
        <CommitPicker />
      {/if}
      {#if $diffActive && $diffData}
        <span class="diff-summary-badge">
          🔀 {$diffData.from_ref}→{$diffData.to_ref}:
          <span style="color:#A5D6A7">+{$diffData.summary.added}</span>
          <span style="color:#EF9A9A">-{$diffData.summary.removed}</span>
          <span style="color:#FFCC80" title="{$diffData.summary.modified_source ?? $diffData.summary.modified} core, {$diffData.summary.modified_impact ?? 0} impact">
            ~{$diffData.summary.modified}
          </span>
          <span class="diff-filter-group">
            <label class="diff-filter-toggle" title="Show only added, removed, and modified entities (hide unchanged)">
              <input type="checkbox" bind:checked={$diffChangesOnly} />
              Changes
            </label>
            <label class="diff-filter-toggle" title="Show only core changes (source code modified) — hide impact-only changes (only relational metrics changed)">
              <input type="checkbox" bind:checked={$diffCoreOnly} />
              Core
            </label>
            {#if $diffChangesOnly || $diffCoreOnly}
              <label class="diff-filter-toggle diff-opacity-control" title="Opacity of unchanged/filtered nodes (0 = hidden, 100 = fully visible)">
                <input type="range" min="0" max="15" step="1"
                  value={$diffDimOpacity * 100}
                  on:input={(e) => diffDimOpacity.set(Number(e.currentTarget.value) / 100)} />
                <span class="diff-opacity-label">{Math.round($diffDimOpacity * 100)}%</span>
              </label>
            {/if}
          </span>
        </span>
      {/if}
    </div>

    <!-- Mode bar: static / live indicator + controls.
         The live toggle needs `/events`, which serve mode has no equivalent
         of (repos are analyzed once, not watched) — so it's hidden there and
         only the manual refresh remains, which works fine. -->
    <div class="mode-bar-bottom">
      {#if !$serveMode}
      <button
        type="button"
        class="mode-indicator"
        class:live={$liveConnected}
        class:broken={liveIsBroken($liveStatus)}
        class:reloading={$liveReloading || $refreshing}
        data-probe="live-indicator"
        data-live-state={$liveStatus.kind === 'stopped' ? $liveStatus.reason : $liveStatus.kind}
        on:click={toggleLiveMode}
        title={liveIsBroken($liveStatus) && $liveStatus.kind === 'stopped'
          ? LIVE_STOP_HINT[$liveStatus.reason]
          : $liveConnected
            ? 'Connected to watch server — click to disconnect'
            : 'Not connected — click to connect to watch server'}
      >
        {#if $liveReloading || $refreshing}
          <span class="mode-icon pulse">↻</span> Reloading…
        {:else if $liveConnected}
          <span class="mode-icon">●</span> Live
        {:else if $liveStatus.kind === 'stopped' && $liveStatus.reason !== 'off'}
          <!-- A stream that failed is not the same as one nobody started.
               "Static" for both is what made a refused origin read as
               "nothing is changing". -->
          <span class="mode-icon">⚠</span> {LIVE_STOP_LABEL[$liveStatus.reason]}
        {:else if $liveStatus.kind === 'retrying'}
          <span class="mode-icon pulse">○</span> Reconnecting…
        {:else}
          <span class="mode-icon">○</span> Static
        {/if}
      </button>
      {/if}
      {#if !$liveConnected}
        <button type="button" class="refresh-btn" on:click={manualRefresh} title="Manually reload data files">
          ↻ Refresh
        </button>
      {/if}
      <!-- Which engine this is. Hidden same-origin, where the answer is
           "the one that served this page" and a chip would be noise. -->
      {#if endpoint().base}
        <button
          type="button"
          class="endpoint-chip"
          data-probe="endpoint-chip"
          on:click={disconnectEndpoint}
          title="Connected to {endpoint().base} — click to disconnect and choose another"
        >
          ⇄ {endpoint().base.replace(/^https?:\/\//, '')}
        </button>
      {/if}
    </div>
    {/if}

    <!-- Placeholder / status overlay.
         Shown when:
           - the scope exceeds the render threshold (analysis still works
             in the side panels, the canvas just declines to draw), or
           - no scope is selected yet, or
           - a load error occurred -->
    {#if $indexData && !$graphLoading && ($scopeOversized || $graphData.nodes.length === 0)}
      <div class="overlay">
        {#if $scopeOversized}
          <div class="overlay-card warn">
            <h3>Scope too large to render</h3>
            <p>
              In scope: <strong>{$selectionStats.entities.toLocaleString()}</strong> entities and
              <strong>{$selectionStats.relationships.toLocaleString()}</strong> relationships,
              above the render threshold of <strong>{ENTITY_THRESHOLD.toLocaleString()}</strong>.
              The graph canvas is paused to protect UI performance.
            </p>
            <p>
              <strong>Analysis continues in the side panels</strong> —
              Quality, Summary, Diff, and Context all work on the full scope.
            </p>
            <p>
              To draw the graph: narrow the scope (fewer folders/files) or
              enable a filter that restricts what's shown — for example
              <em>Changes only</em> during a diff.
            </p>
          </div>
        {:else if $graphLoadError}
          <div class="overlay-card warn">
            <h3>Failed to load</h3>
            <p>{$graphLoadError}</p>
          </div>
        {:else}
          <div class="overlay-card">
            <h3>Pick a scope to visualize</h3>
            <p>
              Indexed: <strong>{$indexData.total_entities.toLocaleString()}</strong> entities and
              <strong>{$indexData.total_relationships.toLocaleString()}</strong> relationships across the whole repo.
            </p>
            {#if suggestedScope}
              <p>
                Start with <strong>{suggestedScope.path}</strong>
                ({suggestedScope.entity_count.toLocaleString()} entities), or check any
                folder in the <button type="button" class="link-btn" on:click={revealScopeTree}>Scope panel</button>.
              </p>
              <button type="button" class="start-here-btn" on:click={startHere}>
                Show {suggestedScope.path}
              </button>
            {:else}
              <p>
                Check the box next to one or more folders/files in the
                <button type="button" class="link-btn" on:click={revealScopeTree}>Scope panel</button>.
              </p>
            {/if}
            <p class="threshold-note">
              Items above the threshold of {ENTITY_THRESHOLD.toLocaleString()} entities are marked with ⚠ and won't render.
            </p>
          </div>
        {/if}
      </div>
    {:else if $graphLoading}
      <div class="overlay">
        <div class="overlay-card">
          <h3>Loading…</h3>
          <p>
            Fetching graph data for {$selectedScopes.size} selected
            scope{$selectedScopes.size === 1 ? '' : 's'}
            ({$selectionStats.entities.toLocaleString()} entities in scope)
          </p>
        </div>
      </div>
    {:else if $indexLoadError}
      <div class="overlay">
        <div class="overlay-card warn">
          <h3>Failed to load index</h3>
          <p>{$indexLoadError}</p>
          <p>Generate one with: <code>nao analyze &lt;path&gt; -f json -o ui/public/data.json</code></p>
        </div>
      </div>
    {/if}
  </GraphView>
  </div>

  <!-- Details: the entity under the pointer, or the pinned one. Its own
       column since UI-040 — as the sidebar's bottom half it took a third of
       the height the scope tree and the quality table were short of.
       In VS Code the native "Selection" view replaces it. -->
  {#if !isVscode()}
    <button class="panel-toggle details" class:collapsed={!showDetails}
      class:squashed={detailsSquashed}
      title={detailsSquashed
        ? 'Details is hidden — the window is too narrow for it and the canvas'
        : showDetails ? 'Hide details' : 'Show details'}
      on:click={() => detailsPaneOpen.set(!$detailsPaneOpen)}>
      {showDetails ? '▶' : '◀'}
    </button>
    <div class="panel details-panel" class:collapsed={!showDetails}
      data-probe="details-panel"
      style="width: {showDetails ? detailsShownWidth : 0}px">
      {#if showDetails}
        <div class="resize-handle left" on:mousedown={startDetailsResize}></div>
        <DetailsPanel />
      {/if}
    </div>
  {/if}

  <!-- Description: the graph as prose. Its own column rather than a tab
       sharing one with Details — reading it is a mode you stay in while the
       pointer sweeps the canvas, so it has to be visible at the same time as
       the metrics, not instead of them.
       In VS Code the native "Description" view replaces it. -->
  {#if !isVscode()}
    <button class="panel-toggle right" class:collapsed={!showDescription}
      class:squashed={descriptionSquashed}
      title={descriptionSquashed
        ? 'Descriptions are hidden — the window is too narrow for a third column'
        : showDescription ? 'Hide descriptions' : 'Show descriptions'}
      on:click={toggleDescription}>
      {showDescription ? '▶' : '◀'}
    </button>
    <div class="panel right-panel" class:collapsed={!showDescription}
      style="width: {showDescription ? DESCRIPTION_WIDTH : 0}px">
      {#if showDescription}
        <DescriptionPanel />
      {/if}
    </div>
  {/if}

</div>
{/if}

<style>
  .boot {
    height: 100vh;
    width: 100vw;
    background: var(--bg-deep);
  }

  .app-root {
    display: flex;
    height: 100vh;
    width: 100vw;
    overflow: hidden;
  }

  .panel {
    background: var(--bg-surface);
    overflow: hidden;
    position: relative;
    transition: width 0.2s;
    flex-shrink: 0;
  }

  .left-panel { border-right: 1px solid var(--border); }
  .details-panel { border-left: 1px solid var(--border); }
  .right-panel { border-left: 1px solid var(--border); }

  .panel.collapsed {
    width: 0 !important;
    border: none;
  }

  .resize-handle {
    position: absolute;
    top: 0;
    width: 5px;
    height: 100%;
    cursor: col-resize;
    z-index: 10;
  }

  .resize-handle:hover { background: var(--accent); }
  .resize-handle.right { right: 0; }
  .resize-handle.left { left: 0; }

  .panel-toggle {
    position: relative;
    z-index: 20;
    width: 20px;
    background: var(--bg-surface);
    border: 1px solid var(--border);
    color: var(--text-muted);
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 0.7rem;
    flex-shrink: 0;
  }

  .panel-toggle:hover { background: var(--bg-hover); color: var(--text); }
  .panel-toggle.left { border-left: none; border-right: none; }
  .panel-toggle.details { border-left: none; border-right: none; }
  .panel-toggle.right { border-left: none; border-right: none; }

  /* Wanted, but the window has no room for it. Dimmed rather than hidden:
     the strip is where the pane comes back from once the window grows. */
  .panel-toggle.squashed {
    color: var(--text-disabled);
    background: var(--bg-deep);
  }

  .canvas-column {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }

  .stats {
    position: absolute;
    bottom: 20px;
    left: 20px;
    background: color-mix(in srgb, var(--bg-surface) 90%, transparent);
    padding: 10px 15px;
    border-radius: 4px;
    font-size: 0.8rem;
    color: var(--text-muted);
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .diff-summary-badge {
    font-size: 0.75rem;
    padding: 2px 8px;
    border-radius: 10px;
    background: rgba(255, 167, 38, 0.1);
    border: 1px solid rgba(255, 167, 38, 0.3);
    display: inline-flex;
    align-items: center;
    gap: 8px;
  }

  .diff-filter-group {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding-left: 8px;
    border-left: 1px solid rgba(255, 167, 38, 0.3);
  }

  .diff-filter-toggle {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    cursor: pointer;
    font-size: 0.7rem;
    color: var(--text-muted, #aaa);
    user-select: none;
  }
  .diff-filter-toggle:hover {
    color: var(--text, #e0e0e0);
  }
  .diff-filter-toggle input[type="checkbox"] {
    margin: 0;
    cursor: pointer;
    accent-color: #FFA726;
  }

  .diff-opacity-control input[type="range"] {
    width: 60px;
    height: 4px;
    cursor: pointer;
    accent-color: #FFA726;
  }

  .diff-opacity-label {
    font-family: 'Monaco', 'Menlo', monospace;
    font-size: 0.65rem;
    min-width: 28px;
    text-align: right;
  }

  .mode-bar-bottom {
    position: absolute;
    bottom: 20px;
    right: 20px;
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .mode-indicator {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 6px 14px;
    border-radius: 20px;
    border: 1px solid var(--border, #0f3460);
    background: color-mix(in srgb, var(--bg-surface, #16213e) 90%, transparent);
    color: var(--text-muted, #aaa);
    font-size: 0.8rem;
    font-family: inherit;
    cursor: pointer;
    transition: all 0.2s;
  }
  .mode-indicator:hover { border-color: var(--text-dim, #666); }
  .mode-indicator.live {
    color: #66BB6A;
    border-color: rgba(102, 187, 106, 0.4);
    background: rgba(102, 187, 106, 0.08);
  }
  .mode-indicator.reloading {
    color: #FFA726;
    border-color: rgba(255, 167, 38, 0.4);
  }
  /* A stream that failed reads differently from one nobody started —
     same weight as `.live`, opposite sign, so "not updating" is a state
     you notice rather than the absence of one. */
  .mode-indicator.broken {
    color: #EF5350;
    border-color: rgba(239, 83, 80, 0.4);
    background: rgba(239, 83, 80, 0.08);
  }

  .endpoint-chip {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 6px 12px;
    border-radius: 20px;
    border: 1px solid var(--border, #0f3460);
    background: color-mix(in srgb, var(--bg-surface, #16213e) 90%, transparent);
    color: var(--text-muted, #aaa);
    font-size: 0.75rem;
    font-family: inherit;
    cursor: pointer;
  }
  .endpoint-chip:hover { color: var(--text); border-color: var(--text-dim, #666); }

  .mode-icon { font-size: 0.9rem; }
  .mode-icon.pulse {
    animation: pulse 0.8s ease-in-out infinite;
  }
  @keyframes pulse { 0%, 100% { opacity: 1; } 50% { opacity: 0.3; } }

  .refresh-btn {
    padding: 6px 12px;
    border-radius: 20px;
    border: 1px solid var(--border, #0f3460);
    background: color-mix(in srgb, var(--bg-surface, #16213e) 90%, transparent);
    color: var(--text-muted, #aaa);
    font-size: 0.78rem;
    font-family: inherit;
    cursor: pointer;
  }
  .refresh-btn:hover { border-color: var(--text-dim, #666); color: #fff; }

  .overlay {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    pointer-events: none;
  }

  .start-here-btn {
    margin-top: 10px;
    padding: 8px 18px;
    border-radius: 6px;
    border: 1px solid var(--accent);
    background: color-mix(in srgb, var(--accent) 14%, transparent);
    color: var(--accent);
    font: inherit;
    font-size: 0.9rem;
    font-weight: 600;
    cursor: pointer;
  }
  .start-here-btn:hover { background: color-mix(in srgb, var(--accent) 28%, transparent); }

  .link-btn {
    padding: 0;
    border: none;
    background: none;
    font: inherit;
    color: var(--accent);
    text-decoration: underline;
    cursor: pointer;
  }

  .threshold-note { color: var(--text-dim); font-size: 0.85em; }

  .overlay-card {
    pointer-events: auto;
    max-width: 520px;
    background: color-mix(in srgb, var(--bg-surface) 95%, transparent);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 24px 28px;
    color: var(--text-secondary);
    font-size: 0.9rem;
    box-shadow: 0 4px 20px rgba(0, 0, 0, 0.4);
  }

  .overlay-card.warn {
    border-color: #c04040;
  }

  .overlay-card h3 {
    margin: 0 0 12px;
    color: var(--text);
  }

  .overlay-card p {
    margin: 8px 0;
    line-height: 1.5;
  }

  .overlay-card code {
    background: var(--bg-deep);
    padding: 2px 6px;
    border-radius: 3px;
    font-family: 'Monaco', 'Menlo', monospace;
    font-size: 0.8rem;
  }
</style>
