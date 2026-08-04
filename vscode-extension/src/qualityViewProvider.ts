import * as vscode from 'vscode';
import type { QualityItem, QualityPayload, QualitySummary } from './panel';
import { AgentLaunchError, spawnAgentTerminal } from './agentTerminal';

/**
 * Native "Quality" side view: shows the top refactor candidates computed by
 * the main graph panel. Clicking a row selects the entity in the graph and
 * (if Follow Selection is on) opens the source file at its line.
 */
export class QualityViewProvider implements vscode.WebviewViewProvider {
  static readonly viewType = 'nao.quality';

  private view?: vscode.WebviewView;
  private lastRows: QualityItem[] = [];
  private lastSummary: QualitySummary | undefined;
  private selectEntityHandler?: (entityId: string) => void;
  private goToSourceHandler?: (file: string, line: number) => void;
  /** Engine port, set by extension.ts once the server is up (UI-037). */
  private serverPort?: number;

  setServerPort(port: number): void {
    this.serverPort = port;
  }

  /**
   * Launch a Claude Code session against one entity (UI-037).
   *
   * Failure is loud on purpose: a launch that silently does nothing is worse
   * than one that says why. The copy-prompt fallback is offered inline so a
   * user without Claude Code installed still gets the value of the prompt.
   */
  private async spawnAgent(entityId: string, entityName: string): Promise<void> {
    if (this.serverPort === undefined) {
      vscode.window.showWarningMessage('Nao: the analysis server is not running yet.');
      return;
    }
    const cwd = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
    if (!cwd) {
      vscode.window.showWarningMessage('Nao: open a folder before launching an agent.');
      return;
    }
    try {
      await spawnAgentTerminal({
        port: this.serverPort,
        entityId,
        entityName,
        cwd,
        binary: vscode.workspace.getConfiguration('nao').get<string>('claudeBinary'),
      });
    } catch (e) {
      const detail = e instanceof Error ? e.message : String(e);
      // When the fetch succeeded and only the binary was missing we already
      // hold the prompt — offer it rather than making the user start over.
      const prompt = e instanceof AgentLaunchError ? e.prompt : undefined;
      const copy = 'Copy prompt instead';
      const choice = await vscode.window.showErrorMessage(
        `Nao: could not launch an agent for ${entityName} — ${detail}`,
        ...(prompt ? [copy] : [])
      );
      if (choice === copy && prompt) {
        await vscode.env.clipboard.writeText(prompt);
        vscode.window.setStatusBarMessage('$(clippy) Refactor prompt copied to clipboard', 2000);
      }
    }
  }

  onSelectEntity(handler: (entityId: string) => void): void {
    this.selectEntityHandler = handler;
  }

  onGoToSource(handler: (file: string, line: number) => void): void {
    this.goToSourceHandler = handler;
  }

  private lastPayload?: QualityPayload;

  update(payload: QualityPayload): void {
    this.lastRows = payload.rows;
    this.lastSummary = payload.summary;
    this.lastPayload = payload;
    this.view?.webview.postMessage({
      type: 'rows',
      rows: payload.rows,
      summary: payload.summary,
      analysisScope: payload.analysisScope,
      availableScopes: payload.availableScopes,
      currentFile: payload.currentFile,
      sortBy: payload.sortBy,
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

    // The webview posts { type: 'ready' } once its message listener is
    // attached. Eager posts before that are lost because the listener
    // hasn't registered yet. Responding to ready avoids the race.
    webviewView.webview.onDidReceiveMessage(async (msg: { type: string; entityId?: string; file?: string; line?: number; markdown?: string; label?: string; command?: string; value?: unknown }) => {
      if (msg?.type === 'ready') {
        webviewView.webview.postMessage({
          type: 'rows',
          rows: this.lastRows,
          summary: this.lastSummary,
          analysisScope: this.lastPayload?.analysisScope ?? 'scope',
          availableScopes: this.lastPayload?.availableScopes ?? ['scope'],
          currentFile: this.lastPayload?.currentFile,
          sortBy: this.lastPayload?.sortBy ?? 'score',
        });
      } else if (msg?.type === 'command' && msg.command) {
        // The scope-picker dropdown sends its changes via this channel.
        // Forward to the same internal command endpoint the Filters /
        // View Options webviews use so it lands in the main panel's
        // Svelte onCommand handler.
        vscode.commands.executeCommand('nao.internalFilterCommand', {
          command: msg.command,
          value: msg.value,
        });
      } else if (msg?.type === 'selectEntity' && msg.entityId) {
        this.selectEntityHandler?.(msg.entityId);
      } else if (msg?.type === 'goToSource' && msg.file && typeof msg.line === 'number') {
        this.goToSourceHandler?.(msg.file, msg.line);
      } else if (msg?.type === 'copyMetrics' && typeof msg.markdown === 'string') {
        await vscode.env.clipboard.writeText(msg.markdown);
        const what = msg.label === 'legend' ? 'Metrics glossary' : 'Metrics';
        vscode.window.setStatusBarMessage(`$(clippy) ${what} copied to clipboard`, 2000);
      } else if (msg?.type === 'spawnAgent' && msg.entityId) {
        await this.spawnAgent(msg.entityId, msg.label ?? msg.entityId);
      }
    });
  }

  private getHtml(): string {
    return /* html */ `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<style>
  body { font-family: var(--vscode-font-family); font-size: var(--vscode-font-size); color: var(--vscode-foreground); margin: 0; padding: 0; }
  .empty { color: var(--vscode-descriptionForeground); font-style: italic; padding: 12px; }
  .header { display: flex; justify-content: space-between; align-items: center; padding: 6px 10px; border-bottom: 1px solid var(--vscode-panel-border); font-size: 0.75em; text-transform: uppercase; letter-spacing: 0.05em; color: var(--vscode-descriptionForeground); position: sticky; top: 0; background: var(--vscode-sideBar-background); }
  .rows { overflow-y: auto; }
  .row {
    display: grid;
    grid-template-columns: 36px 1fr auto;
    gap: 8px;
    align-items: center;
    padding: 4px 10px;
    cursor: pointer;
    border-bottom: 1px solid var(--vscode-panel-border);
  }
  .row:hover { background: var(--vscode-list-hoverBackground); }
  .score {
    font-family: var(--vscode-editor-font-family, monospace);
    font-size: 0.82em;
    text-align: center;
    padding: 1px 4px;
    border-radius: 3px;
    color: var(--vscode-foreground);
  }
  .score.tier-bad   { background: rgba(244, 67, 54, 0.25); color: #f48771; }
  .score.tier-warn  { background: rgba(255, 167, 38, 0.22); color: #e5a650; }
  .score.tier-ok    { background: rgba(76, 175, 80, 0.15); color: #73c990; }
  .name-cell { min-width: 0; display: flex; flex-direction: column; }
  .name { font-weight: 500; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .path { font-size: 0.75em; color: var(--vscode-descriptionForeground); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .kind {
    font-size: 0.7em; text-transform: uppercase;
    padding: 1px 5px; border-radius: 2px;
    background: var(--vscode-badge-background); color: var(--vscode-badge-foreground);
  }
  .metrics { display: flex; flex-wrap: wrap; gap: 3px; margin-top: 3px; }
  .metric {
    font-family: var(--vscode-editor-font-family, monospace);
    font-size: 0.7em;
    padding: 0 4px;
    border-radius: 2px;
    background: var(--vscode-textCodeBlock-background);
    color: var(--vscode-descriptionForeground);
    line-height: 1.4;
    white-space: nowrap;
  }
  .metric.m-bad  { background: rgba(244, 67, 54, 0.18); color: #f48771; }
  .metric.m-warn { background: rgba(255, 167, 38, 0.15); color: #e5a650; }
  .metric.m-ok   { background: rgba(76, 175, 80, 0.10); color: #73c990; }
  .metric.cycle  { background: rgba(244, 67, 54, 0.25); color: #f48771; }
  .smell {
    font-size: 0.7em;
    padding: 0 4px;
    border-radius: 2px;
    background: rgba(244, 67, 54, 0.12);
    color: #f48771;
    font-style: italic;
  }
  /* Informational variant — for soft signals like "Data Class" that
     aren't actual bugs, just hints that a clearer structure might help. */
  .smell.info {
    background: rgba(79, 195, 247, 0.14);
    color: #64b5f6;
  }
  .scope-picker {
    display: flex; align-items: center; gap: 6px;
    padding: 6px 10px;
    border-bottom: 1px solid var(--vscode-panel-border);
    font-size: 0.78em;
  }
  .scope-picker label {
    color: var(--vscode-descriptionForeground);
    text-transform: uppercase; letter-spacing: 0.05em; font-size: 0.85em;
  }
  .scope-picker select {
    flex: 1;
    background: var(--vscode-dropdown-background);
    color: var(--vscode-dropdown-foreground);
    border: 1px solid var(--vscode-dropdown-border);
    border-radius: 3px;
    padding: 2px 4px;
    font-family: inherit;
    font-size: 1em;
  }
  .scope-picker-hint {
    padding: 4px 10px 0;
    font-size: 0.72em;
    color: var(--vscode-descriptionForeground);
  }
  .summary {
    padding: 8px 10px;
    border-bottom: 1px solid var(--vscode-panel-border);
    background: var(--vscode-textCodeBlock-background);
  }
  .summary-head {
    display: flex; align-items: baseline; gap: 8px;
    margin-bottom: 5px;
  }
  .summary-title {
    font-size: 0.72em; text-transform: uppercase; letter-spacing: 0.05em;
    color: var(--vscode-descriptionForeground);
  }
  .summary-pill {
    font-size: 0.75em; padding: 1px 6px; border-radius: 3px;
    font-family: var(--vscode-editor-font-family, monospace);
    font-weight: 600;
  }
  .summary-pill.tier-bad  { background: rgba(244, 67, 54, 0.22); color: #f48771; }
  .summary-pill.tier-warn { background: rgba(255, 167, 38, 0.22); color: #e5a650; }
  .summary-pill.tier-ok   { background: rgba(76, 175, 80, 0.22); color: #73c990; }
  .summary-pill.tier-na   { background: var(--vscode-badge-background); color: var(--vscode-badge-foreground); }
  .summary-line {
    font-size: 0.8em; color: var(--vscode-descriptionForeground);
    margin: 2px 0;
  }
  .summary-line .n { color: var(--vscode-foreground); font-family: var(--vscode-editor-font-family, monospace); }
  .summary-bar {
    display: flex; height: 6px; border-radius: 3px; overflow: hidden;
    margin-top: 6px; background: var(--vscode-panel-border);
  }
  .summary-bar .seg.ok   { background: #73c990; }
  .summary-bar .seg.warn { background: #e5a650; }
  .summary-bar .seg.bad  { background: #f48771; }
  .legend-actions {
    display: flex; justify-content: space-between; align-items: center;
    padding: 4px 10px; border-bottom: 1px solid var(--vscode-panel-border);
    font-size: 0.72em; color: var(--vscode-descriptionForeground);
    text-transform: uppercase; letter-spacing: 0.05em;
  }
  button.legend-btn {
    background: transparent; border: 1px solid var(--vscode-panel-border);
    color: var(--vscode-textLink-foreground); cursor: pointer;
    border-radius: 3px; padding: 2px 8px; font-size: 1em;
    font-family: inherit; text-transform: none; letter-spacing: 0;
  }
  button.legend-btn:hover { background: var(--vscode-list-hoverBackground); }
  button.legend-btn.copied { color: #73c990; border-color: #73c990; }

  /* Floating tooltip for metric badges and summary items */
  #metric-tip {
    position: fixed;
    z-index: 1000;
    max-width: 320px;
    padding: 8px 10px;
    font-size: 0.82em;
    line-height: 1.45;
    background: var(--vscode-editorHoverWidget-background, #252526);
    color: var(--vscode-editorHoverWidget-foreground, #cccccc);
    border: 1px solid var(--vscode-editorHoverWidget-border, #454545);
    border-radius: 4px;
    box-shadow: 0 4px 10px rgba(0, 0, 0, 0.35);
    pointer-events: none;
    opacity: 0;
    transform: translateY(4px);
    transition: opacity 0.12s, transform 0.12s;
  }
  #metric-tip.show {
    opacity: 1;
    transform: translateY(0);
  }
  #metric-tip .tip-title {
    font-weight: 600;
    color: var(--vscode-foreground);
    margin-bottom: 4px;
    font-size: 1.05em;
  }
  #metric-tip .tip-body {
    color: var(--vscode-descriptionForeground);
  }
  [data-tip] { cursor: help; }
  .ghost {
    font-size: 0.7em;
    padding: 0 4px;
    border-radius: 2px;
    background: rgba(156, 120, 255, 0.18);
    color: #b39ddb;
  }
  .goto-btn, .copy-btn, .spawn-btn {
    background: transparent;
    border: 1px solid var(--vscode-panel-border);
    border-radius: 3px;
    color: var(--vscode-textLink-foreground);
    cursor: pointer; padding: 0 5px; margin-left: 3px;
    font-family: inherit; font-size: 0.85em; line-height: 1.6;
    white-space: nowrap;
  }
  /* Launching an agent spends money and edits files. It should not look
     like the two buttons that copy text to the clipboard. */
  .spawn-btn { font-weight: 600; border-color: var(--vscode-focusBorder); }
  .goto-btn:hover, .copy-btn:hover, .spawn-btn:hover { color: var(--vscode-textLink-activeForeground); }
  /* The one action that starts work rather than copying text. */
  .spawn-btn { color: var(--vscode-charts-purple, #c586c0); }
  .copy-btn.copied { color: #73c990; }
</style>
</head>
<body>
  <div id="root">
    <div class="empty">No quality data yet — select a scope in the Scopes view.</div>
  </div>
  <div id="metric-tip"><div class="tip-title"></div><div class="tip-body"></div></div>
<script>
  const vscode = acquireVsCodeApi();
  const root = document.getElementById('root');
  const tip = document.getElementById('metric-tip');
  const tipTitle = tip.querySelector('.tip-title');
  const tipBody = tip.querySelector('.tip-body');

  const METRIC_EXPLANATIONS = {
    score: {
      title: 'Composite score',
      body: 'Weighted sum of CC, Cognitive, Nest, Fan-out, LOC, params, plus a cycle bonus. ~0 = healthy, ~1 = at red threshold, >1 = over the line. Use as a refactor-pressure ranking, not a verdict.',
    },
    cc: {
      title: 'Cyclomatic complexity (CC)',
      body: 'Independent execution paths through a function (branches + 1). Every if / match-arm / loop / ? / && / || adds one. \u2264 10 healthy, \u2264 20 worth a look, > 20 means split the function.',
    },
    cognitive: {
      title: 'Cognitive complexity',
      body: 'Like CC but weights each branch by its nesting depth. Better measure of "how hard is this to understand." \u2264 8 healthy, \u2264 15 worth a look, > 15 flatten with early returns or extract helpers.',
    },
    nest: {
      title: 'Max nesting depth',
      body: 'Deepest level of nested control flow. Deep nesting forces the reader to hold many conditions in mind. \u2264 3 healthy, \u2264 5 amber, > 5 means early-returns or extracted helpers would help.',
    },
    loc: {
      title: 'Lines of code',
      body: 'Inclusive line span of the entity. Weakest signal on its own \u2014 a long but flat function can be fine. Pair with CC and Fan-out before reacting. Callable: \u2264 30 / \u2264 60 / > 60.',
    },
    params: {
      title: 'Parameter count',
      body: 'Number of parameters (self excluded). Long lists usually mean a missing abstraction (group related params into a struct) or a function doing too much. \u2264 4 / \u2264 6 / > 6.',
    },
    fanIn: {
      title: 'Fan-in',
      body: 'Distinct entities that depend on this one. High fan-in is healthy for stable utilities \u2014 it means they are reused. Risky only when the code also changes frequently (ripples through all callers).',
    },
    fanOut: {
      title: 'Fan-out',
      body: 'Distinct entities this one depends on. Classic god-object / orchestrator smell when very high. \u2264 7 healthy, \u2264 15 amber, > 15 red. Introduce a narrow seam (trait/facade) so the caller depends on one port instead of many.',
    },
    cycle: {
      title: 'In cycle',
      body: 'This entity participates in a dependency cycle. Cycles make modules impossible to understand in isolation and break layering. Typical fix: dependency inversion \u2014 both sides depend on an interface instead of each other.',
    },
    fieldCount: {
      title: 'Field / variant count',
      body: 'Structs: number of fields (\u2264 8 / \u2264 15 / > 15). Enums: number of variants (\u2264 6 / \u2264 12 / > 12). Very wide types often should be split or traitified.',
    },
    methodCount: {
      title: 'Method count',
      body: 'Methods directly contained in the type or module. Surface-area proxy. High method count + high field count = classic "blob class" pattern. \u2264 15 / \u2264 25 / > 25 \u2014 consider splitting by responsibility.',
    },
    wmc: {
      title: 'Weighted Methods per Class (WMC)',
      body: 'Sum of cyclomatic complexities of every method in the container. 5 simple methods (total CC 15) is very different from 5 gnarly ones (total CC 120). High WMC is the stronger "blob class" signal when combined with high method count.',
    },
    chainDepth: {
      title: 'Chain depth',
      body: 'Longest outbound call-chain from this entity: a.b().c().d().e() = depth ~5. Deep chains are usually Law-of-Demeter violations or procedural orchestration missing an abstraction. Fix by introducing a facade / coordinator, or by moving the method closer to the data it uses (Tell, Don\u2019t Ask).',
    },
    pagerank: {
      title: 'PageRank centrality',
      body: 'Importance in the dependency graph, weighted by the importance of things that depend on this entity. High PageRank = a "keystone" whose breakage would ripple widely. Value shown is \u00D71000 (raw numbers sum to 1.0 across the graph). Prioritise keystones for tests and be careful when changing them.',
    },
    summaryAvg: {
      title: 'Average score',
      body: 'Mean composite score across every entity in the current scope. Gives you a single "how bad is this scope overall" number. Entity-level scores matter more; this is a headline.',
    },
    summaryMax: {
      title: 'Worst score',
      body: 'The highest composite score in scope \u2014 your single most concerning entity. Usually where to start reading.',
    },
    summaryCycles: {
      title: 'Entities in cycles',
      body: 'Count of entities that belong to at least one dependency cycle. Cycles are structural: 1 usually means a layering violation somewhere.',
    },
    external: {
      title: 'External entity',
      body: 'Referenced by the code but not analysed (stdlib, third-party, or out of scope). Only graph metrics (fan-in / fan-out) are available because there is no source to measure.',
    },
  };

  function showTip(key, target) {
    const data = METRIC_EXPLANATIONS[key];
    if (!data) return;
    tipTitle.textContent = data.title;
    tipBody.textContent = data.body;
    tip.classList.add('show');
    const rect = target.getBoundingClientRect();
    // Prefer below; flip above if it would overflow.
    const margin = 6;
    tip.style.visibility = 'hidden';
    tip.style.left = '0px'; tip.style.top = '0px';
    tip.style.visibility = '';
    const tipRect = tip.getBoundingClientRect();
    let left = rect.left;
    let top = rect.bottom + margin;
    if (left + tipRect.width > window.innerWidth - 8) {
      left = Math.max(8, window.innerWidth - tipRect.width - 8);
    }
    if (top + tipRect.height > window.innerHeight - 8) {
      top = rect.top - tipRect.height - margin;
    }
    tip.style.left = left + 'px';
    tip.style.top = top + 'px';
  }
  function hideTip() { tip.classList.remove('show'); }

  function bindTips(container) {
    container.querySelectorAll('[data-tip]').forEach((el) => {
      el.addEventListener('mouseenter', () => showTip(el.getAttribute('data-tip'), el));
      el.addEventListener('mouseleave', hideTip);
    });
  }

  function escape(s) {
    return String(s ?? '').replace(/[&<>"']/g, (c) => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
  }

  function basename(path) {
    const i = path.lastIndexOf('/');
    return i >= 0 ? path.slice(i + 1) : path;
  }

  function metric(label, value, tier, tipKey) {
    if (value === undefined || value === null) return '';
    const cls = tier && tier !== 'na' ? ' m-' + tier : '';
    const t = tipKey ? ' data-tip="' + tipKey + '"' : '';
    return '<span class="metric' + cls + '"' + t + '>' + label + ' ' + value + '</span>';
  }

  function renderMetrics(r) {
    const m = r.metrics || {};
    const t = r.tiers || {};
    const parts = [];
    if (r.isGhost) parts.push('<span class="ghost" data-tip="external">external</span>');
    parts.push(metric('LOC', m.loc, t.loc, 'loc'));
    if (m.cc !== undefined && m.cc !== null) parts.push(metric('CC', m.cc, t.cc, 'cc'));
    if (m.cognitive !== undefined && m.cognitive !== null) parts.push(metric('Cog', m.cognitive, t.cognitive, 'cognitive'));
    if (m.nesting !== undefined && m.nesting !== null) parts.push(metric('Nest', m.nesting, t.nest, 'nest'));
    if (m.params !== undefined && m.params !== null && m.params > 0) parts.push(metric('P', m.params, t.params, 'params'));
    if (m.fieldCount !== undefined && m.fieldCount !== null && m.fieldCount > 0) parts.push(metric('F', m.fieldCount, t.fieldCount, 'fieldCount'));
    if (m.methodCount !== undefined && m.methodCount !== null && m.methodCount > 0) parts.push(metric('M', m.methodCount, t.methodCount, 'methodCount'));
    parts.push(metric('FI', m.fanIn, null, 'fanIn'));
    parts.push(metric('FO', m.fanOut, t.fanOut, 'fanOut'));
    if (m.wmc !== undefined && m.wmc !== null) parts.push(metric('WMC', m.wmc, null, 'wmc'));
    if (m.chainDepth !== undefined && m.chainDepth !== null && m.chainDepth > 0) parts.push(metric('Chain', m.chainDepth, null, 'chainDepth'));
    if (m.pagerank !== undefined && m.pagerank !== null) parts.push(metric('PR', (m.pagerank * 1000).toFixed(1), null, 'pagerank'));
    if (m.inCycle) parts.push('<span class="metric cycle" data-tip="cycle">\u21BB</span>');
    if (Array.isArray(m.smells) && m.smells.length > 0) {
      for (const s of m.smells) {
        // "Data Bag" is an informational hint, not an alarm — render
        // in blue so the user can tell it apart from the red smells.
        const isInfo = /data\s*bag/i.test(s);
        const cls = isInfo ? 'smell info' : 'smell';
        const title = isInfo
          ? 'Data bag: many fields, little behaviour. Consider grouping related fields into nested sub-structs. (Not a god class \u2014 intentional data records like Kotlin \u201Cdata class\u201D or Python \u201C@dataclass\u201D are usually fine.)'
          : 'Code smell';
        parts.push('<span class="' + cls + '" title="' + escape(title) + '">' + escape(s) + '</span>');
      }
    }
    return '<div class="metrics">' + parts.filter(Boolean).join('') + '</div>';
  }

  function renderSummary(s) {
    if (!s || !s.entityCount) return '';
    const total = s.entityCount;
    const okPct = total ? Math.round((s.okCount / total) * 100) : 0;
    const warnPct = total ? Math.round((s.warnCount / total) * 100) : 0;
    const badPct = total ? Math.round((s.badCount / total) * 100) : 0;
    const tierLabel = s.tier === 'bad' ? 'Poor' : s.tier === 'warn' ? 'Fair' : s.tier === 'ok' ? 'Good' : 'N/A';
    return '<div class="summary">'
      + '<div class="summary-head">'
      +   '<span class="summary-title">Scope quality</span>'
      +   '<span class="summary-pill tier-' + s.tier + '">' + tierLabel + '</span>'
      + '</div>'
      + '<div class="summary-line">'
      +   '<span class="n">' + total.toLocaleString() + '</span> entities'
      +   ' \u00B7 avg <span class="n" data-tip="summaryAvg">' + Number(s.avgScore).toFixed(2) + '</span>'
      +   ' \u00B7 max <span class="n" data-tip="summaryMax">' + Number(s.maxScore).toFixed(2) + '</span>'
      + (s.cycleCount ? ' \u00B7 <span class="n" style="color:#f48771" data-tip="summaryCycles">' + s.cycleCount + '</span> in cycles' : '')
      + '</div>'
      + '<div class="summary-line">'
      +   '<span class="n">' + okPct + '%</span> ok'
      +   ' \u00B7 <span class="n">' + warnPct + '%</span> warn'
      +   ' \u00B7 <span class="n">' + badPct + '%</span> bad'
      + '</div>'
      + '<div class="summary-bar">'
      +   '<div class="seg ok" style="width:' + okPct + '%"></div>'
      +   '<div class="seg warn" style="width:' + warnPct + '%"></div>'
      +   '<div class="seg bad" style="width:' + badPct + '%"></div>'
      + '</div>'
      + '</div>';
  }

  function renderScopePicker(mode, available, currentFile) {
    mode = mode || 'scope';
    available = available || ['scope'];
    const options = [
      { value: 'scope',           label: 'Analysis scope',  available: true },
      { value: 'visualScope',     label: 'Visual scope (scoped tree)',    available: available.indexOf('visualScope') !== -1 },
      { value: 'visualSelection', label: 'Visual selection (on-screen)',  available: available.indexOf('visualSelection') !== -1 },
      { value: 'currentFile',     label: currentFile ? 'Current file (' + basename(currentFile) + ')' : 'Current file',  available: available.indexOf('currentFile') !== -1 },
      { value: 'changedFiles',    label: 'Changed files (diff core)', available: available.indexOf('changedFiles') !== -1 },
    ];
    const opts = options
      .map((o) => '<option value="' + o.value + '"' + (mode === o.value ? ' selected' : '') + (o.available ? '' : ' disabled') + '>' + escape(o.label) + (o.available ? '' : ' \u2014 unavailable') + '</option>')
      .join('');
    return '<div class="scope-picker"><label>Analyze</label><select id="scope-select">' + opts + '</select></div>';
  }

  function renderSortPicker(sortBy) {
    sortBy = sortBy || 'score';
    const options = [
      { value: 'score',       label: 'Composite score' },
      { value: 'pagerank',    label: 'PageRank (centrality)' },
      { value: 'cc',          label: 'Cyclomatic complexity' },
      { value: 'cognitive',   label: 'Cognitive complexity' },
      { value: 'nesting',     label: 'Max nesting depth' },
      { value: 'loc',         label: 'Lines of code' },
      { value: 'params',      label: 'Parameter count' },
      { value: 'fanIn',       label: 'Fan-in' },
      { value: 'fanOut',      label: 'Fan-out' },
      { value: 'wmc',         label: 'WMC (weighted methods)' },
      { value: 'chainDepth',  label: 'Chain depth' },
      { value: 'methodCount', label: 'Method count' },
      { value: 'fieldCount',  label: 'Field / variant count' },
    ];
    const opts = options
      .map((o) => '<option value="' + o.value + '"' + (sortBy === o.value ? ' selected' : '') + '>' + escape(o.label) + '</option>')
      .join('');
    return '<div class="scope-picker"><label>Sort by</label><select id="sort-select">' + opts + '</select></div>';
  }

  function render(rows, summary, analysisScope, availableScopes, currentFile, sortBy) {
    hideTip();
    const pickerHtml = renderScopePicker(analysisScope, availableScopes, currentFile)
                     + renderSortPicker(sortBy);
    const summaryHtml = renderSummary(summary);
    if (!rows || rows.length === 0) {
      root.innerHTML = pickerHtml + summaryHtml + '<div class="empty">No quality data in the current analysis scope.</div>';
      bindTips(root);
      bindScopePicker();
      return;
    }
    const parts = [
      pickerHtml,
      summaryHtml,
      '<div class="header"><span>Top ' + rows.length + ' by score</span><span>Score · Name · Kind</span></div>',
      '<div class="legend-actions">'
        + '<span>Metrics glossary</span>'
        + '<button class="legend-btn" id="copy-legend" title="Copy a plain-text glossary of all metrics (useful for LLM context)">\u2398 Copy legend</button>'
        + '</div>',
      '<div class="rows">',
    ];
    for (const r of rows) {
      const tier = 'tier-' + r.tier;
      const scoreFmt = Number(r.score).toFixed(2);
      parts.push(
        '<div class="row" data-id="' + escape(r.id) + '" data-file="' + escape(r.file) + '" data-line="' + r.line + '">',
        '<div class="score ' + tier + '" data-tip="score">' + scoreFmt + '</div>',
        '<div class="name-cell">',
        '<div class="name">' + escape(r.name) + '</div>',
        '<div class="path">' + escape(basename(r.file)) + ':' + r.line + '</div>',
        renderMetrics(r),
        '</div>',
        '<div><span class="kind">' + escape(r.kind) + '</span> ',
        '<button class="spawn-btn" data-action="spawn" title="Launch a Claude Code agent against this entity \u2014 opens a terminal">Refactor</button> ',
        '<button class="copy-btn" data-action="copy" title="Copy this row\u2019s metrics as a Markdown table">Copy</button> ',
        '<button class="goto-btn" data-action="goto" title="Open the source at this line">Source</button></div>',
        '</div>',
      );
    }
    parts.push('</div>');
    root.innerHTML = parts.join('');
    bindTips(root);
    bindScopePicker();

    // Copy-legend button
    const legendBtn = document.getElementById('copy-legend');
    if (legendBtn) {
      legendBtn.addEventListener('click', () => {
        vscode.postMessage({ type: 'copyMetrics', markdown: buildLegendMarkdown(), label: 'legend' });
        legendBtn.classList.add('copied');
        const original = legendBtn.textContent;
        legendBtn.textContent = '\u2713 Copied';
        setTimeout(() => { legendBtn.classList.remove('copied'); legendBtn.textContent = original; }, 1500);
      });
    }

    root.querySelectorAll('.row').forEach((el, idx) => {
      const row = rows[idx];
      const entityId = el.getAttribute('data-id');
      const file = el.getAttribute('data-file');
      const line = Number(el.getAttribute('data-line'));
      // Row body → select the entity in the graph
      el.addEventListener('click', (ev) => {
        if (ev.target instanceof HTMLElement && ['goto', 'copy', 'spawn'].includes(ev.target.dataset.action ?? '')) return;
        vscode.postMessage({ type: 'selectEntity', entityId });
      });
      // "↗" button → open the file at its line
      el.querySelector('.goto-btn')?.addEventListener('click', (ev) => {
        ev.stopPropagation();
        vscode.postMessage({ type: 'goToSource', file, line });
      });
      // "⎘" button → copy metrics as Markdown
      el.querySelector('.spawn-btn')?.addEventListener('click', (ev) => {
        ev.stopPropagation();
        vscode.postMessage({ type: 'spawnAgent', entityId, label: row.name });
      });
      el.querySelector('.copy-btn')?.addEventListener('click', (ev) => {
        ev.stopPropagation();
        const btn = ev.currentTarget;
        vscode.postMessage({ type: 'copyMetrics', markdown: rowToMarkdown(row) });
        btn.classList.add('copied');
        btn.textContent = '\u2713';
        setTimeout(() => { btn.classList.remove('copied'); btn.textContent = '\u2398'; }, 1200);
      });
    });
  }

  function fmt(v) { return v === undefined || v === null ? '\u2014' : String(v); }
  function fmtPct(v) { return v === undefined || v === null ? '\u2014' : Math.round(v * 100) + '%'; }

  /** Build a plain-text glossary of the entity-level metrics. Meant to be
   *  pasted into an LLM context alongside a copied row so the model can
   *  interpret the numbers correctly. Order mirrors the copied table. */
  function buildLegendMarkdown() {
    const entries = [
      ['Composite score',        METRIC_EXPLANATIONS.score.body],
      ['Cyclomatic complexity',  METRIC_EXPLANATIONS.cc.body],
      ['Cognitive complexity',   METRIC_EXPLANATIONS.cognitive.body],
      ['Max nesting depth',      METRIC_EXPLANATIONS.nest.body],
      ['Lines of code',          METRIC_EXPLANATIONS.loc.body],
      ['Parameter count',        METRIC_EXPLANATIONS.params.body],
      ['Fan-in',                 METRIC_EXPLANATIONS.fanIn.body],
      ['Fan-out',                METRIC_EXPLANATIONS.fanOut.body],
      ['Field / variant count',  METRIC_EXPLANATIONS.fieldCount.body],
      ['Method count',           METRIC_EXPLANATIONS.methodCount.body],
      ['WMC',                    METRIC_EXPLANATIONS.wmc.body],
      ['Chain depth',            METRIC_EXPLANATIONS.chainDepth.body],
      ['PageRank (\u00D71000)',  METRIC_EXPLANATIONS.pagerank.body],
      ['Public field ratio',     'Share of fields marked public. Only scored on structs that also have methods. Higher (>50% amber, >80% red) indicates weak encapsulation when the type has behavior.'],
      ['In cycle',               METRIC_EXPLANATIONS.cycle.body],
      ['Code smells',            'Named anti-pattern signals emitted by the backend (e.g. "god_object", "deep_nesting"). One row can carry several.'],
    ];
    const lines = ['# Nao \u2014 Metrics glossary', ''];
    for (const [name, body] of entries) {
      lines.push('- **' + name + '** \u2014 ' + body);
    }
    return lines.join('\\n');
  }

  function rowToMarkdown(r) {
    const m = r.metrics || {};
    const header = [
      'Composite score',
      'Name',
      'Kind',
      'File',
      'Line',
      'Cyclomatic complexity',
      'Cognitive complexity',
      'Max nesting depth',
      'Lines of code',
      'Parameter count',
      'Fan-in',
      'Fan-out',
      'Field / variant count',
      'Method count',
      'WMC',
      'Chain depth',
      'PageRank (\u00D71000)',
      'Public field ratio',
      'In cycle',
      'Code smells',
    ];
    const values = [
      Number(r.score).toFixed(2),
      r.name,
      r.kind,
      r.file,
      String(r.line),
      fmt(m.cc), fmt(m.cognitive), fmt(m.nesting), fmt(m.loc), fmt(m.params),
      fmt(m.fanIn), fmt(m.fanOut), fmt(m.fieldCount), fmt(m.methodCount),
      fmt(m.wmc), fmt(m.chainDepth),
      m.pagerank != null ? (m.pagerank * 1000).toFixed(2) : '\u2014',
      fmtPct(m.publicFieldRatio),
      m.inCycle ? 'yes' : 'no',
      (m.smells && m.smells.length) ? m.smells.join(', ') : '\u2014',
    ];
    const sep = header.map(() => '---');
    return '| ' + header.join(' | ') + ' |\\n'
         + '| ' + sep.join(' | ') + ' |\\n'
         + '| ' + values.join(' | ') + ' |\\n';
  }

  // Event delegation: one listener on the stable root element, so it
  // survives innerHTML replacements on every render. Per-render rebinds
  // were racing against frequent re-renders — the <select>'s change event
  // was firing on a detached element and being lost.
  root.addEventListener('change', (ev) => {
    const t = ev.target;
    if (t && t.tagName === 'SELECT' && t.id === 'scope-select') {
      vscode.postMessage({ type: 'command', command: 'setQualityAnalysisScope', value: t.value });
    } else if (t && t.tagName === 'SELECT' && t.id === 'sort-select') {
      vscode.postMessage({ type: 'command', command: 'setQualitySortBy', value: t.value });
    }
  });
  function bindScopePicker() { /* no-op — delegated above */ }

  window.addEventListener('message', (event) => {
    const msg = event.data;
    if (msg?.type === 'rows') render(msg.rows, msg.summary, msg.analysisScope, msg.availableScopes, msg.currentFile, msg.sortBy);
  });

  // Listener is attached — ask the extension for the latest rows.
  vscode.postMessage({ type: 'ready' });
</script>
</body>
</html>`;
  }
}
