import * as vscode from 'vscode';
import * as http from 'http';

export interface IndexNode {
  path: string;
  type: 'folder' | 'file';
  entity_count: number;
  relationship_count: number;
  languages?: string[];
  children?: string[];
}

interface IndexData {
  root: string;
  nodes: Record<string, IndexNode>;
  total_entities: number;
  total_relationships: number;
}

/**
 * A node in the scope tree, keyed by its path (relative to the analyzed root).
 * The empty string '' represents the root itself.
 */
class ScopeItem extends vscode.TreeItem {
  constructor(
    public readonly node: IndexNode,
    collapsibleState: vscode.TreeItemCollapsibleState,
    checked: boolean,
    /** How many selected paths exist at-or-below this folder. 0 for files
     *  and for folders with no selection underneath. */
    selectedInside = 0
  ) {
    const label =
      node.path === ''
        ? '(workspace root)'
        : node.path.split('/').pop() ?? node.path;
    super(label, collapsibleState);

    this.id = node.path || '__root__';
    this.resourceUri = node.path ? vscode.Uri.file(node.path) : undefined;
    const base = `${node.entity_count} · ${node.relationship_count} rels`;
    this.description = selectedInside > 0 && node.type === 'folder'
      ? `${base} · ${selectedInside} \u2713`
      : base;
    const selLine = selectedInside > 0 && node.type === 'folder'
      ? `\n${selectedInside} selected path${selectedInside === 1 ? '' : 's'} inside`
      : '';
    this.tooltip = `${node.path || '(root)'}\n${node.entity_count} entities\n${node.relationship_count} relationships${
      node.languages?.length ? `\n${node.languages.join(', ')}` : ''
    }${selLine}`;
    this.iconPath = new vscode.ThemeIcon(
      node.type === 'folder' ? 'folder' : 'file-code'
    );
    this.checkboxState = checked
      ? vscode.TreeItemCheckboxState.Checked
      : vscode.TreeItemCheckboxState.Unchecked;
    this.contextValue = node.type;
  }
}

export type ScopeChangeHandler = (paths: string[]) => void;

export class ScopeTreeProvider implements vscode.TreeDataProvider<ScopeItem> {
  private readonly _onDidChangeTreeData = new vscode.EventEmitter<
    ScopeItem | undefined | void
  >();
  readonly onDidChangeTreeData = this._onDidChangeTreeData.event;

  /** Default selection applied when setIndex runs for the first time
   *  — used to let the "Analysis Scope" tree start with the root
   *  checked so Quality works out of the box on the whole codebase. */
  private readonly initialSelection: Set<string>;

  private index: IndexData | undefined;
  private selection: Set<string>;
  private serverPort = 3200;
  private changeHandler?: ScopeChangeHandler;

  constructor(initialSelection: string[] = []) {
    this.initialSelection = new Set(initialSelection);
    this.selection = new Set(this.initialSelection);
    this.rebuildDerivedState();
  }
  /** True while we're applying a selection change that came from outside
   *  (e.g. the Svelte app's "Scope to changes") so we don't echo it back
   *  and create a loop. */
  private suppressChangeHandler = false;
  /** Folders that should render as Expanded. Includes every ancestor of
   *  every currently-selected path, so the user can see their selection
   *  without manually drilling into each folder. */
  private forceExpand = new Set<string>();
  /** Cache: folder path → number of selected descendants. Shown as
   *  "N \u2713" next to the folder description so the user can tell at a
   *  glance where the selection lives even when folders are collapsed. */
  private selectedInsideCount = new Map<string, number>();
  /** Case-insensitive substring filter over full paths. Empty = no filter.
   *  A matched folder implicitly keeps its whole subtree visible, because
   *  every descendant's full path contains the folder's path. */
  private filterQuery = '';
  /** Paths visible under the active filter: every match plus its ancestors.
   *  undefined when no filter is active (= everything visible). */
  private filterVisible: Set<string> | undefined;
  /** Folders expanded because a filtered match lives underneath. */
  private filterExpand = new Set<string>();

  setServerPort(port: number): void {
    this.serverPort = port;
  }

  onSelectionChanged(handler: ScopeChangeHandler): void {
    this.changeHandler = handler;
  }

  /** Fetch the index from the mezz server and rebuild the tree. */
  async refresh(): Promise<void> {
    try {
      this.index = await fetchIndex(this.serverPort);
      this.rebuildFilterState();
      this._onDidChangeTreeData.fire();
    } catch (err) {
      vscode.window.showWarningMessage(
        `Mezzanine: could not load scope index — ${(err as Error).message}`
      );
    }
  }

  get filter(): string {
    return this.filterQuery;
  }

  /** Restrict the tree to paths containing `query` (plus their ancestors,
   *  auto-expanded so matches are immediately visible). Empty string clears. */
  setFilter(query: string): void {
    this.filterQuery = query.trim();
    this.rebuildFilterState();
    this._onDidChangeTreeData.fire();
  }

  private rebuildFilterState(): void {
    this.filterVisible = undefined;
    this.filterExpand.clear();
    if (!this.filterQuery || !this.index) return;
    const q = this.filterQuery.toLowerCase();
    const visible = new Set<string>();
    for (const path of Object.keys(this.index.nodes)) {
      if (path === '' || !path.toLowerCase().includes(q)) continue;
      visible.add(path);
      let cur = path;
      while (cur.length > 0) {
        const idx = cur.lastIndexOf('/');
        cur = idx >= 0 ? cur.slice(0, idx) : '';
        visible.add(cur);
        this.filterExpand.add(cur);
      }
    }
    this.filterVisible = visible;
  }

  /** Currently checked paths. */
  getSelection(): string[] {
    return [...this.selection];
  }

  /** All index nodes (for the QuickPick scope picker). Empty before the
   *  index has loaded. */
  getIndexNodes(): IndexNode[] {
    return this.index ? Object.values(this.index.nodes) : [];
  }

  /** Replace the selection as a user-initiated action: minimizes the paths,
   *  updates the tree, and notifies the panel — unlike setSelection, which
   *  is the silent inbound-sync path. */
  replaceSelection(paths: string[]): void {
    const next = new Set(minimizePaths(paths));
    if (setsEqual(this.selection, next)) return;
    this.selection = next;
    this.rebuildDerivedState();
    this._onDidChangeTreeData.fire();
    this.changeHandler?.([...this.selection]);
  }

  /** Widen the scope: replace each selected path with its own parent folder
   *  (root stays root). Overlaps collapse via minimizePaths — e.g. two
   *  sibling files widen to the single shared parent. */
  extendToParents(): void {
    if (this.selection.size === 0) return;
    this.replaceSelection([...this.selection].map(parentOf));
  }

  clear(): void {
    this.selection.clear();
    this.rebuildDerivedState();
    this._onDidChangeTreeData.fire();
    this.changeHandler?.([]);
  }

  /** Select the root (empty path), which scope-minimization treats as
   *  "everything under the root". Equivalent to checking every folder/file
   *  manually, but cheap and guaranteed complete. */
  selectAll(): void {
    this.selection = new Set(['']);
    this.rebuildDerivedState();
    this._onDidChangeTreeData.fire();
    this.changeHandler?.(['']);
  }

  /** Handle checkbox toggles from user clicks.
   *
   *  Narrowing semantics: when the user checks a child while an ancestor
   *  is already in the set, the ancestor is removed. Without this, the
   *  set would contain both ('', 'src', 'src/analyzer' etc.) and the
   *  scope-minimization step would collapse to the root — so checking a
   *  child would have no effect on the effective scope. */
  applyCheckboxChanges(items: readonly [ScopeItem, vscode.TreeItemCheckboxState][]): void {
    for (const [item, state] of items) {
      const path = item.node.path;
      if (state === vscode.TreeItemCheckboxState.Checked) {
        // Remove any explicitly-selected ancestor so the effective scope
        // is actually narrowed to the checked path (and its siblings
        // that were checked before).
        let cur = path;
        while (cur.length > 0) {
          const slash = cur.lastIndexOf('/');
          cur = slash >= 0 ? cur.slice(0, slash) : '';
          if (this.selection.has(cur)) {
            this.selection.delete(cur);
          }
        }
        // Also covers the root case: if '' is selected and we check any
        // non-root path, the '' entry is dropped by the loop above.
        this.selection.add(path);
      } else {
        this.selection.delete(path);
      }
    }
    this.rebuildDerivedState();
    this._onDidChangeTreeData.fire();
    if (!this.suppressChangeHandler) {
      this.changeHandler?.([...this.selection]);
    }
  }

  /** Replace the selection from an external source (e.g. the main panel
   *  reporting that scopes changed). Refreshes the tree but does NOT fire
   *  the change handler, to avoid an echo loop back to the source. */
  setSelection(paths: string[]): void {
    const next = new Set(paths);
    if (setsEqual(this.selection, next)) return;
    this.selection = next;
    this.rebuildDerivedState();
    this.suppressChangeHandler = true;
    try {
      this._onDidChangeTreeData.fire();
    } finally {
      this.suppressChangeHandler = false;
    }
  }

  /** Recompute the two display-helper caches (`forceExpand` and
   *  `selectedInsideCount`) from the current selection. O(N × depth). */
  private rebuildDerivedState(): void {
    this.forceExpand.clear();
    this.selectedInsideCount.clear();
    for (const path of this.selection) {
      // Each selected path counts toward every ancestor's tally and forces
      // every ancestor folder to render expanded.
      let current = path;
      while (current.length > 0) {
        const idx = current.lastIndexOf('/');
        current = idx >= 0 ? current.slice(0, idx) : '';
        this.forceExpand.add(current);
        this.selectedInsideCount.set(current, (this.selectedInsideCount.get(current) ?? 0) + 1);
      }
    }
  }

  getTreeItem(element: ScopeItem): vscode.TreeItem {
    return element;
  }

  getChildren(element?: ScopeItem): ScopeItem[] {
    if (!this.index) return [];

    // Root: start from the index's synthetic root node (path = '')
    const parent = element ? element.node : this.index.nodes[''];
    if (!parent) return [];

    const childPaths = parent.children ?? [];
    return childPaths
      .map((p) => this.index!.nodes[p])
      .filter((n): n is IndexNode => !!n)
      .filter((n) => !this.filterVisible || this.filterVisible.has(n.path))
      // Folders first, then files; alphabetical within each group.
      .sort((a, b) => {
        if (a.type !== b.type) return a.type === 'folder' ? -1 : 1;
        return a.path.localeCompare(b.path);
      })
      .map((n) => {
        const isFolder = n.type === 'folder' && (n.children?.length ?? 0) > 0;
        const expanded =
          isFolder && (this.forceExpand.has(n.path) || this.filterExpand.has(n.path));
        const selectedInside = this.selectedInsideCount.get(n.path) ?? 0;
        return new ScopeItem(
          n,
          isFolder
            ? (expanded
                ? vscode.TreeItemCollapsibleState.Expanded
                : vscode.TreeItemCollapsibleState.Collapsed)
            : vscode.TreeItemCollapsibleState.None,
          this.selection.has(n.path),
          selectedInside,
        );
      });
  }
}

function parentOf(path: string): string {
  const idx = path.lastIndexOf('/');
  return idx >= 0 ? path.slice(0, idx) : '';
}

/** Drop paths already covered by an ancestor in the set ('' covers all).
 *  Mirrors minimizeSelection in the webview's scope store. */
function minimizePaths(paths: string[]): string[] {
  const sorted = [...new Set(paths)].sort((a, b) => a.length - b.length);
  const result: string[] = [];
  for (const p of sorted) {
    const covered = result.some((k) => k === '' || k === p || p.startsWith(k + '/'));
    if (!covered) result.push(p);
  }
  return result;
}

function setsEqual<T>(a: Set<T>, b: Set<T>): boolean {
  if (a.size !== b.size) return false;
  for (const v of a) if (!b.has(v)) return false;
  return true;
}

function fetchIndex(port: number): Promise<IndexData> {
  return new Promise((resolve, reject) => {
    const req = http.request(
      {
        host: '127.0.0.1',
        port,
        path: '/api/index',
        method: 'GET',
        timeout: 5000,
      },
      (res) => {
        if (res.statusCode !== 200) {
          reject(new Error(`HTTP ${res.statusCode}`));
          res.resume();
          return;
        }
        let body = '';
        res.setEncoding('utf-8');
        res.on('data', (chunk) => (body += chunk));
        res.on('end', () => {
          try {
            resolve(JSON.parse(body));
          } catch (e) {
            reject(e);
          }
        });
      }
    );
    req.on('error', reject);
    req.on('timeout', () => {
      req.destroy();
      reject(new Error('timeout'));
    });
    req.end();
  });
}
