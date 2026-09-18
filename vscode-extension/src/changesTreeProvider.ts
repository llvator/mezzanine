import * as vscode from 'vscode';
import * as http from 'http';
import * as path from 'path';
import { VisualizerPanel, type ChangedFilesState, type ChangedFileRowState } from './panel';

/**
 * "Changes" — git's list of the files a loaded comparison touched, native
 * (UI-137).
 *
 * The browser has had this since [UI-134] as a sidebar tab; the editor, where
 * the change is actually being made, has never had it. The reading it makes
 * possible is *is the canvas telling me the truth about this change?* — a file
 * outside the analysis has no row on the canvas to be missing from, so
 * "another language" and "nothing changed" reach a reader as the same silence.
 * Every row here says which of the two it is.
 *
 * Two things this deliberately does not do:
 *
 * - **It does not join.** `ui/src/viewmodels/changedFiles.ts` owns that, and
 *   the webview sends the finished reading across the bridge. A second copy on
 *   this side would be a join that can disagree with itself, and the number
 *   worth publishing here is precisely a disagreement count.
 * - **It does not filter the canvas.** Clicking a row opens a diff. It does
 *   not narrow, scope or seed anything. An instrument that changed what it
 *   measured would not be one.
 */

/** The scheme the base — and, off a commit, the head — side is read through. */
const SCHEME = 'mezz-change';

/** What an older webview, which sends no file list at all, reads as. */
const EMPTY: ChangedFilesState = {
  active: false,
  rows: [],
  onlyInGraph: [],
  totals: { files: 0, additions: 0, deletions: 0 },
};

type ChangeNode = FileRow | ResidueGroup | ResiduePath;

interface FileRow { kind: 'file'; row: ChangedFileRowState; }
interface ResidueGroup { kind: 'residueGroup'; paths: string[]; }
interface ResiduePath { kind: 'residuePath'; path: string; }

export class ChangesTreeProvider implements vscode.TreeDataProvider<ChangeNode> {
  static readonly viewId = 'mezz.changes';

  private state: ChangedFilesState | undefined;
  private readonly _onDidChangeTreeData = new vscode.EventEmitter<ChangeNode | undefined>();
  readonly onDidChangeTreeData = this._onDidChangeTreeData.event;
  private treeView?: vscode.TreeView<ChangeNode>;

  constructor(private readonly getWorkspaceRoot: () => string | undefined) {}

  attachTreeView(view: vscode.TreeView<ChangeNode>): void {
    this.treeView = view;
    this.render();
  }

  /**
   * Take the webview's reading. An inactive state empties the view rather than
   * leaving the previous comparison's rows on screen — under a working head a
   * stale list is a list describing the previous save.
   */
  setState(state: ChangedFilesState): void {
    this.state = state.active ? state : undefined;
    this._onDidChangeTreeData.fire(undefined);
    this.render();
  }

  // ─── TreeDataProvider contract ─────────────────────────────────────────

  getTreeItem(node: ChangeNode): vscode.TreeItem {
    if (node.kind === 'file') return this.fileItem(node.row);
    if (node.kind === 'residueGroup') return residueGroupItem(node.paths);
    return residuePathItem(node.path);
  }

  getChildren(node?: ChangeNode): ChangeNode[] {
    if (!this.state) return [];
    if (!node) return this.roots(this.state);
    if (node.kind === 'residueGroup') {
      return node.paths.map((p) => ({ kind: 'residuePath', path: p }) as ResiduePath);
    }
    return [];
  }

  /** The rows, and the residue behind one collapsed node when there is any. */
  private roots(state: ChangedFilesState): ChangeNode[] {
    const rows: ChangeNode[] = state.rows.map((row) => ({ kind: 'file', row }));
    if (state.onlyInGraph.length > 0) {
      rows.push({ kind: 'residueGroup', paths: state.onlyInGraph });
    }
    return rows;
  }

  private fileItem(row: ChangedFileRowState): vscode.TreeItem {
    const item = new vscode.TreeItem(
      path.posix.basename(row.path),
      vscode.TreeItemCollapsibleState.None,
    );
    // The real file, so the theme colours the label the way it colours the
    // same path everywhere else. A deletion has no file on disk and gets no
    // uri — VS Code would draw it as missing, which it is, but the row is
    // about the change rather than about the current tree.
    const root = this.getWorkspaceRoot();
    if (root && row.status !== 'D') {
      item.resourceUri = vscode.Uri.file(path.join(root, row.path));
    }
    item.description = describeRow(row);
    item.tooltip = tooltipFor(row);
    item.iconPath = statusIcon(row);
    item.contextValue = 'mezzChangedFile';
    item.command = {
      command: 'mezz.changes.open',
      title: 'Open Change',
      arguments: [row],
    };
    return item;
  }

  /** The count on the title, and the ref pair above the rows. */
  private render(): void {
    if (!this.treeView) return;
    const state = this.state;
    if (!state) {
      this.treeView.badge = undefined;
      this.treeView.message = undefined;
      return;
    }
    const { files, additions, deletions } = state.totals;
    this.treeView.badge = files === 0
      ? undefined
      : { value: files, tooltip: `${files} changed file${files === 1 ? '' : 's'}` };
    this.treeView.message = state.rows.length === 0
      ? `Git reports no changed files for ${refPair(state)}.`
      : `${refPair(state)} · +${additions} −${deletions}`;
  }
}

/**
 * The two sides, named rather than hashed (UI-139).
 *
 * Falls back to the raw refs when the labels are absent — an older webview
 * sends none — so the header degrades to what it always said instead of to
 * nothing.
 */
function refPair(state: ChangedFilesState): string {
  const from = state.fromLabel?.text ?? state.fromRef ?? '?';
  const to = state.toLabel?.text ?? state.toRef ?? '?';
  return `${from} → ${to}`;
}

/**
 * `M · src/server · +4 −2 · 4 entities` — the letter, the directory, the
 * churn, the verdict.
 *
 * The letter leads even though the icon already carries the colour: `U` and
 * `A` are the same hue family and the same shape, and telling a file git has
 * never followed from one staged for the next commit is most of why the list
 * is open.
 */
function describeRow(row: ChangedFileRowState): string {
  const dir = path.posix.dirname(row.path);
  const parts = [row.letter, dir === '.' ? '' : dir];
  parts.push(row.binary ? 'binary' : `+${row.additions} −${row.deletions}`);
  if (row.agreement.label) parts.push(row.agreement.label);
  return parts.filter(Boolean).join('  ·  ');
}

function tooltipFor(row: ChangedFileRowState): vscode.MarkdownString {
  const md = new vscode.MarkdownString();
  md.appendMarkdown(`**${row.path}**\n\n`);
  md.appendMarkdown(row.phrase);
  if (row.oldPath) md.appendMarkdown(`\n\nwas \`${row.oldPath}\``);
  md.appendMarkdown(`\n\n${row.binary ? 'Binary.' : `+${row.additions} −${row.deletions}`}`);
  if (row.agreement.hint) md.appendMarkdown(`\n\n${row.agreement.hint}`);
  return md;
}

/**
 * The status as a colour rather than a letter.
 *
 * The theme's own git colours, so a row reads the same here as it does in the
 * Source Control view. A rename keeps the modified hue for the reason
 * `statusChange` gives on the browser end: the file survived, and giving a
 * `git mv` the green of new code would make a restructure read as a rewrite.
 */
function statusIcon(row: ChangedFileRowState): vscode.ThemeIcon {
  if (row.status === 'A') {
    return new vscode.ThemeIcon('diff-added', new vscode.ThemeColor(
      row.untracked
        ? 'gitDecoration.untrackedResourceForeground'
        : 'gitDecoration.addedResourceForeground',
    ));
  }
  if (row.status === 'D') {
    return new vscode.ThemeIcon('diff-removed', new vscode.ThemeColor('gitDecoration.deletedResourceForeground'));
  }
  if (row.status === 'R' || row.status === 'C') {
    return new vscode.ThemeIcon('diff-renamed', new vscode.ThemeColor('gitDecoration.modifiedResourceForeground'));
  }
  return new vscode.ThemeIcon('diff-modified', new vscode.ThemeColor('gitDecoration.modifiedResourceForeground'));
}

function residueGroupItem(paths: string[]): vscode.TreeItem {
  const item = new vscode.TreeItem(
    `Only in the graph (${paths.length})`,
    vscode.TreeItemCollapsibleState.Expanded,
  );
  item.iconPath = new vscode.ThemeIcon('warning', new vscode.ThemeColor('list.warningForeground'));
  item.tooltip = new vscode.MarkdownString(
    'The diff reports changed entities in files git does not list for this pair.\n\n'
    + 'Empty on a healthy comparison. When it is not, the two readings disagree '
    + 'about what changed — a diff left over from a previous comparison, or a '
    + 'path spelling that stopped matching between `diff.json` and the tree.',
  );
  return item;
}

function residuePathItem(p: string): vscode.TreeItem {
  const item = new vscode.TreeItem(p, vscode.TreeItemCollapsibleState.None);
  item.tooltip = p;
  return item;
}

// ─── Reading a side of a file ────────────────────────────────────────────

/**
 * One side of one file, through the endpoint UI-134 already built.
 *
 * The URI carries everything the fetch needs, because a
 * `TextDocumentContentProvider` is handed nothing else — VS Code caches by URI
 * and reopens documents across a window reload with no other context.
 */
function sideUri(row: ChangedFileRowState, side: 'base' | 'head', state: ChangedFilesState): vscode.Uri {
  const query = new URLSearchParams({
    side,
    from: state.fromRef ?? '',
    // `headRef`, not `toRef`: the endpoint takes the `WORKING` / `STAGED`
    // sentinels, and `diff.json`'s lowercase `working` would reach git as a
    // ref that does not resolve — a failure that would present as an empty
    // file rather than as an error.
    to: state.headRef ?? '',
  });
  if (row.oldPath) query.set('base_path', row.oldPath);
  return vscode.Uri.from({ scheme: SCHEME, path: `/${row.path}`, query: query.toString() });
}

export class ChangeContentProvider implements vscode.TextDocumentContentProvider {
  constructor(private readonly getServerPort: () => number | undefined) {}

  async provideTextDocumentContent(uri: vscode.Uri): Promise<string> {
    const port = this.getServerPort();
    if (!port) return '';
    const params = new URLSearchParams(uri.query);
    const body = {
      from_ref: params.get('from') ?? '',
      // Already mapped by `headRefFor` on the webview end, so what arrives
      // here is the `WORKING` / `STAGED` / sha the endpoint takes.
      to_ref: params.get('to') ?? '',
      path: uri.path.replace(/^\//, ''),
      base_path: params.get('base_path') ?? undefined,
    };
    try {
      const answer = await postJson<FileDiffResponse>(port, '/api/file-diff', body);
      const side = params.get('side') === 'base' ? answer.base : answer.head;
      // A side that does not exist — an addition's base, a deletion's head —
      // opens as an empty document rather than failing, which is what
      // `git.openChange` does for the same two cases.
      return side?.text ?? '';
    } catch {
      return '';
    }
  }
}

interface FileDiffResponse {
  base?: { text: string; binary: boolean; truncated: boolean };
  head?: { text: string; binary: boolean; truncated: boolean };
  binary: boolean;
}

/**
 * Open one row as a native diff editor.
 *
 * `vscode.diff` and not a webview: the folded gaps, the inline navigation, the
 * gutter actions and the change ruler are all already built, and under a
 * working head the right-hand side is the real file — editable, which is the
 * point, because that is the diff still being worked in.
 */
async function openChange(
  row: ChangedFileRowState,
  getState: () => ChangedFilesState | undefined,
  getWorkspaceRoot: () => string | undefined,
): Promise<void> {
  const state = getState();
  if (!state) return;
  if (row.binary) {
    void vscode.window.showInformationMessage(`${row.path} is binary — nothing to diff.`);
    return;
  }
  const root = getWorkspaceRoot();
  const onDisk = root && state.toRef === 'working' && row.status !== 'D'
    ? vscode.Uri.file(path.join(root, row.path))
    : undefined;
  // The bare refs here, not the named pair: this is an editor tab, and a
  // commit subject in it would push the filename — the only part that tells
  // one open diff from another — off the end.
  const title = `${path.posix.basename(row.path)} (${state.fromRef ?? '?'} ↔ ${state.toRef ?? '?'})`;
  await vscode.commands.executeCommand(
    'vscode.diff',
    sideUri(row, 'base', state),
    onDisk ?? sideUri(row, 'head', state),
    title,
  );
}

/**
 * Everything this view needs registered, behind one call.
 *
 * `activate` is already an Overfull Head — cyclomatic 64 over 729 lines — and
 * the complexity gate is diff-only on the functions a change touches, so the
 * tree, the content provider, the command and the bridge subscription are
 * wired here rather than four more statements there.
 */
export function registerChangesView(
  context: vscode.ExtensionContext,
  getServerPort: () => number | undefined,
  getWorkspaceRoot: () => string | undefined,
): ChangesTreeProvider {
  const provider = new ChangesTreeProvider(getWorkspaceRoot);
  const view = vscode.window.createTreeView(ChangesTreeProvider.viewId, {
    treeDataProvider: provider,
  });
  provider.attachTreeView(view);

  let state: ChangedFilesState | undefined;
  context.subscriptions.push(
    view,
    vscode.workspace.registerTextDocumentContentProvider(
      SCHEME,
      new ChangeContentProvider(getServerPort),
    ),
    vscode.commands.registerCommand(
      'mezz.changes.open',
      (row: ChangedFileRowState) => openChange(row, () => state, getWorkspaceRoot),
    ),
    // The file list rides inside `DiffState` — see `ChangedFilesState` for
    // why it is not a message of its own.
    VisualizerPanel.onDiffChanged((diff) => {
      const next = diff.changedFiles ?? EMPTY;
      state = next.active ? next : undefined;
      provider.setState(next);
      // Drives the view's `viewsWelcome`, so an empty pane says why it is
      // empty instead of drawing a fake row that says so.
      void vscode.commands.executeCommand('setContext', 'mezz.hasChanges', !!state);
    }),
  );
  return provider;
}

function postJson<T>(port: number, apiPath: string, body: unknown, timeoutMs = 8000): Promise<T> {
  const payload = JSON.stringify(body);
  return new Promise((resolve, reject) => {
    const req = http.request(
      {
        host: '127.0.0.1',
        port,
        path: apiPath,
        method: 'POST',
        timeout: timeoutMs,
        headers: {
          'Content-Type': 'application/json',
          'Content-Length': Buffer.byteLength(payload),
        },
      },
      (res) => {
        if (res.statusCode !== 200) {
          reject(new Error(`HTTP ${res.statusCode}`));
          res.resume();
          return;
        }
        let text = '';
        res.setEncoding('utf-8');
        res.on('data', (chunk) => (text += chunk));
        res.on('end', () => {
          try {
            resolve(JSON.parse(text));
          } catch (e) {
            reject(e);
          }
        });
      },
    );
    req.on('error', reject);
    req.on('timeout', () => {
      req.destroy();
      reject(new Error('timeout'));
    });
    req.end(payload);
  });
}
