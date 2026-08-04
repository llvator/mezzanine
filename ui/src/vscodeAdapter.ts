/**
 * VS Code adapter layer.
 *
 * When the app runs inside a VS Code webview, `window.__NAO_VSCODE__`
 * is set by the extension host. This module exposes helpers that the
 * stores and components can use without directly touching the VS Code API.
 *
 * When running standalone (browser / nao watch), these are no-ops or
 * fall through to the default behaviour.
 */

import { endpoint, withToken } from './endpoint';

export interface VscodeConfig {
  apiBase: string;
  goToDefinition(filePath: string, line?: number): void;
  postMessage(msg: Record<string, unknown>): void;
}

declare global {
  interface Window {
    __NAO_VSCODE__?: VscodeConfig;
  }
}

/** True when the app is running inside a VS Code webview. */
export function isVscode(): boolean {
  return typeof window !== 'undefined' && !!window.__NAO_VSCODE__;
}

/**
 * Return the API base URL: `''` for same-origin, a full origin otherwise.
 *
 * The webview still wins, and same-origin is still the default — see
 * `endpoint.ts` for the resolution order this now delegates to.
 */
export function apiBase(): string {
  return endpoint().base;
}

/**
 * Slug of the repo the app is currently viewing under `nao serve`, or
 * `null` under `nao watch` / VS Code, where the repo is implicit.
 *
 * Deliberately a module-level variable rather than a store: `apiUrl` is
 * called from plain async functions all over `stores/`, and every one of
 * them needs the *current* slug synchronously. `stores/serveMode.ts` owns
 * the reactive view of this and keeps the two in step.
 */
let serveRepoSlug: string | null = null;

/** Point every subsequent `apiUrl()` at `slug`, or back at the flat
 *  namespace when passed `null`. Called only by `stores/serveMode.ts`. */
export function setServeRepo(slug: string | null): void {
  serveRepoSlug = slug;
}

/** The active serve-mode repo slug, or `null`. */
export function serveRepo(): string | null {
  return serveRepoSlug;
}

/**
 * Build a full URL for an API endpoint.
 * Standalone:  `/api/graph` → `/api/graph`
 * Serve mode:  `/api/graph` → `/api/repos/{slug}/graph`
 * VS Code:     `/api/graph` → `http://localhost:3200/api/graph`
 * Configured:  `/api/graph` → `http://localhost:3200/api/graph`
 *
 * VS Code mode never takes the serve branch — the extension talks to a
 * `nao watch` server, which has no per-repo namespace.
 *
 * The two prefixes compose rather than compete: the serve-mode slug is a
 * *path* namespace and the configured base is an *origin*, so a remote
 * `nao serve` needs both. Applying only the slug — which is what this did
 * before UI-032 — sent every serve-mode request back to the page's own
 * origin, where nothing is listening.
 */
export function apiUrl(path: string): string {
  const { base, token } = endpoint();
  if (!isVscode() && serveRepoSlug !== null && path.startsWith('/api/')) {
    // Drop the leading `/api`, keeping any query string: `/api/commits?x`
    // → `/api/repos/{slug}/commits?x`.
    return withToken(`${base}/api/repos/${serveRepoSlug}${path.slice('/api'.length)}`, token);
  }
  return withToken(`${base}${path}`, token);
}

/**
 * Build a URL that takes the configured base but never the serve-mode slug.
 *
 * For the handful of endpoints that live outside the per-repo namespace —
 * `GET /api/repos` above all, which is what *decides* whether there is a
 * slug. Routing that through `apiUrl` once the slug is set would prefix it
 * with the very namespace it exists to discover; skipping the base instead
 * makes serve mode undetectable against a remote engine.
 */
export function flatApiUrl(path: string): string {
  const { base, token } = endpoint();
  return withToken(`${base}${path}`, token);
}

/**
 * Navigate to a source location. In VS Code this opens the file in
 * the editor. In standalone mode this is a no-op (or could open a
 * source view panel in the future).
 */
export function goToDefinition(filePath: string, line?: number): void {
  if (window.__NAO_VSCODE__) {
    window.__NAO_VSCODE__.goToDefinition(filePath, line);
  }
}

/** Report that the user selected (or deselected) a node in the graph.
 *  The extension forwards this payload to the native Selection side view. */
export function reportSelection(payload: Record<string, unknown> | null): void {
  if (window.__NAO_VSCODE__) {
    window.__NAO_VSCODE__.postMessage({
      type: 'selectionChanged',
      payload: payload ?? undefined,
    });
  }
}

/** Report the description chain for the node the pointer (or the selection)
 *  is on, so the native Description side view can narrate it. The chain is
 *  resolved here rather than in the extension because the parent walk needs
 *  the entity-level graph and the details sidecar, both of which live in
 *  the webview. */
export function reportDescription(payload: {
  source: 'hover' | 'selection';
  chain: unknown[];
} | null): void {
  if (window.__NAO_VSCODE__) {
    window.__NAO_VSCODE__.postMessage({
      type: 'descriptionChanged',
      payload: payload ?? undefined,
    });
  }
}

/** Report the current quality summary + top rows so the native Quality
 *  side view can display scope-level aggregation plus refactor candidates. */
export function reportQuality(payload: {
  summary: Record<string, unknown>;
  rows: unknown[];
}): void {
  if (window.__NAO_VSCODE__) {
    window.__NAO_VSCODE__.postMessage({ type: 'qualityChanged', payload });
  }
}

/** Report the current set of selected scope paths so the native Scopes
 *  tree can sync its checkboxes. Used whenever the scope changes from
 *  anywhere other than a user click in the tree itself (e.g. from the
 *  Diff view's "Scope to changes" button). */
export function reportScopes(paths: string[]): void {
  if (window.__NAO_VSCODE__) {
    window.__NAO_VSCODE__.postMessage({ type: 'scopesChanged', paths });
  }
}

/** Same as reportScopes but for the separate *analysis* scope that
 *  drives the Quality / Summary side panels. */
export function reportAnalysisScopes(paths: string[]): void {
  if (window.__NAO_VSCODE__) {
    window.__NAO_VSCODE__.postMessage({ type: 'analysisScopesChanged', paths });
  }
}

/** Report the current filter state (entity types, rel types, direction,
 *  languages, plus the complete sets available in the current scope) so the
 *  native Filters side view can render togglable chips. */
export function reportFilters(state: {
  entityTypes: { name: string; enabled: boolean }[];
  relTypes: { name: string; enabled: boolean }[];
  directions: { outgoing: boolean; incoming: boolean };
  languages: { name: string; enabled: boolean }[];
}): void {
  if (window.__NAO_VSCODE__) {
    window.__NAO_VSCODE__.postMessage({ type: 'filtersChanged', state });
  }
}

/** Report current diff state (active flag, refs, summary counts, filter
 *  toggles) so the native Diff side view can render a summary + controls. */
export function reportDiff(state: {
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
  hasScope: boolean;
  changedFileCount: number;
  filtersEnabled: boolean;
  hasSelection: boolean;
}): void {
  if (window.__NAO_VSCODE__) {
    window.__NAO_VSCODE__.postMessage({ type: 'diffChanged', state });
  }
}

export type TriStateValue = 'on' | 'off' | 'general';

/** Report per-level filter overrides so the native Level Filters view
 *  can render collapsible per-depth sections with tri-state chips. */
export function reportLevelFilters(state: {
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
}): void {
  if (window.__NAO_VSCODE__) {
    window.__NAO_VSCODE__.postMessage({ type: 'levelFiltersChanged', state });
  }
}

export interface FocusFileEvent {
  /** Absolute path to the file in the user's filesystem. */
  filePath: string;
  /** Path relative to the analyzed workspace root (usable as a scope key). */
  relativePath: string;
  /** Cursor line (1-based) at the time of the focus request, if known. */
  line?: number;
}

export interface FocusCursorEvent {
  filePath: string;
  relativePath: string;
  /** Cursor line (1-based). */
  line: number;
}

/** Subscribe to focus-file events from the extension host.
 *  Returns an unsubscribe function. */
export function onFocusFile(
  callback: (event: FocusFileEvent) => void
): () => void {
  const handler = (e: Event) => {
    const detail = (e as CustomEvent).detail;
    if (detail?.filePath && detail?.relativePath) {
      callback({
        filePath: detail.filePath,
        relativePath: detail.relativePath,
        line: detail.line,
      });
    }
  };
  window.addEventListener('nao:focusFile', handler);
  return () => window.removeEventListener('nao:focusFile', handler);
}

/** Subscribe to cursor-follow events (cursor moved within the editor). */
export function onFocusCursor(
  callback: (event: FocusCursorEvent) => void
): () => void {
  const handler = (e: Event) => {
    const detail = (e as CustomEvent).detail;
    if (detail?.relativePath && typeof detail.line === 'number') {
      callback({
        filePath: detail.filePath,
        relativePath: detail.relativePath,
        line: detail.line,
      });
    }
  };
  window.addEventListener('nao:focusCursor', handler);
  return () => window.removeEventListener('nao:focusCursor', handler);
}

/** Subscribe to external scope-selection changes (from the native TreeView). */
export function onSetScopes(callback: (paths: string[]) => void): () => void {
  const handler = (e: Event) => {
    const detail = (e as CustomEvent).detail;
    if (Array.isArray(detail?.paths)) callback(detail.paths);
  };
  window.addEventListener('nao:setScopes', handler);
  return () => window.removeEventListener('nao:setScopes', handler);
}

/** Subscribe to drill-in requests (native Selection panel's "Drill in"
 *  button). Semantically distinct from setScopes: drill-in always resets
 *  auto-level so the view expands to the finest level that fits. */
export function onDrillIn(callback: (path: string) => void): () => void {
  const handler = (e: Event) => {
    const detail = (e as CustomEvent).detail;
    if (typeof detail?.path === 'string') callback(detail.path);
  };
  window.addEventListener('nao:drillIn', handler);
  return () => window.removeEventListener('nao:drillIn', handler);
}

/** Subscribe to generic view-option commands from the native Controls panel.
 *  Commands: setViewMode, setGraphLevel, setTreeDepth, setTreeDensity,
 *  setHoverDepth, setShowLabels, setShowKindLabels, setShowLinkLabels,
 *  setAutoFit, clearSelection, zoomIn, zoomOut, resetZoom, fitView, fitWidth. */
export function onCommand(
  callback: (command: string, value: unknown) => void
): () => void {
  const handler = (e: Event) => {
    const detail = (e as CustomEvent).detail;
    if (typeof detail?.command === 'string') callback(detail.command, detail.value);
  };
  window.addEventListener('nao:command', handler);
  return () => window.removeEventListener('nao:command', handler);
}
