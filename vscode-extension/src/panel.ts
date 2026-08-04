import * as vscode from 'vscode';
import * as path from 'path';
import * as fs from 'fs';
import type { SelectionPayload } from './sideViewProvider';
import type { DescriptionPayload } from './descriptionViewProvider';

interface GoToDefinitionLocation {
  filePath: string;
  line?: number;
}

type GoToDefinitionHandler = (location: GoToDefinitionLocation) => void;
type SelectionHandler = (payload: SelectionPayload | undefined) => void;
type DescriptionHandler = (payload: DescriptionPayload | undefined) => void;

export type MetricTier = 'ok' | 'warn' | 'bad' | 'na';

export interface QualityItem {
  id: string;
  name: string;
  kind: string;
  file: string;
  line: number;
  score: number;
  tier: 'ok' | 'warn' | 'bad';
  /** True for entities referenced but not analyzed (stdlib / external crates
   *  / files outside the scope). Most metrics are unavailable for these. */
  isGhost?: boolean;
  metrics: {
    loc: number;
    cc?: number;
    cognitive?: number;
    nesting?: number;
    params?: number;
    fanIn: number;
    fanOut: number;
    fieldCount?: number;
    methodCount?: number;
    wmc?: number;
    chainDepth?: number;
    pagerank?: number;
    inCycle: boolean;
    smells?: string[];
  };
  tiers: {
    cc: MetricTier;
    cognitive: MetricTier;
    nest: MetricTier;
    loc: MetricTier;
    params: MetricTier;
    fanOut: MetricTier;
    fieldCount: MetricTier;
    methodCount: MetricTier;
  };
}
export interface QualitySummary {
  entityCount: number;
  avgScore: number;
  maxScore: number;
  okCount: number;
  warnCount: number;
  badCount: number;
  cycleCount: number;
  badRatio: number;
  tier: 'ok' | 'warn' | 'bad' | 'na';
}
export type QualityAnalysisScope = 'scope' | 'visualScope' | 'visualSelection' | 'currentFile' | 'changedFiles';
export type QualitySortKey =
  | 'score' | 'pagerank' | 'cc' | 'cognitive' | 'nesting' | 'loc'
  | 'params' | 'fanIn' | 'fanOut' | 'wmc' | 'chainDepth'
  | 'methodCount' | 'fieldCount';

export interface QualityPayload {
  summary: QualitySummary;
  rows: QualityItem[];
  /** Which subset of the visual scope the Quality view is currently
   *  analyzing. Defaults to 'scope' (the full visual scope). */
  analysisScope: QualityAnalysisScope;
  /** The scopes whose prerequisites are satisfied — e.g. 'currentFile'
   *  is only meaningful when the user has an editor focused, and
   *  'changedFiles' only when a diff is loaded. */
  availableScopes: QualityAnalysisScope[];
  /** Workspace-relative path of the file currently focused, if any.
   *  Used in the dropdown label so the user knows what 'Current file'
   *  actually means right now. */
  currentFile?: string | null;
  /** Which metric the rows are currently ranked by (descending). */
  sortBy: QualitySortKey;
}
type QualityHandler = (payload: QualityPayload) => void;

export interface FilterState {
  entityTypes: { name: string; enabled: boolean }[];
  relTypes: { name: string; enabled: boolean }[];
  directions: { outgoing: boolean; incoming: boolean };
  languages: { name: string; enabled: boolean }[];
  /** "Ghost nodes (external refs)" master toggle — hides every ghost
   *  when off. Mirrors the webview's inline filter panel. */
  showGhosts: boolean;
  /** Sub-toggle for `ghost_stdlib`-tagged ghosts (Python builtins,
   *  Rust Vec/HashMap, JS console, …) independent of the master
   *  ghost toggle. Off by default. */
  showBuiltinGhosts: boolean;
}
type FilterHandler = (state: FilterState) => void;

export type TriStateValue = 'on' | 'off' | 'general';

export interface LevelFilterState {
  allEntityTypes: string[];
  allRelTypes: string[];
  levels: Record<number, {
    enabled: boolean;
    peerEdges: boolean;
    entityTypes: Record<string, TriStateValue>;
    relTypes: Record<string, TriStateValue>;
    outgoing: TriStateValue;
    incoming: TriStateValue;
  }>;
  showDirectEdges: boolean;
  showCrossLevelEdges: boolean;
}
type LevelFilterHandler = (state: LevelFilterState) => void;

export interface DiffState {
  active: boolean;
  fromRef?: string;
  toRef?: string;
  summary?: {
    added: number;
    removed: number;
    modified: number;
    modifiedSource?: number;
    modifiedImpact?: number;
    unchanged: number;
  };
  changesOnly: boolean;
  coreOnly: boolean;
  dimOpacity: number;
  computing: boolean;
  error?: string | null;
  /** Whether any scope is currently selected. Without one, the graph is
   *  empty even if a diff is loaded. */
  hasScope: boolean;
  /** How many distinct files contain added/removed/modified entities.
   *  Used to label the "Scope to changed files" button. */
  changedFileCount: number;
  /** Master toggle for whether diff filters are currently applied. */
  filtersEnabled: boolean;
  /** Whether a node is currently selected — selection changes how the diff
   *  filters behave (non-matching nodes get hidden rather than dimmed). */
  hasSelection: boolean;
}
type DiffHandler = (state: DiffState) => void;
type ScopesHandler = (paths: string[]) => void;

/**
 * Manages the Webview panel that hosts the Svelte visualization UI.
 *
 * The webview loads the same Svelte app built by Vite, but with a small
 * adapter injected via window.__NAO_VSCODE__ that tells the app to
 * route API calls to the local nao server and enables go-to-definition.
 */
export class VisualizerPanel {
  static currentPanel: VisualizerPanel | undefined;
  private static goToDefHandlers: GoToDefinitionHandler[] = [];
  private static goToDefDisposable: vscode.Disposable | undefined;
  private static selectionHandlers: SelectionHandler[] = [];
  private static selectionDisposable: vscode.Disposable | undefined;
  private static descriptionHandlers: DescriptionHandler[] = [];
  private static descriptionDisposable: vscode.Disposable | undefined;
  private static qualityHandlers: QualityHandler[] = [];
  private static qualityDisposable: vscode.Disposable | undefined;
  private static filterHandlers: FilterHandler[] = [];
  private static filterDisposable: vscode.Disposable | undefined;
  private static levelFilterHandlers: LevelFilterHandler[] = [];
  private static levelFilterDisposable: vscode.Disposable | undefined;
  private static diffHandlers: DiffHandler[] = [];
  private static diffDisposable: vscode.Disposable | undefined;
  private static scopesHandlers: ScopesHandler[] = [];
  private static scopesDisposable: vscode.Disposable | undefined;
  private static analysisScopesHandlers: ScopesHandler[] = [];
  private static analysisScopesDisposable: vscode.Disposable | undefined;

  private readonly panel: vscode.WebviewPanel;
  private readonly extensionUri: vscode.Uri;
  private readonly serverPort: number;
  private readonly workspaceRoot: string;
  private disposables: vscode.Disposable[] = [];

  /** Messages queued before the Svelte app's listeners are mounted.
   *  Flushed when the webview posts { type: 'ready' }. */
  private pendingMessages: unknown[] = [];
  private webviewReady = false;

  private constructor(
    panel: vscode.WebviewPanel,
    extensionUri: vscode.Uri,
    serverPort: number,
    workspaceRoot: string
  ) {
    this.panel = panel;
    this.extensionUri = extensionUri;
    this.serverPort = serverPort;
    this.workspaceRoot = workspaceRoot;

    this.panel.webview.html = this.getHtml();

    // Handle messages from the webview
    this.panel.webview.onDidReceiveMessage(
      (msg) => this.handleMessage(msg),
      null,
      this.disposables
    );

    this.panel.onDidDispose(() => this.dispose(), null, this.disposables);
  }

  /** Send a message to the webview, buffering until the Svelte app is mounted. */
  private post(msg: unknown): void {
    if (this.webviewReady) {
      this.panel.webview.postMessage(msg);
    } else {
      this.pendingMessages.push(msg);
    }
  }

  private flushPending(): void {
    this.webviewReady = true;
    for (const msg of this.pendingMessages) {
      this.panel.webview.postMessage(msg);
    }
    this.pendingMessages = [];
  }

  static createOrShow(
    context: vscode.ExtensionContext,
    serverPort: number,
    workspaceRoot: string
  ): VisualizerPanel {
    const column = vscode.ViewColumn.Beside;

    if (VisualizerPanel.currentPanel) {
      VisualizerPanel.currentPanel.panel.reveal(column);
      return VisualizerPanel.currentPanel;
    }

    const panel = vscode.window.createWebviewPanel(
      'naoVisualizer',
      'Nao — Code Visualizer',
      column,
      {
        enableScripts: true,
        retainContextWhenHidden: true,
        localResourceRoots: [
          vscode.Uri.joinPath(context.extensionUri, 'webview-dist'),
        ],
      }
    );

    VisualizerPanel.currentPanel = new VisualizerPanel(
      panel,
      context.extensionUri,
      serverPort,
      workspaceRoot
    );

    return VisualizerPanel.currentPanel;
  }

  static onGoToDefinition(handler: GoToDefinitionHandler): vscode.Disposable {
    VisualizerPanel.goToDefHandlers.push(handler);
    if (!VisualizerPanel.goToDefDisposable) {
      VisualizerPanel.goToDefDisposable = {
        dispose: () => {
          VisualizerPanel.goToDefHandlers = [];
        },
      };
    }
    return VisualizerPanel.goToDefDisposable;
  }

  static onSelectionChanged(handler: SelectionHandler): vscode.Disposable {
    VisualizerPanel.selectionHandlers.push(handler);
    if (!VisualizerPanel.selectionDisposable) {
      VisualizerPanel.selectionDisposable = {
        dispose: () => {
          VisualizerPanel.selectionHandlers = [];
        },
      };
    }
    return VisualizerPanel.selectionDisposable;
  }

  static onDescriptionChanged(handler: DescriptionHandler): vscode.Disposable {
    VisualizerPanel.descriptionHandlers.push(handler);
    if (!VisualizerPanel.descriptionDisposable) {
      VisualizerPanel.descriptionDisposable = {
        dispose: () => {
          VisualizerPanel.descriptionHandlers = [];
        },
      };
    }
    return VisualizerPanel.descriptionDisposable;
  }

  static onQualityChanged(handler: QualityHandler): vscode.Disposable {
    VisualizerPanel.qualityHandlers.push(handler);
    if (!VisualizerPanel.qualityDisposable) {
      VisualizerPanel.qualityDisposable = {
        dispose: () => {
          VisualizerPanel.qualityHandlers = [];
        },
      };
    }
    return VisualizerPanel.qualityDisposable;
  }

  static onFiltersChanged(handler: FilterHandler): vscode.Disposable {
    VisualizerPanel.filterHandlers.push(handler);
    if (!VisualizerPanel.filterDisposable) {
      VisualizerPanel.filterDisposable = {
        dispose: () => {
          VisualizerPanel.filterHandlers = [];
        },
      };
    }
    return VisualizerPanel.filterDisposable;
  }

  static onLevelFiltersChanged(handler: LevelFilterHandler): vscode.Disposable {
    VisualizerPanel.levelFilterHandlers.push(handler);
    if (!VisualizerPanel.levelFilterDisposable) {
      VisualizerPanel.levelFilterDisposable = {
        dispose: () => {
          VisualizerPanel.levelFilterHandlers = [];
        },
      };
    }
    return VisualizerPanel.levelFilterDisposable;
  }

  static onDiffChanged(handler: DiffHandler): vscode.Disposable {
    VisualizerPanel.diffHandlers.push(handler);
    if (!VisualizerPanel.diffDisposable) {
      VisualizerPanel.diffDisposable = {
        dispose: () => {
          VisualizerPanel.diffHandlers = [];
        },
      };
    }
    return VisualizerPanel.diffDisposable;
  }

  static onScopesChanged(handler: ScopesHandler): vscode.Disposable {
    VisualizerPanel.scopesHandlers.push(handler);
    if (!VisualizerPanel.scopesDisposable) {
      VisualizerPanel.scopesDisposable = {
        dispose: () => {
          VisualizerPanel.scopesHandlers = [];
        },
      };
    }
    return VisualizerPanel.scopesDisposable;
  }

  static onAnalysisScopesChanged(handler: ScopesHandler): vscode.Disposable {
    VisualizerPanel.analysisScopesHandlers.push(handler);
    if (!VisualizerPanel.analysisScopesDisposable) {
      VisualizerPanel.analysisScopesDisposable = {
        dispose: () => {
          VisualizerPanel.analysisScopesHandlers = [];
        },
      };
    }
    return VisualizerPanel.analysisScopesDisposable;
  }

  static dispose(): void {
    VisualizerPanel.currentPanel?.dispose();
  }

  /** Tell the webview to scope the visualization to a specific file.
   *  The file path is converted to a path relative to the analyzed root so
   *  the webview can look it up in the index. */
  focusFile(filePath: string, line?: number): void {
    const relativePath = this.toRelative(filePath);
    if (relativePath === undefined) return;
    this.post({ type: 'focusFile', filePath, relativePath, line });
  }

  /** Tell the webview to select the node containing the cursor, without
   *  re-scoping. Used for cursor-follow behaviour while editing. */
  focusCursor(filePath: string, line: number): void {
    const relativePath = this.toRelative(filePath);
    console.log(
      `[cursor-sync/panel] focusCursor called filePath=${filePath} line=${line} workspaceRoot=${this.workspaceRoot} → relativePath=${relativePath ?? '(null)'}`
    );
    if (relativePath === undefined) {
      console.log(
        `[cursor-sync/panel] SKIPPED — filePath is outside workspaceRoot (toRelative returned undefined)`
      );
      return;
    }
    console.log(
      `[cursor-sync/panel] posting message type=focusCursor relativePath=${relativePath} line=${line} webviewReady=${this.webviewReady}`
    );
    this.post({ type: 'focusCursor', filePath, relativePath, line });
  }

  /** Lightweight update of the "which file is the user looking at" signal.
   *  Unlike focusFile, this does NOT change scope or selection — it only
   *  powers the Quality view's "Current file" analysis option so the
   *  dropdown entry is enabled as soon as an editor is focused. Sent on
   *  panel creation and on every editor change, regardless of autoVisualize. */
  setCurrentEditorFile(filePath: string): void {
    const relativePath = this.toRelative(filePath);
    if (relativePath === undefined) return;
    this.post({ type: 'command', command: 'setCurrentFile', value: relativePath });
  }

  /** Replace the current scope selection in the graph with the given paths.
   *  Empty array clears the selection. */
  setScopes(paths: string[]): void {
    this.post({ type: 'setScopes', paths });
  }

  /** Narrow the scope to a single path AND re-enable auto-level so the view
   *  expands to the finest level that fits. Distinct from `setScopes`
   *  because auto-level pin state should persist across scope changes EXCEPT
   *  when the user explicitly drills in. */
  drillIn(path: string): void {
    console.log('[drill] VisualizerPanel.drillIn() posting to webview path=' + path + ' webviewReady=' + this.webviewReady);
    this.post({ type: 'drillIn', path });
  }

  /** Generic command dispatch — used by the native "View Options" sidebar to
   *  drive store updates and GraphView actions (zoom, fit, toggle mode, etc.). */
  sendCommand(command: string, value?: unknown): void {
    this.post({ type: 'command', command, value });
  }

  private toRelative(filePath: string): string | undefined {
    const rel = path.relative(this.workspaceRoot, filePath);
    if (rel.startsWith('..') || path.isAbsolute(rel)) return undefined;
    return rel;
  }

  private handleMessage(msg: { type: string; [key: string]: unknown }): void {
    switch (msg.type) {
      case 'goToDefinition': {
        const location = msg as unknown as { type: string } & GoToDefinitionLocation;
        for (const handler of VisualizerPanel.goToDefHandlers) {
          handler(location);
        }
        break;
      }
      case 'selectionChanged': {
        const payload = msg.payload as SelectionPayload | undefined;
        for (const handler of VisualizerPanel.selectionHandlers) {
          handler(payload);
        }
        break;
      }
      case 'descriptionChanged': {
        const payload = msg.payload as DescriptionPayload | undefined;
        for (const handler of VisualizerPanel.descriptionHandlers) {
          handler(payload);
        }
        break;
      }
      case 'qualityChanged': {
        const payload = msg.payload as QualityPayload | undefined;
        if (payload) {
          for (const handler of VisualizerPanel.qualityHandlers) {
            handler(payload);
          }
        }
        break;
      }
      case 'filtersChanged': {
        const state = msg.state as FilterState | undefined;
        if (state) {
          for (const handler of VisualizerPanel.filterHandlers) {
            handler(state);
          }
        }
        break;
      }
      case 'levelFiltersChanged': {
        const state = msg.state as LevelFilterState | undefined;
        if (state) {
          for (const handler of VisualizerPanel.levelFilterHandlers) {
            handler(state);
          }
        }
        break;
      }
      case 'diffChanged': {
        const state = msg.state as DiffState | undefined;
        if (state) {
          for (const handler of VisualizerPanel.diffHandlers) {
            handler(state);
          }
        }
        break;
      }
      case 'scopesChanged': {
        const paths = (msg.paths as string[]) ?? [];
        for (const handler of VisualizerPanel.scopesHandlers) {
          handler(paths);
        }
        break;
      }
      case 'analysisScopesChanged': {
        const paths = (msg.paths as string[]) ?? [];
        for (const handler of VisualizerPanel.analysisScopesHandlers) {
          handler(paths);
        }
        break;
      }
      case 'ready':
        this.flushPending();
        break;
    }
  }

  private dispose(): void {
    VisualizerPanel.currentPanel = undefined;
    this.panel.dispose();
    for (const d of this.disposables) d.dispose();
    this.disposables = [];
  }

  /**
   * Build the HTML for the webview.
   *
   * Strategy: if a pre-built webview-dist/ exists (from `npm run build:webview`),
   * load it. Otherwise, fall back to an iframe pointing at the nao server,
   * which serves the UI directly (works during development).
   */
  private getHtml(): string {
    const webviewDistPath = path.join(this.extensionUri.fsPath, 'webview-dist');
    const indexPath = path.join(webviewDistPath, 'index.html');

    if (fs.existsSync(indexPath)) {
      return this.getBuiltHtml(indexPath, webviewDistPath);
    }

    // Fallback: iframe to the running nao server
    return this.getIframeHtml();
  }

  /**
   * Load the pre-built Svelte app and inject the VS Code adapter config.
   */
  private getBuiltHtml(indexPath: string, distDir: string): string {
    let html = fs.readFileSync(indexPath, 'utf-8');
    const webview = this.panel.webview;

    // Rewrite asset paths to use webview URIs
    html = html.replace(
      /(href|src)="(\/?)([^"]*?)"/g,
      (_match, attr, _slash, filePath) => {
        if (filePath.startsWith('http')) return `${attr}="${filePath}"`;
        const diskPath = vscode.Uri.file(path.join(distDir, filePath));
        const webviewUri = webview.asWebviewUri(diskPath);
        return `${attr}="${webviewUri}"`;
      }
    );

    // Inject the VS Code adapter before the first <script>
    const adapterScript = this.getAdapterScript();
    html = html.replace('<head>', `<head>\n${adapterScript}`);

    return html;
  }

  /**
   * Fallback: render the UI in an iframe pointing at the nao server.
   * This works during development without building the webview.
   */
  private getIframeHtml(): string {
    const serverUrl = `http://localhost:${this.serverPort}`;
    return /* html */ `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <style>
    body, html { margin: 0; padding: 0; width: 100%; height: 100%; overflow: hidden; }
    iframe { border: none; width: 100%; height: 100%; }
  </style>
</head>
<body>
  <iframe id="nao-frame" src="${serverUrl}"></iframe>
  <script>
    const vscode = acquireVsCodeApi();
    const frame = document.getElementById('nao-frame');

    // Forward messages from the iframe to the extension
    window.addEventListener('message', (e) => {
      if (e.source === frame.contentWindow && e.data?.type) {
        vscode.postMessage(e.data);
      }
    });

    // Forward messages from the extension to the iframe
    window.addEventListener('message', (e) => {
      if (e.source !== frame.contentWindow && e.data?.type) {
        frame.contentWindow?.postMessage(e.data, '*');
      }
    });
  </script>
</body>
</html>`;
  }

  /**
   * Adapter script injected into the webview HTML.
   * Sets up window.__NAO_VSCODE__ so the Svelte app knows it's inside VS Code.
   */
  private getAdapterScript(): string {
    return /* html */ `<script>
  // VS Code webview API
  const __vscode = acquireVsCodeApi();

  // Configuration object that the Svelte app reads
  window.__NAO_VSCODE__ = {
    /** Base URL for all API calls (points to the nao server) */
    apiBase: 'http://localhost:${this.serverPort}',

    /** Send a go-to-definition request to the extension host */
    goToDefinition(filePath, line) {
      __vscode.postMessage({ type: 'goToDefinition', filePath, line });
    },

    /** Post an arbitrary message to the extension host */
    postMessage(msg) {
      __vscode.postMessage(msg);
    },
  };

  // Listen for messages from the extension host
  window.addEventListener('message', (event) => {
    const msg = event.data;
    if (!msg?.type) return;
    if (msg.type === 'focusCursor') {
      console.log('[cursor-sync/webview-bridge] received focusCursor', msg);
    }
    if (msg.type === 'focusFile' || msg.type === 'focusCursor' || msg.type === 'setScopes' || msg.type === 'drillIn' || msg.type === 'command') {
      if (msg.type === 'drillIn') {
        console.log('[drill] webview-bridge received drillIn message', msg);
      }
      window.dispatchEvent(new CustomEvent('nao:' + msg.type, { detail: msg }));
      if (msg.type === 'focusCursor') {
        console.log('[cursor-sync/webview-bridge] dispatched CustomEvent nao:focusCursor');
      }
      if (msg.type === 'drillIn') {
        console.log('[drill] webview-bridge dispatched CustomEvent nao:drillIn');
      }
    }
  });

  // The Svelte app signals ready itself (after onMount attaches listeners)
  // by calling window.__NAO_VSCODE__.postMessage({ type: 'ready' }).
</script>`;
  }
}
