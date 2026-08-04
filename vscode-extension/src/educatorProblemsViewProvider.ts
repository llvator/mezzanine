import * as vscode from 'vscode';
import * as path from 'path';
import { ScanHit, fetchScan, relativeToWorkspace } from './educatorClient';

/**
 * Dedicated "Educator Problems" panel under the Nao sidebar — replaces the
 * earlier `DiagnosticCollection` integration so Nao findings don't share the
 * Problems view with Java / SonarQube / compiler diagnostics.
 *
 * Lifecycle mirrors the previous diagnostics module: scan a Java buffer on
 * open and save, drop it from the tree on close. The fetch path
 * (`/api/educator/scan`) is unchanged — only the rendering target moved.
 */

type EducatorTreeNode = FileNode | HitNode;

interface FileNode {
  kind: 'file';
  uri: vscode.Uri;
  /** Workspace-relative path; used as the file-node label. */
  relPath: string;
  hits: ScanHit[];
}

interface HitNode {
  kind: 'hit';
  uri: vscode.Uri;
  hit: ScanHit;
}

export class EducatorProblemsView implements vscode.TreeDataProvider<EducatorTreeNode> {
  static readonly viewId = 'nao.educatorProblems';

  private readonly hitsByUri = new Map<string, FileNode>();
  private readonly _onDidChangeTreeData = new vscode.EventEmitter<EducatorTreeNode | undefined>();
  readonly onDidChangeTreeData = this._onDidChangeTreeData.event;

  /** TreeView handle — attached after construction so the provider can set
      the view's `badge` (the small counter next to the panel title). */
  private treeView?: vscode.TreeView<EducatorTreeNode>;
  /** URI of the editor currently in focus — the badge counts hits for this
      file, not the whole workspace. Updated by the host extension. */
  private activeUri?: vscode.Uri;

  constructor(
    private readonly getServerPort: () => number | undefined,
    private readonly getWorkspaceRoot: () => string | undefined,
    private readonly output: vscode.OutputChannel
  ) {}

  /** Receive the TreeView handle once it has been created. Owning the view
      lets the provider drive the badge counter as data changes. */
  attachTreeView(view: vscode.TreeView<EducatorTreeNode>): void {
    this.treeView = view;
    this.updateBadge();
  }

  /** Tell the provider which file is currently in focus. Setting `undefined`
      (non-Java editor, no editor) clears the badge. */
  setActiveUri(uri: vscode.Uri | undefined): void {
    this.activeUri = uri;
    this.updateBadge();
  }

  // ─── TreeDataProvider contract ─────────────────────────────────────────

  getTreeItem(node: EducatorTreeNode): vscode.TreeItem {
    if (node.kind === 'file') {
      const item = new vscode.TreeItem(
        `${path.basename(node.relPath)}  (${node.hits.length})`,
        vscode.TreeItemCollapsibleState.Expanded
      );
      item.tooltip = node.relPath;
      item.resourceUri = node.uri;
      item.iconPath = vscode.ThemeIcon.File;
      return item;
    }
    const { hit } = node;
    const label = `${hit.rule_id}  ${hit.line + 1}:${hit.col + 1}`;
    const item = new vscode.TreeItem(label, vscode.TreeItemCollapsibleState.None);
    item.description = hit.title;
    item.tooltip = `${hit.title}\n\nRule: ${hit.rule_id}\nSeverity: ${hit.severity}\nKind: ${hit.rule_kind}`;
    item.iconPath = severityIcon(hit.severity);
    item.command = {
      command: 'nao.educator.openHit',
      title: 'Open Educator Hit',
      arguments: [{ file: node.uri.fsPath, line: hit.line, col: hit.col }],
    };
    return item;
  }

  getChildren(node?: EducatorTreeNode): EducatorTreeNode[] {
    if (!node) {
      const files = [...this.hitsByUri.values()];
      files.sort((a, b) => a.relPath.localeCompare(b.relPath));
      return files;
    }
    if (node.kind === 'file') {
      return node.hits.map((hit) => ({ kind: 'hit', uri: node.uri, hit }));
    }
    return [];
  }

  // ─── Scan integration ──────────────────────────────────────────────────

  /** Scan one document and replace its entry in the tree. */
  async refresh(document: vscode.TextDocument): Promise<void> {
    if (document.languageId !== 'java') return;
    if (document.uri.scheme !== 'file') return;

    const port = this.getServerPort();
    if (!port) {
      this.log(`refresh skipped — server not running for ${document.uri.fsPath}`);
      return;
    }
    const workspaceRoot = this.getWorkspaceRoot();
    if (!workspaceRoot) return;
    const rel = relativeToWorkspace(document.uri.fsPath, workspaceRoot);
    if (!rel) return;

    try {
      const response = await fetchScan(port, rel);
      const key = document.uri.toString();
      if (response.hits.length === 0) {
        this.hitsByUri.delete(key);
      } else {
        this.hitsByUri.set(key, {
          kind: 'file',
          uri: document.uri,
          relPath: rel,
          hits: response.hits,
        });
      }
      this._onDidChangeTreeData.fire(undefined);
      this.updateBadge();
      this.log(`scanned ${rel} → ${response.hits.length} hit(s)`);
    } catch (err) {
      this.log(`scan failed for ${rel}: ${(err as Error).message}`);
    }
  }

  clear(document: vscode.TextDocument): void {
    if (this.hitsByUri.delete(document.uri.toString())) {
      this._onDidChangeTreeData.fire(undefined);
      this.updateBadge();
    }
  }

  /** Recompute the view's title badge from the active file's hit count.
      Skips when no view is attached yet. A zero count clears the badge so
      "no problems" doesn't look the same as "1 problem". */
  private updateBadge(): void {
    if (!this.treeView) return;
    const node = this.activeUri ? this.hitsByUri.get(this.activeUri.toString()) : undefined;
    const count = node?.hits.length ?? 0;
    if (count === 0) {
      this.treeView.badge = undefined;
      return;
    }
    const fileName = this.activeUri ? path.basename(this.activeUri.fsPath) : '';
    this.treeView.badge = {
      value: count,
      tooltip: `${count} Educator finding${count === 1 ? '' : 's'} in ${fileName}`,
    };
  }

  private log(line: string): void {
    this.output.appendLine(`[educator-problems] ${line}`);
  }
}

function severityIcon(severity: string): vscode.ThemeIcon {
  switch (severity) {
    case 'error':
      return new vscode.ThemeIcon('error', new vscode.ThemeColor('list.errorForeground'));
    case 'warning':
      return new vscode.ThemeIcon('warning', new vscode.ThemeColor('list.warningForeground'));
    default:
      return new vscode.ThemeIcon('info');
  }
}
