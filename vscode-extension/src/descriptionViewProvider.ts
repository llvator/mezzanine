import * as vscode from 'vscode';

/** One rung of the ancestry chain, resolved in the webview (which owns the
 *  entity graph and the details sidecar) and posted here ready to render. */
export interface DescriptionEntry {
  entityId: string;
  name: string;
  qualifiedName: string;
  kind: string;
  filePath: string;
  line: number;
  documentation: string | null;
  /** 0 for the node itself, 1 for its parent, and so on. */
  depth: number;
}

export interface DescriptionPayload {
  /** Whether the pointer or the selection put this chain on screen. */
  source: 'hover' | 'selection';
  chain: DescriptionEntry[];
}

/**
 * Native "Description" view — the graph read as prose.
 *
 * The sibling of "follow selection to editor": that mode answers "where is
 * this?" by jumping the editor to a node's source, this one answers "what is
 * this for?" by showing the node's description followed by its ancestors'.
 * Skimming the canvas with the pointer narrates the graph without a click.
 *
 * Deliberately its own view rather than a third Inspector tab: the whole
 * point is reading it *while* hovering, and a tab would make it compete with
 * Details for the same sidebar slice.
 */
export class DescriptionViewProvider implements vscode.WebviewViewProvider {
  static readonly viewType = 'mezz.description';

  private view?: vscode.WebviewView;
  /** Replayed on `ready`: VS Code tears the webview down whenever the view
   *  is hidden, so the last chain has to survive outside it. */
  private last?: DescriptionPayload;

  constructor(private readonly extensionUri: vscode.Uri) {}

  update(payload: DescriptionPayload | undefined): void {
    this.last = payload;
    this.view?.webview.postMessage({ type: 'description', payload });
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

    webviewView.webview.onDidReceiveMessage(
      (msg: { type: string; filePath?: string; line?: number }) => {
        if (msg?.type === 'ready') {
          if (this.last) {
            webviewView.webview.postMessage({ type: 'description', payload: this.last });
          }
        } else if (msg?.type === 'goToDefinition' && msg.filePath) {
          vscode.commands.executeCommand('mezz.internalGoToDefinition', {
            filePath: msg.filePath,
            line: msg.line,
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
  body {
    font-family: var(--vscode-font-family); font-size: var(--vscode-font-size);
    color: var(--vscode-foreground); margin: 0; padding: 8px 12px 16px;
  }
  .empty { color: var(--vscode-descriptionForeground); font-style: italic; padding: 12px 0; line-height: 1.5; }
  .source-badge {
    display: inline-block; font-size: 0.7em; text-transform: uppercase; letter-spacing: 0.06em;
    color: var(--vscode-descriptionForeground); margin-bottom: 8px;
  }
  .source-badge .dot { color: var(--vscode-textLink-foreground); }

  /* Each rung dims and shrinks as the chain climbs, so the node under the
     pointer stays the subject and the ancestors read as context. */
  .rung { padding-bottom: 10px; }
  .rung + .rung { border-top: 1px solid var(--vscode-panel-border); padding-top: 10px; }
  .rung.up-1 { opacity: 0.88; }
  .rung.up-2 { opacity: 0.76; }
  .rung.up-3 { opacity: 0.64; }

  .parent-of {
    font-size: 0.68em; text-transform: uppercase; letter-spacing: 0.08em;
    color: var(--vscode-descriptionForeground); margin-bottom: 4px;
  }
  .head { display: flex; align-items: baseline; gap: 6px; flex-wrap: wrap; margin-bottom: 4px; }
  .kind {
    font-size: 0.68em; text-transform: uppercase; padding: 1px 5px; border-radius: 2px;
    background: var(--vscode-badge-background); color: var(--vscode-badge-foreground); flex-shrink: 0;
  }
  .name {
    font-weight: 600; background: none; border: none; padding: 0; cursor: pointer;
    color: var(--vscode-foreground); font-family: inherit; font-size: 1em; text-align: left;
  }
  .name:hover { color: var(--vscode-textLink-activeForeground); text-decoration: underline; }
  .name.static { cursor: default; }
  .name.static:hover { color: var(--vscode-foreground); text-decoration: none; }
  .doc {
    white-space: pre-wrap; overflow-wrap: anywhere; line-height: 1.5;
    margin: 0; font-family: inherit; font-size: 0.92em;
  }
  .doc.missing { color: var(--vscode-descriptionForeground); font-style: italic; }
  .loc { font-size: 0.78em; color: var(--vscode-descriptionForeground); margin-top: 3px; word-break: break-all; }
</style>
</head>
<body>
  <div id="root">
    <div class="empty">Hover a node in the graph to read its description, and its parents'.</div>
  </div>
<script>
  const vscode = acquireVsCodeApi();
  const root = document.getElementById('root');

  function escapeHtml(s) {
    return String(s == null ? '' : s).replace(/[&<>"']/g, (c) => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
  }

  function renderRung(e, i) {
    const clamp = Math.min(e.depth, 3);
    const hasSource = !!e.filePath;
    const doc = e.documentation
      ? '<p class="doc">' + escapeHtml(e.documentation) + '</p>'
      : '<p class="doc missing">No description.</p>';
    return '<div class="rung up-' + clamp + '">'
      + (i > 0 ? '<div class="parent-of">parent</div>' : '')
      + '<div class="head">'
      + '<span class="kind">' + escapeHtml(e.kind) + '</span>'
      + '<button class="name' + (hasSource ? '' : ' static') + '" data-i="' + i + '"'
      + ' title="' + escapeHtml(e.qualifiedName || e.name) + '"'
      + (hasSource ? '' : ' disabled') + '>' + escapeHtml(e.name) + '</button>'
      + '</div>'
      + doc
      + (hasSource ? '<div class="loc">' + escapeHtml(e.filePath) + ':' + e.line + '</div>' : '')
      + '</div>';
  }

  function render(payload) {
    if (!payload || !payload.chain || payload.chain.length === 0) {
      root.innerHTML = '<div class="empty">Hover a node in the graph to read its description, and its parents\\'.</div>';
      return;
    }
    const chain = payload.chain;
    const badge = payload.source === 'hover'
      ? '<span class="dot">●</span> Hovering'
      : '<span class="dot">●</span> Selected';
    root.innerHTML = '<div class="source-badge">' + badge + '</div>'
      + chain.map(renderRung).join('');

    root.querySelectorAll('button.name').forEach((btn) => {
      btn.addEventListener('click', () => {
        const e = chain[Number(btn.getAttribute('data-i'))];
        if (e && e.filePath) {
          vscode.postMessage({ type: 'goToDefinition', filePath: e.filePath, line: e.line });
        }
      });
    });
  }

  window.addEventListener('message', (event) => {
    if (event.data?.type === 'description') render(event.data.payload);
  });

  vscode.postMessage({ type: 'ready' });
</script>
</body>
</html>`;
  }
}
