import * as vscode from 'vscode';
import type { FilterState, LevelFilterState } from './panel';
import type { SelectionPayload } from './sideViewProvider';

/**
 * Native "Controls" sidebar — a single webview with three tabs that used to be
 * three separate views (each cramped by VS Code's fixed sidebar height):
 *
 *   • Options — the old "View Options": view mode, aggregation level, tree
 *     depth/density, hover depth, labels, layers, zoom, editor-sync.
 *   • Filters — the general filter layer: entity/relationship/language chips,
 *     edge direction, ghosts.
 *   • Node    — per-depth (L1/L2/L3) tri-state overrides for the selected
 *     node's neighborhood, plus edge-display toggles.
 *
 * Consolidating them means one view owns the sidebar slice, so each tab gets
 * the full height instead of a third of it.
 *
 * Message protocol (webview → extension):
 *   { type: 'command',       command, value }  → Options controls
 *   { type: 'filterCommand', command, value }  → Filters / Node controls
 * The two channels are kept distinct because Options commands are handled here
 * (setFollowSelection) or forwarded to the panel, whereas Filters/Node commands
 * always route through `nao.internalFilterCommand`.
 */
export class ControlsViewProvider implements vscode.WebviewViewProvider {
  static readonly viewType = 'nao.controls';

  private view?: vscode.WebviewView;
  private commandHandler?: (command: string, value: unknown) => void;

  // Latest pushed state, replayed when the webview (re)connects. Webviews are
  // torn down when the view is hidden, so `ready` must rehydrate all tabs.
  private lastFilterState?: FilterState;
  private lastLevelState?: LevelFilterState;
  private lastSelection?: { name: string; kind: string } | undefined;
  /** Mirror of the extension's `followSelection`. The Options tab is otherwise
   *  stateless button chrome, but this one toggle owns behaviour that outlives
   *  the webview: without mirroring it, hiding and re-showing the view redraws
   *  the checkbox unchecked while following stays on, and it then takes two
   *  clicks to turn off what the UI claims is already off. */
  private followSelection = false;

  onCommand(handler: (command: string, value: unknown) => void): void {
    this.commandHandler = handler;
  }

  /** Keep the checkbox honest about the extension-side toggle. Stamped into
   *  the HTML rather than posted on `ready`, because `getHtml()` re-runs on
   *  every resolve — the markup *is* the rehydration point. */
  setFollowSelection(on: boolean): void {
    this.followSelection = on;
  }

  updateFilters(state: FilterState): void {
    this.lastFilterState = state;
    this.view?.webview.postMessage({ type: 'filtersState', state });
  }

  updateLevelFilters(state: LevelFilterState): void {
    this.lastLevelState = state;
    this.postLevel();
  }

  setSelection(payload: SelectionPayload | undefined): void {
    this.lastSelection = payload ? { name: payload.name, kind: payload.kind } : undefined;
    this.postLevel();
  }

  private postLevel(): void {
    this.view?.webview.postMessage({
      type: 'levelState',
      state: this.lastLevelState,
      selection: this.lastSelection,
    });
  }

  resolveWebviewView(
    webviewView: vscode.WebviewView,
    _context: vscode.WebviewViewResolveContext,
    _token: vscode.CancellationToken
  ): void {
    this.view = webviewView;
    webviewView.webview.options = { enableScripts: true };
    webviewView.webview.html = this.getHtml();

    webviewView.webview.onDidReceiveMessage(
      (msg: { type: string; command?: string; value?: unknown }) => {
        if (msg?.type === 'ready') {
          if (this.lastFilterState) {
            webviewView.webview.postMessage({ type: 'filtersState', state: this.lastFilterState });
          }
          this.postLevel();
        } else if (msg?.type === 'command' && msg.command) {
          this.commandHandler?.(msg.command, msg.value);
        } else if (msg?.type === 'filterCommand' && msg.command) {
          vscode.commands.executeCommand('nao.internalFilterCommand', {
            command: msg.command,
            value: msg.value,
          });
        }
      }
    );
  }

  private getHtml(): string {
    return /* html */ `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<style>
  body { font-family: var(--vscode-font-family); font-size: var(--vscode-font-size); color: var(--vscode-foreground); margin: 0; padding: 0; }

  /* Tab bar */
  .tab-bar {
    display: flex; position: sticky; top: 0; z-index: 5;
    background: var(--vscode-sideBar-background, var(--vscode-editor-background));
    border-bottom: 1px solid var(--vscode-panel-border);
  }
  .tab {
    flex: 1; padding: 6px 4px; background: transparent; color: var(--vscode-descriptionForeground);
    border: none; border-bottom: 2px solid transparent; cursor: pointer;
    font-size: 0.8em; text-transform: uppercase; letter-spacing: 0.04em; font-family: inherit;
  }
  .tab:hover { color: var(--vscode-foreground); }
  .tab.active { color: var(--vscode-foreground); border-bottom-color: var(--vscode-focusBorder, var(--vscode-textLink-foreground)); }

  .pane { display: none; }
  .pane.active { display: block; }
  #pane-options { padding: 8px 12px; }
  #pane-filters { padding: 8px 10px; }
  #pane-node    { padding: 8px 10px; }

  /* ---- Shared / Options ---- */
  .section { margin-bottom: 14px; }
  .section-title { font-size: 0.75em; text-transform: uppercase; letter-spacing: 0.05em; color: var(--vscode-descriptionForeground); margin-bottom: 6px; }
  .segmented { display: inline-flex; border: 1px solid var(--vscode-panel-border); border-radius: 3px; overflow: hidden; width: 100%; }
  .segmented button { flex: 1; padding: 4px 6px; background: transparent; color: var(--vscode-foreground); border: none; border-right: 1px solid var(--vscode-panel-border); cursor: pointer; font-size: 0.9em; }
  .segmented button:last-child { border-right: none; }
  .segmented button:hover { background: var(--vscode-list-hoverBackground); }
  .segmented button.active { background: var(--vscode-button-background); color: var(--vscode-button-foreground); }
  .action-row { display: grid; grid-template-columns: 1fr 1fr; gap: 4px; }
  .action-row.triple { grid-template-columns: repeat(3, 1fr); }
  button.action {
    padding: 4px 8px; background: transparent; color: var(--vscode-foreground);
    border: 1px solid var(--vscode-panel-border); border-radius: 3px; cursor: pointer; font-size: 0.85em;
  }
  button.action:hover { background: var(--vscode-list-hoverBackground); }
  #pane-options .toggle-row { display: flex; align-items: center; justify-content: space-between; padding: 2px 0; font-size: 0.9em; }
  #pane-options .toggle-row label { cursor: pointer; }
  input[type="checkbox"] { margin: 0; }

  .empty { color: var(--vscode-descriptionForeground); font-style: italic; padding: 12px 0; }

  /* ---- Filters ---- */
  .section-header {
    display: flex; justify-content: space-between; align-items: center;
    font-size: 0.72em; text-transform: uppercase; letter-spacing: 0.05em;
    color: var(--vscode-descriptionForeground); margin-bottom: 4px;
  }
  .bulk-actions { display: flex; gap: 4px; }
  .bulk-btn {
    background: transparent; border: none; color: var(--vscode-textLink-foreground);
    cursor: pointer; font-size: 0.9em; padding: 0 2px;
  }
  .bulk-btn:hover { color: var(--vscode-textLink-activeForeground); }
  .chips { display: flex; flex-wrap: wrap; gap: 3px; }
  .chip {
    font-size: 0.8em; padding: 2px 7px;
    border: 1px solid var(--vscode-panel-border); border-radius: 10px;
    cursor: pointer; user-select: none; background: transparent;
    color: var(--vscode-descriptionForeground);
  }
  .chip:hover { background: var(--vscode-list-hoverBackground); }
  .chip.on { background: var(--vscode-button-background); color: var(--vscode-button-foreground); border-color: transparent; }
  .directions { display: grid; grid-template-columns: 1fr 1fr; gap: 4px; }
  .dir-toggle {
    display: flex; align-items: center; gap: 6px; font-size: 0.85em; padding: 4px 8px;
    border: 1px solid var(--vscode-panel-border); border-radius: 3px; cursor: pointer;
    background: transparent; color: var(--vscode-foreground);
  }
  .dir-toggle:hover { background: var(--vscode-list-hoverBackground); }
  .dir-toggle.on { background: var(--vscode-button-background); color: var(--vscode-button-foreground); border-color: transparent; }
  #pane-filters .toggle-row { display: flex; flex-direction: column; gap: 4px; }
  #pane-filters .toggle-row label {
    display: flex; align-items: center; gap: 6px;
    font-size: 0.85em; color: var(--vscode-descriptionForeground); cursor: pointer;
  }
  #pane-filters .toggle-row input[type="checkbox"] { cursor: pointer; }

  /* ---- Node (level filters) ---- */
  .level { margin-bottom: 10px; border: 1px solid var(--vscode-panel-border); border-radius: 4px; overflow: hidden; }
  .level.disabled .level-body { opacity: 0.5; pointer-events: none; }
  .level-head {
    display: flex; align-items: center; gap: 6px; padding: 6px 8px;
    background: var(--vscode-textCodeBlock-background); cursor: pointer; user-select: none; font-weight: 500;
  }
  .level-head:hover { background: var(--vscode-list-hoverBackground); }
  .level-head input[type="checkbox"] { margin: 0; cursor: pointer; }
  .level-head .caret { margin-left: auto; font-size: 0.75em; color: var(--vscode-descriptionForeground); }
  .level-head.l1 { border-left: 3px solid #4CAF50; }
  .level-head.l2 { border-left: 3px solid #FF9800; }
  .level-head.l3 { border-left: 3px solid #E91E63; }
  .level-body { padding: 8px; display: none; flex-direction: column; gap: 8px; }
  .level.open .level-body { display: flex; }
  .sub-title {
    font-size: 0.7em; text-transform: uppercase; letter-spacing: 0.05em;
    color: var(--vscode-descriptionForeground); margin-bottom: 2px;
  }
  .tri {
    display: inline-flex; align-items: center; gap: 3px; padding: 2px 5px 2px 2px;
    border: 1px solid var(--vscode-panel-border); border-radius: 3px; cursor: pointer;
    font-size: 0.8em; background: transparent; color: var(--vscode-foreground); user-select: none;
  }
  .tri:hover { background: var(--vscode-list-hoverBackground); }
  .tri .state {
    display: inline-block; width: 14px; text-align: center;
    font-family: var(--vscode-editor-font-family, monospace); font-size: 0.85em; font-weight: 600;
    border-radius: 2px; padding: 0 2px; line-height: 1.3;
  }
  .tri.s-on    .state { background: rgba(76, 175, 80, 0.22); color: #73c990; }
  .tri.s-off   .state { background: rgba(244, 67, 54, 0.22); color: #f48771; }
  .tri.s-general .state { background: var(--vscode-badge-background); color: var(--vscode-badge-foreground); }
  .peer-row label { display: inline-flex; align-items: center; gap: 6px; font-size: 0.85em; cursor: pointer; }
  .edge-display { margin-top: 14px; padding-top: 10px; border-top: 1px solid var(--vscode-panel-border); }
  .edge-display .toggle-row { display: flex; align-items: center; justify-content: space-between; font-size: 0.85em; padding: 3px 0; }
  .edge-display label { cursor: pointer; display: flex; align-items: center; gap: 6px; }
  .selected-info {
    font-size: 0.8em; padding: 4px 6px; margin-bottom: 10px;
    background: var(--vscode-textCodeBlock-background); border-radius: 3px;
    display: flex; align-items: center; gap: 6px;
    white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
  }
  .selected-info .kind {
    font-size: 0.85em; text-transform: uppercase; padding: 1px 5px; border-radius: 2px;
    background: var(--vscode-badge-background); color: var(--vscode-badge-foreground); flex-shrink: 0;
  }
  .selected-info .name { font-weight: 500; overflow: hidden; text-overflow: ellipsis; }
  .legend {
    font-size: 0.74em; padding: 6px 8px; margin-bottom: 10px;
    background: var(--vscode-textBlockQuote-background, rgba(127,127,127,0.08));
    border-left: 2px solid var(--vscode-textLink-foreground); border-radius: 2px;
    color: var(--vscode-descriptionForeground); line-height: 1.5;
  }
  .legend .k {
    display: inline-block; width: 14px; text-align: center;
    font-family: var(--vscode-editor-font-family, monospace); font-weight: 600;
    padding: 0 2px; border-radius: 2px; margin: 0 2px;
  }
  .legend .k-on    { background: rgba(76, 175, 80, 0.22); color: #73c990; }
  .legend .k-off   { background: rgba(244, 67, 54, 0.22); color: #f48771; }
  .legend .k-gen   { background: var(--vscode-badge-background); color: var(--vscode-badge-foreground); }
</style>
</head>
<body>
  <div class="tab-bar">
    <button class="tab" data-tab="options">Options</button>
    <button class="tab" data-tab="filters">Filters</button>
    <button class="tab" data-tab="node">Node</button>
  </div>

  <!-- ===== Options ===== -->
  <div class="pane" id="pane-options">
    <div class="section">
      <div class="section-title">Editor Sync</div>
      <div class="toggle-row">
        <label><input type="checkbox" data-toggle="setFollowSelection"${this.followSelection ? ' checked' : ''} /> Follow selection to editor</label>
      </div>
      <div style="font-size:0.75em; color:var(--vscode-descriptionForeground); margin-top:2px">
        When on, clicking a node opens the file at its line.
      </div>
      <div class="toggle-row">
        <label><input type="checkbox" data-toggle="setDescribeOnHover" checked /> Describe hovered node</label>
      </div>
      <div style="font-size:0.75em; color:var(--vscode-descriptionForeground); margin-top:2px">
        When on, the <b>Description</b> view follows the pointer. Off, it
        follows the selection only.
      </div>
    </div>

    <div class="section">
      <div class="section-title">Diff</div>
      <div class="toggle-row">
        <label><input type="checkbox" data-toggle="setDiffFiltersEnabled" checked /> Apply diff filters</label>
      </div>
      <div style="font-size:0.75em; color:var(--vscode-descriptionForeground); margin-top:2px">
        When off, a loaded diff still colors entities but <b>Changes only</b>,
        <b>Core only</b>, and <b>Dim opacity</b> don't hide or dim anything.
      </div>
    </div>

    <div class="section">
      <div class="section-title">View Mode</div>
      <div class="segmented" data-group="viewMode">
        <button data-value="graph" class="active">Graph</button>
        <button data-value="tree">Tree</button>
      </div>
    </div>

    <div class="section">
      <div class="section-title">Aggregation Level</div>
      <div class="segmented" data-group="graphLevel">
        <button data-value="entity" class="active">Entity</button>
        <button data-value="file">File</button>
        <button data-value="module">Module</button>
      </div>
    </div>

    <div class="section">
      <div class="section-title">Tree Depth</div>
      <div class="segmented" data-group="treeDepth">
        <button data-value="1">L1</button>
        <button data-value="2" class="active">L2</button>
        <button data-value="3">L3</button>
      </div>
    </div>

    <div class="section">
      <div class="section-title">Tree Density</div>
      <div class="segmented" data-group="treeDensity">
        <button data-value="compact">Compact</button>
        <button data-value="normal" class="active">Normal</button>
        <button data-value="spacious">Spacious</button>
      </div>
    </div>

    <div class="section">
      <div class="section-title">Hover Depth</div>
      <div class="segmented" data-group="hoverDepth">
        <button data-value="1" class="active">H1</button>
        <button data-value="2">H2</button>
        <button data-value="3">H3</button>
      </div>
    </div>

    <div class="section">
      <div class="section-title">Labels</div>
      <div class="toggle-row"><label><input type="checkbox" data-toggle="setShowLabels"> Node labels</label></div>
      <div class="toggle-row"><label><input type="checkbox" data-toggle="setShowKindLabels"> Kind labels</label></div>
      <div class="toggle-row"><label><input type="checkbox" data-toggle="setShowLinkLabels"> Link labels</label></div>
      <div class="toggle-row"><label><input type="checkbox" data-toggle="setAutoFit"> Auto-fit view</label></div>
    </div>

    <div class="section">
      <div class="section-title">Layers</div>
      <div class="toggle-row"><label><input type="checkbox" data-toggle="setShowTemplateVars"> Template variables (Ansible)</label></div>
    </div>

    <div class="section">
      <div class="section-title">Zoom</div>
      <div class="action-row triple">
        <button class="action" data-action="zoomIn">+ In</button>
        <button class="action" data-action="zoomOut">- Out</button>
        <button class="action" data-action="resetZoom">Reset</button>
      </div>
      <div class="action-row" style="margin-top:4px">
        <button class="action" data-action="fitView">Fit View</button>
        <button class="action" data-action="fitWidth">Fit Width</button>
      </div>
    </div>

    <div class="section">
      <button class="action" style="width:100%" data-action="clearSelection">Clear Selection</button>
    </div>
  </div>

  <!-- ===== Filters ===== -->
  <div class="pane" id="pane-filters">
    <div id="filters-root">
      <div class="empty">No graph loaded yet. Select a scope to see filters.</div>
    </div>
  </div>

  <!-- ===== Node (level filters) ===== -->
  <div class="pane" id="pane-node">
    <div id="level-root">
      <div class="empty">No graph loaded yet. Select a scope to see level filters.</div>
    </div>
  </div>

<script>
  const vscode = acquireVsCodeApi();
  const prev = vscode.getState() || {};

  function escapeHtml(s) {
    return String(s == null ? '' : s).replace(/[&<>"']/g, (c) => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
  }

  // ---- Tabs ----
  let activeTab = prev.activeTab || 'options';
  function setTab(tab) {
    activeTab = tab;
    vscode.setState({ ...(vscode.getState() || {}), activeTab: tab });
    document.querySelectorAll('.tab').forEach((t) => t.classList.toggle('active', t.getAttribute('data-tab') === tab));
    document.querySelectorAll('.pane').forEach((p) => p.classList.toggle('active', p.id === 'pane-' + tab));
  }
  document.querySelectorAll('.tab').forEach((t) => {
    t.addEventListener('click', () => setTab(t.getAttribute('data-tab')));
  });
  setTab(activeTab);

  // ================= Options =================
  (function () {
    const pane = document.getElementById('pane-options');
    const groupToCommand = {
      viewMode: 'setViewMode',
      graphLevel: 'setGraphLevel',
      treeDepth: 'setTreeDepth',
      treeDensity: 'setTreeDensity',
      hoverDepth: 'setHoverDepth',
    };
    pane.querySelectorAll('.segmented').forEach((group) => {
      const groupName = group.getAttribute('data-group');
      const command = groupToCommand[groupName];
      group.querySelectorAll('button').forEach((btn) => {
        btn.addEventListener('click', () => {
          group.querySelectorAll('button').forEach((b) => b.classList.remove('active'));
          btn.classList.add('active');
          let value = btn.getAttribute('data-value');
          if (groupName === 'treeDepth' || groupName === 'hoverDepth') value = Number(value);
          vscode.postMessage({ type: 'command', command, value });
        });
      });
    });
    pane.querySelectorAll('input[type="checkbox"][data-toggle]').forEach((cb) => {
      cb.addEventListener('change', () => {
        vscode.postMessage({ type: 'command', command: cb.getAttribute('data-toggle'), value: cb.checked });
      });
    });
    pane.querySelectorAll('button.action[data-action]').forEach((btn) => {
      btn.addEventListener('click', () => {
        vscode.postMessage({ type: 'command', command: btn.getAttribute('data-action') });
      });
    });
  })();

  // ================= Filters =================
  const Filters = (function () {
    const root = document.getElementById('filters-root');
    let state = null;

    function renderChips(items, cmd) {
      if (!items || items.length === 0) return '<div class="empty" style="padding:4px 0">none in scope</div>';
      return '<div class="chips">'
        + items.map((it) =>
            '<button class="chip' + (it.enabled ? ' on' : '') + '" '
            + 'data-cmd="' + cmd + '" '
            + 'data-name="' + escapeHtml(it.name) + '" '
            + 'data-enabled="' + (!it.enabled) + '" '
            + 'title="Click to ' + (it.enabled ? 'hide' : 'show') + ' ' + escapeHtml(it.name) + '">'
            + escapeHtml(it.name)
            + '</button>'
          ).join('')
        + '</div>';
    }

    function render() {
      if (!state) {
        root.innerHTML = '<div class="empty">No graph loaded yet. Select a scope to see filters.</div>';
        return;
      }
      const allE = state.entityTypes.map((e) => e.name);
      const allR = state.relTypes.map((r) => r.name);
      root.innerHTML =
        '<div class="section">'
        + '<div class="section-header"><span>Entity types</span>'
        + '<span class="bulk-actions">'
        + '<button class="bulk-btn" data-bulk="entitiesAll">All</button>'
        + '<button class="bulk-btn" data-bulk="entitiesNone">None</button>'
        + '</span></div>'
        + renderChips(state.entityTypes, 'toggleEntityType')
        + '</div>'

        + '<div class="section">'
        + '<div class="section-header"><span>Relationship types</span>'
        + '<span class="bulk-actions">'
        + '<button class="bulk-btn" data-bulk="relsAll">All</button>'
        + '<button class="bulk-btn" data-bulk="relsNone">None</button>'
        + '</span></div>'
        + renderChips(state.relTypes, 'toggleRelType')
        + '</div>'

        + '<div class="section">'
        + '<div class="section-header"><span>Direction</span></div>'
        + '<div class="directions">'
        + '<button class="dir-toggle' + (state.directions.outgoing ? ' on' : '') + '" data-dir="outgoing" title="Show edges pointing out from each node">→ Outgoing</button>'
        + '<button class="dir-toggle' + (state.directions.incoming ? ' on' : '') + '" data-dir="incoming" title="Show edges pointing into each node">← Incoming</button>'
        + '</div></div>'

        + '<div class="section">'
        + '<div class="section-header"><span>Languages</span></div>'
        + renderChips(state.languages, 'toggleLanguage')
        + '</div>'

        + '<div class="section">'
        + '<div class="section-header"><span>Ghosts</span></div>'
        + '<div class="toggle-row">'
        + '<label><input type="checkbox" data-ghost="master"' + (state.showGhosts ? ' checked' : '') + '>'
        + 'Ghost nodes (external refs)</label>'
        + '<label><input type="checkbox" data-ghost="builtins"' + (state.showBuiltinGhosts ? ' checked' : '') + '>'
        + 'Builtins (print, len, Vec, …)</label>'
        + '</div></div>';

      root.querySelectorAll('.chip').forEach((btn) => {
        btn.addEventListener('click', () => {
          vscode.postMessage({ type: 'filterCommand', command: btn.getAttribute('data-cmd'), value: { name: btn.getAttribute('data-name'), enabled: btn.getAttribute('data-enabled') === 'true' } });
        });
      });
      root.querySelectorAll('.bulk-btn').forEach((btn) => {
        btn.addEventListener('click', () => {
          const action = btn.getAttribute('data-bulk');
          if (action === 'entitiesAll') vscode.postMessage({ type: 'filterCommand', command: 'selectAllEntityTypes', value: allE });
          else if (action === 'entitiesNone') vscode.postMessage({ type: 'filterCommand', command: 'clearAllEntityTypes' });
          else if (action === 'relsAll') vscode.postMessage({ type: 'filterCommand', command: 'selectAllRelTypes', value: allR });
          else if (action === 'relsNone') vscode.postMessage({ type: 'filterCommand', command: 'clearAllRelTypes' });
        });
      });
      root.querySelectorAll('.dir-toggle').forEach((btn) => {
        btn.addEventListener('click', () => {
          const dir = btn.getAttribute('data-dir');
          const cmd = dir === 'outgoing' ? 'setOutgoing' : 'setIncoming';
          vscode.postMessage({ type: 'filterCommand', command: cmd, value: !btn.classList.contains('on') });
        });
      });
      root.querySelectorAll('input[data-ghost]').forEach((el) => {
        el.addEventListener('change', () => {
          const cmd = el.getAttribute('data-ghost') === 'master' ? 'setShowGhosts' : 'setShowBuiltinGhosts';
          vscode.postMessage({ type: 'filterCommand', command: cmd, value: el.checked });
        });
      });
    }

    return { apply(s) { state = s; render(); } };
  })();

  // ================= Node (level filters) =================
  const Level = (function () {
    const root = document.getElementById('level-root');
    let state = null;
    let selection = null;
    const openLevels = new Set([1]);

    function triSymbol(v) { return v === 'on' ? '✓' : v === 'off' ? '✗' : 'G'; }

    function renderTriChips(list, level, kind, stateMap) {
      if (!list || list.length === 0) return '<div class="empty" style="padding:2px 0">none</div>';
      return '<div class="chips">'
        + list.map((name) => {
            const s = stateMap[name] || 'general';
            return '<button class="tri s-' + s + '" '
              + 'data-level="' + level + '" data-kind="' + kind + '" data-key="' + escapeHtml(name) + '" '
              + 'title="' + escapeHtml(name) + ' — click to cycle (on → off → inherit)">'
              + '<span class="state">' + triSymbol(s) + '</span><span>' + escapeHtml(name) + '</span>'
              + '</button>';
          }).join('')
        + '</div>';
    }

    function renderLevel(level) {
      const lo = state.levels[level];
      if (!lo) return '';
      const title = level === 1 ? '1st Level (Direct)' : level === 2 ? '2nd Level (Indirect)' : '3rd Level (3 hops)';
      const isOpen = openLevels.has(level);
      return '<div class="level ' + (isOpen ? 'open ' : '') + (lo.enabled ? '' : 'disabled') + '">'
        + '<div class="level-head l' + level + '" data-action="toggleOpen" data-level="' + level + '">'
        + '<input type="checkbox"' + (lo.enabled ? ' checked' : '') + ' data-action="toggleEnabled" data-level="' + level + '">'
        + '<span>' + title + '</span>'
        + '<span class="caret">' + (isOpen ? '▾' : '▸') + '</span>'
        + '</div>'
        + '<div class="level-body">'
        + '<div class="peer-row"><label>'
        + '<input type="checkbox"' + (lo.peerEdges ? ' checked' : '') + ' data-action="togglePeer" data-level="' + level + '">'
        + 'Peer edges (between same-level nodes)</label></div>'
        + '<div><div class="sub-title">Entity Types</div>'
        + renderTriChips(state.allEntityTypes, level, 'entity', lo.entityTypes)
        + '</div>'
        + '<div><div class="sub-title">Direction</div>'
        + '<div class="chips">'
        + '<button class="tri s-' + lo.outgoing + '" data-level="' + level + '" data-kind="direction" data-key="outgoing" title="Outgoing edges">'
        + '<span class="state">' + triSymbol(lo.outgoing) + '</span><span>→ Outgoing</span>'
        + '</button>'
        + '<button class="tri s-' + lo.incoming + '" data-level="' + level + '" data-kind="direction" data-key="incoming" title="Incoming edges">'
        + '<span class="state">' + triSymbol(lo.incoming) + '</span><span>← Incoming</span>'
        + '</button>'
        + '</div></div>'
        + '<div><div class="sub-title">Relationship Types</div>'
        + renderTriChips(state.allRelTypes, level, 'rel', lo.relTypes)
        + '</div>'
        + '</div>'
        + '</div>';
    }

    function render() {
      if (!state) {
        root.innerHTML = '<div class="empty">No graph loaded yet. Select a scope to see these filters.</div>';
        return;
      }
      if (!selection) {
        root.innerHTML = '<div class="empty">➤ Select a node in the graph to filter its neighborhood by depth.<br><br>'
          + 'These filters shape what appears <em>1, 2, or 3 hops away</em> from whatever node you pick.</div>';
        return;
      }
      const legend = '<div class="legend">'
        + 'Click a chip to cycle how this kind is treated <b>at that depth</b>:<br>'
        + '<span class="k k-on">✓</span> force-show (override Filters) · '
        + '<span class="k k-off">✗</span> force-hide (override Filters) · '
        + '<span class="k k-gen">G</span> inherit from <b>Filters</b>'
        + '</div>';
      const header = '<div class="selected-info" title="' + escapeHtml(selection.name) + '">'
        + '<span class="kind">' + escapeHtml(selection.kind) + '</span>'
        + '<span class="name">' + escapeHtml(selection.name) + '</span>'
        + '</div>';
      root.innerHTML =
        header + legend
        + renderLevel(1) + renderLevel(2) + renderLevel(3)
        + '<div class="edge-display">'
        + '<div class="sub-title">Edge Display</div>'
        + '<div class="toggle-row"><label>'
        + '<input type="checkbox"' + (state.showDirectEdges ? ' checked' : '') + ' data-action="setDirect">'
        + 'Direct edges (selected node)</label></div>'
        + '<div class="toggle-row"><label>'
        + '<input type="checkbox"' + (state.showCrossLevelEdges ? ' checked' : '') + ' data-action="setCross">'
        + 'Cross-level edges (L1↔L2↔L3)</label></div>'
        + '</div>';

      root.querySelectorAll('.level-head').forEach((el) => {
        el.addEventListener('click', (ev) => {
          if (ev.target instanceof HTMLInputElement) return;
          const level = Number(el.getAttribute('data-level'));
          if (openLevels.has(level)) openLevels.delete(level); else openLevels.add(level);
          render();
        });
      });
      root.querySelectorAll('input[data-action="toggleEnabled"]').forEach((cb) => {
        cb.addEventListener('click', (ev) => ev.stopPropagation());
        cb.addEventListener('change', () => {
          vscode.postMessage({ type: 'filterCommand', command: 'toggleLevelEnabled', value: Number(cb.getAttribute('data-level')) });
        });
      });
      root.querySelectorAll('input[data-action="togglePeer"]').forEach((cb) => {
        cb.addEventListener('change', () => {
          vscode.postMessage({ type: 'filterCommand', command: 'toggleLevelPeerEdges', value: Number(cb.getAttribute('data-level')) });
        });
      });
      root.querySelectorAll('.tri').forEach((btn) => {
        btn.addEventListener('click', () => {
          const level = Number(btn.getAttribute('data-level'));
          const kind = btn.getAttribute('data-kind');
          const key = btn.getAttribute('data-key');
          let cmd, value;
          if (kind === 'entity') { cmd = 'cycleLevelEntityType'; value = { level, key }; }
          else if (kind === 'rel') { cmd = 'cycleLevelRelType'; value = { level, key }; }
          else if (kind === 'direction') { cmd = 'cycleLevelDirection'; value = { level, dir: key }; }
          if (cmd) vscode.postMessage({ type: 'filterCommand', command: cmd, value });
        });
      });
      root.querySelectorAll('input[data-action="setDirect"]').forEach((cb) => {
        cb.addEventListener('change', () => {
          vscode.postMessage({ type: 'filterCommand', command: 'setShowDirectEdges', value: cb.checked });
        });
      });
      root.querySelectorAll('input[data-action="setCross"]').forEach((cb) => {
        cb.addEventListener('change', () => {
          vscode.postMessage({ type: 'filterCommand', command: 'setShowCrossLevelEdges', value: cb.checked });
        });
      });
    }

    return { apply(s, sel) { state = s; selection = sel || null; render(); } };
  })();

  // ---- Inbound state ----
  window.addEventListener('message', (event) => {
    const msg = event.data;
    if (msg?.type === 'filtersState') Filters.apply(msg.state);
    else if (msg?.type === 'levelState') Level.apply(msg.state, msg.selection);
  });

  vscode.postMessage({ type: 'ready' });
</script>
</body>
</html>`;
  }
}
