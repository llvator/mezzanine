import * as vscode from 'vscode';
import * as http from 'http';

export interface SelectionPayload {
  entityId: string;
  originalId: string;
  name: string;
  qualifiedName?: string;
  kind: string;
  filePath: string;
  line: number;
  endLine: number;
  language?: string;
  sourceCode?: string | null;
  parameters?: string[];
  returnType?: string | null;
  metrics?: Record<string, unknown>;
}

/**
 * Native "Inspector" webview — a single view with two tabs that used to be two
 * separate views (each cramped by VS Code's fixed sidebar height):
 *
 *   • Details — the old "Selection": name/kind/location/metrics/source for the
 *     entity currently selected in the main visualizer, plus Go-to-source and
 *     Drill-in actions.
 *   • Context — the old "Context": pick a mode/depth and copy one of four
 *     pre-built context payloads (paths, ranges, entity source, full files),
 *     served by the watch server's /api/scope endpoint.
 *
 * Both tabs track the same selection; consolidating them means one view owns
 * the sidebar slice so each tab gets the full height instead of half of it.
 */
export class SelectionViewProvider implements vscode.WebviewViewProvider {
  static readonly viewType = 'nao.selection';

  private view?: vscode.WebviewView;
  private lastSelection?: SelectionPayload;
  private serverPort = 3200;
  /** Context tab: cache keyed by `${entityId}:${mode}:${depth}` → scope
   *  response, so stats show immediately and copy doesn't re-fetch. */
  private cache = new Map<string, ScopeResponse>();

  constructor(private readonly extensionUri: vscode.Uri) {}

  setServerPort(port: number): void {
    this.serverPort = port;
    // If the view is already resolved, tell the Details tab the new port.
    this.view?.webview.postMessage({ type: 'config', apiBase: `http://localhost:${port}` });
  }

  updateSelection(payload: SelectionPayload | undefined): void {
    this.lastSelection = payload;
    // New entity → drop the Context cache so stats reflect the new subject.
    this.cache.clear();
    this.view?.webview.postMessage({ type: 'selection', payload });
  }

  resolveWebviewView(
    webviewView: vscode.WebviewView,
    _context: vscode.WebviewViewResolveContext,
    _token: vscode.CancellationToken
  ): void {
    this.view = webviewView;
    webviewView.webview.options = {
      enableScripts: true,
      localResourceRoots: [this.extensionUri],
    };
    webviewView.webview.html = this.getHtml();

    // The webview posts { type: 'ready' } once its message listener is
    // attached; we respond with the current state to avoid a race where
    // messages sent immediately after .html land before the listener exists.
    webviewView.webview.onDidReceiveMessage(async (msg: {
      type: string;
      filePath?: string;
      line?: number;
      path?: string;
      mode?: string;
      depth?: number;
      key?: string;
    }) => {
      if (msg?.type === 'ready') {
        webviewView.webview.postMessage({
          type: 'config',
          apiBase: `http://localhost:${this.serverPort}`,
        });
        if (this.lastSelection) {
          webviewView.webview.postMessage({ type: 'selection', payload: this.lastSelection });
        }
      } else if (msg?.type === 'goToDefinition' && msg.filePath) {
        vscode.commands.executeCommand('nao.internalGoToDefinition', {
          filePath: msg.filePath,
          line: msg.line,
        });
      } else if (msg?.type === 'drillIn' && typeof msg.path === 'string') {
        console.log('[drill] SelectionViewProvider received drillIn path=' + msg.path + ' (handlers=' + SelectionViewProvider.drillInHandlers.length + ')');
        for (const handler of SelectionViewProvider.drillInHandlers) {
          handler(msg.path);
        }
      } else if (msg?.type === 'requestStats') {
        await this.handleRequestStats(msg);
      } else if (msg?.type === 'fetchAndCopy') {
        await this.handleFetchAndCopy(msg);
      }
    });
  }

  // ---- Context tab: server-backed scope fetching ----

  private async getScope(mode: string, depth: number): Promise<ScopeResponse | undefined> {
    if (!this.lastSelection?.originalId) return undefined;
    const key = `${this.lastSelection.originalId}:${mode}:${depth}`;
    const cached = this.cache.get(key);
    if (cached) return cached;
    const data = await fetchScope(this.serverPort, {
      entity_id: this.lastSelection.originalId,
      mode,
      depth,
      excluded_files: [],
    });
    this.cache.set(key, data);
    return data;
  }

  private async handleRequestStats(msg: { mode?: string; depth?: number }): Promise<void> {
    const mode = msg.mode ?? 'manual';
    const depth = msg.depth ?? 1;
    try {
      const data = await this.getScope(mode, depth);
      if (!data) return;
      this.view?.webview.postMessage({
        type: 'stats',
        mode,
        depth,
        entities: data.entities?.length ?? 0,
        entityTokens: data.token_count_entities ?? 0,
        fileTokens: data.token_count_files ?? 0,
      });
    } catch (err) {
      this.view?.webview.postMessage({
        type: 'statsError',
        mode,
        depth,
        message: (err as Error).message,
      });
    }
  }

  private async handleFetchAndCopy(msg: { mode?: string; depth?: number; key?: string }): Promise<void> {
    if (!this.lastSelection?.originalId || !msg.key) return;
    const mode = msg.mode ?? 'manual';
    const depth = msg.depth ?? 1;
    try {
      const data = await this.getScope(mode, depth);
      if (!data) return;
      const text = (data.exports as Record<string, string>)[msg.key];
      if (!text) {
        vscode.window.showWarningMessage(`Nao: no ${msg.key} context available for this entity.`);
        return;
      }
      await vscode.env.clipboard.writeText(text);
      const entityTok = data.token_count_entities ?? 0;
      const fileTok = data.token_count_files ?? 0;
      this.view?.webview.postMessage({
        type: 'copied',
        key: msg.key,
        entities: data.entities?.length ?? 0,
        entityTokens: entityTok,
        fileTokens: fileTok,
      });
      vscode.window.setStatusBarMessage(
        `$(clippy) Copied ${msg.key} context — ${data.entities?.length ?? 0} entities, ~${entityTok} + ~${fileTok} tokens`,
        3000
      );
    } catch (err) {
      vscode.window.showErrorMessage(`Nao: context fetch failed — ${(err as Error).message}`);
    }
  }

  /** Handlers the extension registers so drill-in actions (fired from the
   *  Inspector's "Drill in" button) can be forwarded to the main
   *  visualizer panel. */
  private static readonly drillInHandlers: Array<(path: string) => void> = [];

  static onDrillIn(handler: (path: string) => void): vscode.Disposable {
    SelectionViewProvider.drillInHandlers.push(handler);
    return {
      dispose: () => {
        const i = SelectionViewProvider.drillInHandlers.indexOf(handler);
        if (i >= 0) SelectionViewProvider.drillInHandlers.splice(i, 1);
      },
    };
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
  #pane-details { padding: 8px 12px; }
  #pane-context { padding: 8px 10px; }

  .empty { color: var(--vscode-descriptionForeground); font-style: italic; padding: 12px 0; }
  .section { margin-top: 12px; }
  .section-title { font-size: 0.75em; text-transform: uppercase; color: var(--vscode-descriptionForeground); margin-bottom: 4px; letter-spacing: 0.05em; }

  /* ---- Details ---- */
  .entity-name { font-size: 1.1em; font-weight: 600; color: var(--vscode-symbolIcon-classForeground, var(--vscode-foreground)); margin-bottom: 4px; }
  .entity-kind { display: inline-block; font-size: 0.75em; text-transform: uppercase; padding: 2px 6px; border-radius: 3px; background: var(--vscode-badge-background); color: var(--vscode-badge-foreground); margin-right: 6px; }
  .entity-location { font-size: 0.85em; color: var(--vscode-descriptionForeground); margin-bottom: 8px; word-break: break-all; }
  .entity-qualified { font-family: var(--vscode-editor-font-family, monospace); font-size: 0.85em; color: var(--vscode-textLink-foreground); margin-bottom: 8px; }
  .metrics { display: grid; grid-template-columns: auto 1fr; gap: 2px 12px; font-size: 0.85em; }
  .metric-key { color: var(--vscode-descriptionForeground); }
  .metric-val { font-family: var(--vscode-editor-font-family, monospace); }
  .source {
    background: var(--vscode-textCodeBlock-background);
    border: 1px solid var(--vscode-panel-border);
    border-radius: 3px; padding: 8px;
    font-family: var(--vscode-editor-font-family, monospace); font-size: 0.82em;
    overflow-x: auto; white-space: pre; max-height: 300px; overflow-y: auto;
    color: var(--vscode-editor-foreground);
  }
  .goto-btn, .drill-btn {
    display: inline-block; padding: 4px 10px; font-size: 0.85em;
    background: var(--vscode-button-background); color: var(--vscode-button-foreground);
    border: none; border-radius: 3px; cursor: pointer; margin-top: 4px; margin-right: 6px;
  }
  .goto-btn:hover, .drill-btn:hover { background: var(--vscode-button-hoverBackground); }
  .drill-btn {
    background: var(--vscode-button-secondaryBackground, var(--vscode-button-background));
    color: var(--vscode-button-secondaryForeground, var(--vscode-button-foreground));
  }

  /* ---- Context ---- */
  .entity { margin-bottom: 10px; }
  #pane-context .entity-name { font-size: 1em; font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  #pane-context .entity-kind {
    display: inline-block; font-size: 0.7em; text-transform: uppercase;
    padding: 1px 5px; border-radius: 2px;
    background: var(--vscode-badge-background); color: var(--vscode-badge-foreground); margin-right: 4px;
  }
  #pane-context .section { margin-top: 0; margin-bottom: 10px; }
  #pane-context .section-title { font-size: 0.7em; margin-bottom: 3px; }
  .segmented { display: inline-flex; border: 1px solid var(--vscode-panel-border); border-radius: 3px; overflow: hidden; width: 100%; }
  .segmented button { flex: 1; padding: 3px 4px; background: transparent; color: var(--vscode-foreground); border: none; border-right: 1px solid var(--vscode-panel-border); cursor: pointer; font-size: 0.85em; }
  .segmented button:last-child { border-right: none; }
  .segmented button:hover { background: var(--vscode-list-hoverBackground); }
  .segmented button.active { background: var(--vscode-button-background); color: var(--vscode-button-foreground); }
  .copy-grid { display: grid; grid-template-columns: 1fr 1fr; gap: 4px; }
  button.copy {
    padding: 5px 8px; background: transparent; color: var(--vscode-foreground);
    border: 1px solid var(--vscode-panel-border); border-radius: 3px; cursor: pointer;
    font-size: 0.85em; text-align: center;
  }
  button.copy:hover { background: var(--vscode-list-hoverBackground); }
  button.copy.accent { background: var(--vscode-button-background); color: var(--vscode-button-foreground); border-color: transparent; }
  button.copy.accent:hover { background: var(--vscode-button-hoverBackground); }
  button.copy.copied { color: #73c990; border-color: #73c990; }
  button.copy:disabled { opacity: 0.5; cursor: not-allowed; }
  .stats { font-size: 0.75em; color: var(--vscode-descriptionForeground); margin-top: 6px; min-height: 1em; }
  .preview-stats {
    font-size: 0.78em; color: var(--vscode-descriptionForeground);
    padding: 4px 6px; margin-bottom: 6px;
    background: var(--vscode-textCodeBlock-background); border-radius: 3px; min-height: 1.2em;
  }
  .preview-stats .n { color: var(--vscode-foreground); font-family: var(--vscode-editor-font-family, monospace); }
  .preview-stats.loading { opacity: 0.6; }
  .preview-stats.error { color: #f48771; }
</style>
</head>
<body>
  <div class="tab-bar">
    <button class="tab" data-tab="details">Details</button>
    <button class="tab" data-tab="context">Context</button>
  </div>

  <div class="pane" id="pane-details">
    <div id="details-root">
      <div class="empty">Select a node in the graph to see its details here.</div>
    </div>
  </div>

  <div class="pane" id="pane-context">
    <div id="context-root">
      <div class="empty">Select a node in the graph to build its context.</div>
    </div>
  </div>

<script>
  const vscode = acquireVsCodeApi();
  const prev = vscode.getState() || {};

  function escapeHtml(s) {
    return String(s == null ? '' : s).replace(/[&<>"']/g, (c) => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
  }

  // ---- Tabs ----
  let activeTab = prev.activeTab || 'details';
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

  // ================= Details =================
  const Details = (function () {
    const root = document.getElementById('details-root');

    function renderMetrics(m) {
      if (!m) return '';
      const rows = Object.entries(m)
        .filter(([, v]) => v !== null && v !== undefined && v !== false)
        .map(([k, v]) => '<div class="metric-key">' + escapeHtml(k) + '</div><div class="metric-val">' + escapeHtml(typeof v === 'number' ? Number(v).toFixed(2).replace(/\\.00$/, '') : v) + '</div>')
        .join('');
      if (!rows) return '';
      return '<div class="section"><div class="section-title">Metrics</div><div class="metrics">' + rows + '</div></div>';
    }

    function render(p) {
      if (!p) {
        root.innerHTML = '<div class="empty">Select a node in the graph to see its details here.</div>';
        return;
      }
      const locEnd = p.endLine && p.endLine !== p.line ? '-' + p.endLine : '';
      const sourceSection = p.sourceCode
        ? '<div class="section"><div class="section-title">Source</div><pre class="source">' + escapeHtml(p.sourceCode) + '</pre></div>'
        : '';
      const canDrill = p.kind === 'file' || p.kind === 'module';
      const drillButton = canDrill
        ? '<button class="drill-btn" id="drill" title="Narrow the scope to this ' + escapeHtml(p.kind) + ' and show its entities">Drill in ↓</button>'
        : '';
      root.innerHTML =
        '<div class="entity-name">' + escapeHtml(p.name) + '</div>'
        + '<div><span class="entity-kind">' + escapeHtml(p.kind) + '</span>'
        + (p.language ? '<span class="entity-kind">' + escapeHtml(p.language) + '</span>' : '') + '</div>'
        + (p.qualifiedName && p.qualifiedName !== p.name ? '<div class="entity-qualified">' + escapeHtml(p.qualifiedName) + '</div>' : '')
        + '<div class="entity-location">' + escapeHtml(p.filePath) + ':' + p.line + locEnd + '</div>'
        + '<button class="goto-btn" id="goto">Go to Source</button>'
        + drillButton
        + renderMetrics(p.metrics)
        + sourceSection;
      document.getElementById('goto')?.addEventListener('click', () => {
        vscode.postMessage({ type: 'goToDefinition', filePath: p.filePath, line: p.line });
      });
      if (canDrill) {
        document.getElementById('drill')?.addEventListener('click', () => {
          console.log('[drill] Inspector "Drill in" clicked path=' + p.originalId);
          vscode.postMessage({ type: 'drillIn', path: p.originalId });
        });
      }
    }

    return { render };
  })();

  // ================= Context =================
  const Context = (function () {
    const root = document.getElementById('context-root');
    let selection = null;
    let mode = 'manual';
    let depth = 1;
    let lastStats = '';
    let previewState = { status: 'idle', entities: 0, entityTokens: 0, fileTokens: 0, error: '' };
    let requestTimer = null;

    function renderPreviewText() {
      if (previewState.status === 'loading') return 'Calculating…';
      if (previewState.status === 'error') return 'Error: ' + escapeHtml(previewState.error || 'unknown');
      if (previewState.status === 'idle') return '—';
      return '<span class="n">' + previewState.entities + '</span> entities · '
           + '~<span class="n">' + previewState.entityTokens.toLocaleString() + '</span> tok entities · '
           + '~<span class="n">' + previewState.fileTokens.toLocaleString() + '</span> tok files';
    }

    function requestPreview() {
      if (!selection) return;
      previewState = { ...previewState, status: 'loading' };
      const el = document.getElementById('preview-stats');
      if (el) { el.className = 'preview-stats loading'; el.innerHTML = renderPreviewText(); }
      clearTimeout(requestTimer);
      requestTimer = setTimeout(() => {
        vscode.postMessage({ type: 'requestStats', mode, depth });
      }, 150);
    }

    function render() {
      if (!selection) {
        root.innerHTML = '<div class="empty">Select a node in the graph to build its context.</div>';
        return;
      }
      const depthRow = mode === 'manual'
        ? '<div class="section"><div class="section-title">Depth</div>'
          + '<div class="segmented" data-group="depth">'
          + [0, 1, 2, 3].map((d) => '<button data-value="' + d + '"' + (depth === d ? ' class="active"' : '') + '>' + d + '</button>').join('')
          + '</div></div>'
        : '';
      root.innerHTML =
        '<div class="entity">'
        + '<div><span class="entity-kind">' + escapeHtml(selection.kind) + '</span></div>'
        + '<div class="entity-name" title="' + escapeHtml(selection.qualifiedName || selection.name) + '">' + escapeHtml(selection.name) + '</div>'
        + '</div>'
        + '<div class="section"><div class="section-title">Mode</div>'
        + '<div class="segmented" data-group="mode">'
        + ['manual', 'refactor', 'understand'].map((m) =>
            '<button data-value="' + m + '"' + (mode === m ? ' class="active"' : '') + '>' + m.charAt(0).toUpperCase() + m.slice(1) + '</button>'
          ).join('')
        + '</div></div>'
        + depthRow
        + '<div class="section"><div class="section-title">Scope preview</div>'
        + '<div class="preview-stats ' + (previewState.status === 'loading' ? 'loading' : previewState.status === 'error' ? 'error' : '') + '" id="preview-stats">'
        + renderPreviewText()
        + '</div></div>'
        + '<div class="section"><div class="section-title">Copy context</div>'
        + '<div class="copy-grid">'
        + '<button class="copy" data-key="paths" title="Just the file paths (one per line)">Paths</button>'
        + '<button class="copy" data-key="ranges" title="Paths with line ranges per entity">Ranges</button>'
        + '<button class="copy" data-key="entity_context" title="Source code of each entity in scope">Entity Context</button>'
        + '<button class="copy accent" data-key="full_files" title="The complete contents of every file in scope">Full Files</button>'
        + '</div></div>'
        + '<div class="stats" id="stats">' + lastStats + '</div>';

      root.querySelectorAll('.segmented button').forEach((btn) => {
        btn.addEventListener('click', () => {
          const group = btn.parentElement.getAttribute('data-group');
          const value = btn.getAttribute('data-value');
          if (group === 'mode') mode = value;
          else if (group === 'depth') depth = Number(value);
          render();
          requestPreview();
        });
      });

      root.querySelectorAll('button.copy').forEach((btn) => {
        btn.addEventListener('click', () => {
          const key = btn.getAttribute('data-key');
          btn.disabled = true;
          btn.textContent = 'Fetching…';
          vscode.postMessage({ type: 'fetchAndCopy', key, mode, depth });
        });
      });
    }

    function onSelection(payload) {
      selection = payload || null;
      lastStats = '';
      previewState = { status: 'idle', entities: 0, entityTokens: 0, fileTokens: 0, error: '' };
      render();
      requestPreview();
    }

    function onStats(msg) {
      if (msg.mode === mode && msg.depth === depth) {
        previewState = { status: 'ready', entities: msg.entities, entityTokens: msg.entityTokens, fileTokens: msg.fileTokens, error: '' };
        const el = document.getElementById('preview-stats');
        if (el) { el.className = 'preview-stats'; el.innerHTML = renderPreviewText(); }
      }
    }

    function onStatsError(msg) {
      if (msg.mode === mode && msg.depth === depth) {
        previewState = { status: 'error', entities: 0, entityTokens: 0, fileTokens: 0, error: msg.message || 'failed' };
        const el = document.getElementById('preview-stats');
        if (el) { el.className = 'preview-stats error'; el.innerHTML = renderPreviewText(); }
      }
    }

    function onCopied(msg) {
      const labels = { paths: 'Paths', ranges: 'Ranges', entity_context: 'Entity Context', full_files: 'Full Files' };
      root.querySelectorAll('button.copy').forEach((btn) => {
        btn.disabled = false;
        const key = btn.getAttribute('data-key');
        const label = labels[key] || key;
        if (key === msg.key) {
          btn.textContent = '✓ Copied';
          btn.classList.add('copied');
          setTimeout(() => { btn.textContent = label; btn.classList.remove('copied'); }, 1500);
        } else {
          btn.textContent = label;
        }
      });
      lastStats = msg.entities + ' entities · ~' + msg.entityTokens + ' tok entities · ~' + msg.fileTokens + ' tok files';
      const statsEl = document.getElementById('stats');
      if (statsEl) statsEl.textContent = lastStats;
    }

    return { onSelection, onStats, onStatsError, onCopied };
  })();

  // ---- Inbound ----
  window.addEventListener('message', (event) => {
    const msg = event.data;
    if (msg?.type === 'selection') {
      Details.render(msg.payload);
      Context.onSelection(msg.payload);
    } else if (msg?.type === 'config') {
      // Details tab currently only needs the port for future use.
    } else if (msg?.type === 'stats') {
      Context.onStats(msg);
    } else if (msg?.type === 'statsError') {
      Context.onStatsError(msg);
    } else if (msg?.type === 'copied') {
      Context.onCopied(msg);
    }
  });

  vscode.postMessage({ type: 'ready' });
</script>
</body>
</html>`;
  }
}

interface ScopeRequest {
  entity_id: string;
  mode: string;
  depth: number;
  excluded_files: string[];
}

interface ScopeResponse {
  entities: { id: string }[];
  token_count_entities: number;
  token_count_files: number;
  exports: Record<string, string>;
}

function fetchScope(port: number, body: ScopeRequest): Promise<ScopeResponse> {
  return new Promise((resolve, reject) => {
    const payload = JSON.stringify(body);
    const req = http.request(
      {
        host: '127.0.0.1',
        port,
        path: '/api/scope',
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          'Content-Length': Buffer.byteLength(payload).toString(),
        },
        timeout: 15000,
      },
      (res) => {
        let data = '';
        res.setEncoding('utf-8');
        res.on('data', (chunk) => (data += chunk));
        res.on('end', () => {
          if (res.statusCode !== 200) {
            reject(new Error(`HTTP ${res.statusCode}: ${data.slice(0, 200)}`));
            return;
          }
          try {
            resolve(JSON.parse(data));
          } catch (e) {
            reject(e);
          }
        });
      }
    );
    req.on('error', reject);
    req.on('timeout', () => { req.destroy(); reject(new Error('timeout')); });
    req.write(payload);
    req.end();
  });
}
