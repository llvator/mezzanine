import * as vscode from 'vscode';
import { MezzServer } from './server';
import { resolveMezzBinary, startupFailureMessage } from './mezzBinary';
import { VisualizerPanel } from './panel';
import { SelectionViewProvider } from './sideViewProvider';
import type { SelectionPayload } from './sideViewProvider';
import { DescriptionViewProvider } from './descriptionViewProvider';
import { ScopeTreeProvider } from './scopeTreeProvider';
import { ControlsViewProvider } from './controlsViewProvider';
import { QualityViewProvider } from './qualityViewProvider';
import { DiffViewProvider } from './diffViewProvider';
import { EducatorHoverProvider } from './educatorHoverProvider';
import { EducatorViewProvider } from './educatorViewProvider';
import { EducatorProblemsView } from './educatorProblemsViewProvider';
import * as http from 'http';
import * as path from 'path';
import * as fs from 'fs';

let server: MezzServer | undefined;

/** Per-workspace override for the analyzed/git root, chosen via
 *  "Mezzanine: Select Git Repository…". Needed when the git repository is a
 *  subfolder of the opened workspace: git commands walk *up* from the
 *  analyzed root, so a repo *below* the workspace root is invisible and
 *  the Diff panel fails. Persisted in workspaceState across reloads. */
let projectRootOverride: string | undefined;

/** Memento key persisting `projectRootOverride`. */
const PROJECT_ROOT_KEY = 'mezz.projectRoot';

/** Toggle: when true, selecting a node auto-opens the file at its line. */
let followSelection = false;

/** Toggle: when true, the analysis-scope tree (and the visual scope) track the
 *  active editor — file or folder depending on `mezz.analysisScopeFollowMode`.
 *  Distinct from `mezz.autoVisualize`, which only narrows the visual scope. */
let followActiveAnalysisScope = false;

/** Memento key persisting `followActiveAnalysisScope` across reloads. */
const FOLLOW_STATE_KEY = 'mezz.analysisScopes.followActive';

/** Timestamp (ms) until which editor-change events should be ignored to
 *  prevent ping-pong when we programmatically open a file from a node click. */
let suppressEditorEventsUntil = 0;

export function activate(context: vscode.ExtensionContext) {
  const outputChannel = vscode.window.createOutputChannel('Mezzanine Code Visualizer');
  // Restore the user's project-root choice; drop it if the folder is gone.
  projectRootOverride = context.workspaceState.get<string>(PROJECT_ROOT_KEY);
  if (projectRootOverride && !fs.existsSync(projectRootOverride)) {
    projectRootOverride = undefined;
    void context.workspaceState.update(PROJECT_ROOT_KEY, undefined);
  }
  // Educator content (rules + lessons) ships with the extension under
  // `<extensionPath>/content/`. The watch server uses it as a fallback so the
  // educator works in any workspace, not just the mezz repo itself.
  const bundledContentPath = path.join(context.extensionPath, 'content');
  const selectionProvider = new SelectionViewProvider(context.extensionUri);
  const descriptionProvider = new DescriptionViewProvider(context.extensionUri);
  const scopeTreeProvider = new ScopeTreeProvider();
  // Analysis scope defaults to the root — "analyze the whole repo" — so
  // Quality works out of the box with meaningful metrics before the user
  // does anything else.
  const analysisScopeTreeProvider = new ScopeTreeProvider(['']);
  const controlsProvider = new ControlsViewProvider();
  const qualityProvider = new QualityViewProvider();
  const diffProvider = new DiffViewProvider();

  context.subscriptions.push(
    vscode.window.registerWebviewViewProvider(
      SelectionViewProvider.viewType,
      selectionProvider
    ),
    vscode.window.registerWebviewViewProvider(
      DescriptionViewProvider.viewType,
      descriptionProvider
    ),
    vscode.window.registerWebviewViewProvider(
      ControlsViewProvider.viewType,
      controlsProvider
    ),
    vscode.window.registerWebviewViewProvider(
      QualityViewProvider.viewType,
      qualityProvider
    ),
    vscode.window.registerWebviewViewProvider(
      DiffViewProvider.viewType,
      diffProvider
    )
  );

  // Scope tree ↔ main panel bidirectional sync. Checkbox clicks flow via
  // scopeTreeProvider.onSelectionChanged → setScopes. External scope changes
  // (e.g. "Scope to changes") flow back here via scopesChanged.
  context.subscriptions.push(
    VisualizerPanel.onScopesChanged((paths) => scopeTreeProvider.setSelection(paths)),
    VisualizerPanel.onAnalysisScopesChanged((paths) => analysisScopeTreeProvider.setSelection(paths))
  );

  // Diff state broker + commit picker
  context.subscriptions.push(
    VisualizerPanel.onDiffChanged((state) => diffProvider.update(state)),
    // The branch the graph is, reported separately because it is true whether
    // or not a diff is loaded (UI-114).
    VisualizerPanel.onBranchChanged((label) => diffProvider.updateBranch(label))
  );
  diffProvider.onCurrentChanges(() => {
    VisualizerPanel.currentPanel?.sendCommand('triggerDiff', { fromRef: 'HEAD', toRef: 'WORKING' });
  });
  diffProvider.onPickCommits(async () => {
    const s = server;
    if (!s) {
      vscode.window.showWarningMessage('Mezzanine: start the visualizer first (Mezzanine: Open Code Visualizer).');
      return;
    }
    await pickAndTriggerDiff(s.port);
  });

  // Filter state ↔ main panel broker
  context.subscriptions.push(
    VisualizerPanel.onFiltersChanged((state) => controlsProvider.updateFilters(state)),
    VisualizerPanel.onLevelFiltersChanged((state) => controlsProvider.updateLevelFilters(state))
  );
  context.subscriptions.push(
    vscode.commands.registerCommand(
      'mezz.internalFilterCommand',
      (args: { command: string; value: unknown }) => {
        VisualizerPanel.currentPanel?.sendCommand(args.command, args.value);
      }
    )
  );

  // Quality view ↔ main panel: rows in, clicks out
  context.subscriptions.push(
    VisualizerPanel.onQualityChanged((rows) => qualityProvider.update(rows))
  );
  qualityProvider.onSelectEntity((entityId) => {
    VisualizerPanel.currentPanel?.sendCommand('selectEntityById', entityId);
  });
  qualityProvider.onGoToSource(async (file, line) => {
    await openAtLine(resolveWorkspacePath(file), line);
  });

  // Forward every click in the Controls view to the main panel, except for
  // commands that configure extension-side behaviour.
  controlsProvider.onCommand((command, value) => {
    if (command === 'setFollowSelection') {
      followSelection = !!value;
      controlsProvider.setFollowSelection(followSelection);
      return;
    }
    VisualizerPanel.currentPanel?.sendCommand(command, value);
  });

  const scopeTreeView = vscode.window.createTreeView('mezz.scopes', {
    treeDataProvider: scopeTreeProvider,
    showCollapseAll: true,
    canSelectMany: false,
    manageCheckboxStateManually: true,
  });
  context.subscriptions.push(scopeTreeView);

  const analysisScopeTreeView = vscode.window.createTreeView('mezz.analysisScopes', {
    treeDataProvider: analysisScopeTreeProvider,
    showCollapseAll: true,
    canSelectMany: false,
    manageCheckboxStateManually: true,
  });
  context.subscriptions.push(analysisScopeTreeView);

  // Checkbox changes → update tree state and push the selection to the
  // main panel so the graph re-renders / analysis re-runs with the new scope.
  context.subscriptions.push(
    scopeTreeView.onDidChangeCheckboxState((e) => {
      scopeTreeProvider.applyCheckboxChanges(e.items);
    }),
    analysisScopeTreeView.onDidChangeCheckboxState((e) => {
      analysisScopeTreeProvider.applyCheckboxChanges(e.items);
    })
  );

  scopeTreeProvider.onSelectionChanged((paths) => {
    VisualizerPanel.currentPanel?.setScopes(paths);
  });
  analysisScopeTreeProvider.onSelectionChanged((paths) => {
    VisualizerPanel.currentPanel?.sendCommand('setAnalysisScopes', paths);
  });

  // Tree view title-bar commands
  context.subscriptions.push(
    vscode.commands.registerCommand('mezz.scopes.refresh', () =>
      scopeTreeProvider.refresh()
    ),
    vscode.commands.registerCommand('mezz.scopes.selectAll', () =>
      scopeTreeProvider.selectAll()
    ),
    vscode.commands.registerCommand('mezz.scopes.clear', () =>
      scopeTreeProvider.clear()
    ),
    vscode.commands.registerCommand('mezz.analysisScopes.refresh', () =>
      analysisScopeTreeProvider.refresh()
    ),
    vscode.commands.registerCommand('mezz.analysisScopes.selectAll', () =>
      analysisScopeTreeProvider.selectAll()
    ),
    vscode.commands.registerCommand('mezz.analysisScopes.clear', () =>
      analysisScopeTreeProvider.clear()
    ),
    // Widen: replace each selected path with its own parent folder. Pairs
    // with "Visualize Current File" — start from one file, grow the scope
    // one level per click.
    vscode.commands.registerCommand('mezz.scopes.extendToParent', () =>
      scopeTreeProvider.extendToParents()
    ),
    vscode.commands.registerCommand('mezz.analysisScopes.extendToParent', () =>
      analysisScopeTreeProvider.extendToParents()
    ),
    // Tree filtering (substring on the full path) + fuzzy QuickPick — the
    // fast paths for scoping in large repos where checkbox navigation is slow.
    vscode.commands.registerCommand('mezz.scopes.filter', () =>
      promptScopeFilter(scopeTreeProvider, scopeTreeView, 'mezz.scopesFiltered')
    ),
    vscode.commands.registerCommand('mezz.scopes.clearFilter', () =>
      applyScopeFilter(scopeTreeProvider, scopeTreeView, 'mezz.scopesFiltered', '')
    ),
    vscode.commands.registerCommand('mezz.analysisScopes.filter', () =>
      promptScopeFilter(analysisScopeTreeProvider, analysisScopeTreeView, 'mezz.analysisScopesFiltered')
    ),
    vscode.commands.registerCommand('mezz.analysisScopes.clearFilter', () =>
      applyScopeFilter(analysisScopeTreeProvider, analysisScopeTreeView, 'mezz.analysisScopesFiltered', '')
    ),
    vscode.commands.registerCommand('mezz.scopes.pick', () =>
      pickScopes(scopeTreeProvider, 'Mezzanine: Pick Visual Scopes')
    ),
    vscode.commands.registerCommand('mezz.analysisScopes.pick', () =>
      pickScopes(analysisScopeTreeProvider, 'Mezzanine: Pick Analysis Scopes')
    ),
    vscode.commands.registerCommand('mezz.analysisScopes.followActive', async () => {
      await setFollowActive(context, true);
      // Apply immediately so the user sees the scope narrow without
      // having to switch editors first.
      applyFollow(analysisScopeTreeProvider, vscode.window.activeTextEditor);
    }),
    vscode.commands.registerCommand('mezz.analysisScopes.unfollowActive', async () => {
      // Per spec: turning follow off leaves both scopes wherever the
      // editor last placed them. No restore of prior selection.
      await setFollowActive(context, false);
    }),
    vscode.commands.registerCommand('mezz.analysisScopes.followFile', async () => {
      await setFollowMode('file');
      if (followActiveAnalysisScope) {
        applyFollow(analysisScopeTreeProvider, vscode.window.activeTextEditor);
      }
    }),
    vscode.commands.registerCommand('mezz.analysisScopes.followFolder', async () => {
      await setFollowMode('folder');
      if (followActiveAnalysisScope) {
        applyFollow(analysisScopeTreeProvider, vscode.window.activeTextEditor);
      }
    })
  );

  // Restore persisted follow state and seed both context keys so the
  // title-bar icons render correctly on activation.
  followActiveAnalysisScope = context.workspaceState.get<boolean>(FOLLOW_STATE_KEY, false);
  void vscode.commands.executeCommand(
    'setContext',
    'mezz.followActiveAnalysisScope',
    followActiveAnalysisScope
  );
  void vscode.commands.executeCommand(
    'setContext',
    'mezz.analysisScopeFollowModeIsFile',
    getFollowMode() === 'file'
  );
  // If follow was on from a previous session, seed the scope from the
  // current editor now. The tree's setSelection is no-op-safe before the
  // index loads; the right checkbox renders once the index arrives.
  if (followActiveAnalysisScope) {
    applyFollow(analysisScopeTreeProvider, vscode.window.activeTextEditor);
  }
  // Keep the file/folder context key in sync if the setting is edited
  // directly (Settings UI / settings.json).
  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration((e) => {
      if (e.affectsConfiguration('mezz.analysisScopeFollowMode')) {
        void vscode.commands.executeCommand(
          'setContext',
          'mezz.analysisScopeFollowModeIsFile',
          getFollowMode() === 'file'
        );
        if (followActiveAnalysisScope) {
          applyFollow(analysisScopeTreeProvider, vscode.window.activeTextEditor);
        }
      }
    })
  );

  async function setFollowActive(ctx: vscode.ExtensionContext, on: boolean): Promise<void> {
    followActiveAnalysisScope = on;
    await ctx.workspaceState.update(FOLLOW_STATE_KEY, on);
    await vscode.commands.executeCommand(
      'setContext',
      'mezz.followActiveAnalysisScope',
      on
    );
  }

  async function setFollowMode(mode: 'file' | 'folder'): Promise<void> {
    await vscode.workspace
      .getConfiguration('mezz')
      .update('analysisScopeFollowMode', mode, vscode.ConfigurationTarget.Workspace);
    // The onDidChangeConfiguration listener above will update the
    // context key. Apply happens at the command call site.
  }

  // Broker: when the main panel reports a selection change, push it to the
  // native side views. Also handles the reverse case where a side view
  // asks to jump to source.
  context.subscriptions.push(
    VisualizerPanel.onSelectionChanged(async (payload) => {
      selectionProvider.updateSelection(payload);
      controlsProvider.setSelection(payload);

      // Auto-open the file at its line when "follow selection" is on.
      if (followSelection && payload?.filePath && isOpenableSelection(payload)) {
        const absPath = resolveWorkspacePath(payload.filePath);
        // Suppress the resulting active-editor/cursor change events for a
        // short window so they don't bounce back into the graph as a
        // focusFile/focusCursor.
        suppressEditorEventsUntil = Date.now() + 800;
        try {
          await openAtLine(absPath, payload.line, { preserveFocus: true });
        } catch (err) {
          // Nothing opened, so nothing will echo back — drop the window
          // rather than swallowing 800ms of the user's real editor events
          // for a click that did nothing.
          suppressEditorEventsUntil = 0;
          outputChannel.appendLine(
            `[follow] could not open ${absPath}:${payload.line} — ${(err as Error).message}`
          );
        }
      }
    })
  );

  // Broker: the hovered (or selected) node's description chain → the native
  // Description view. Hover is high-frequency, so the webview debounces
  // before posting; nothing here needs to throttle again.
  context.subscriptions.push(
    VisualizerPanel.onDescriptionChanged((payload) => {
      descriptionProvider.update(payload);
    })
  );

  // Drill-in from the native Selection side view: forward the request to
  // the main visualizer so it narrows the visual scope and re-picks the
  // aggregation level (same path a double-click on the canvas takes).
  context.subscriptions.push(
    SelectionViewProvider.onDrillIn((path) => {
      VisualizerPanel.currentPanel?.drillIn(path);
    })
  );

  // Internal command used by the side view's "Go to Source" button.
  context.subscriptions.push(
    vscode.commands.registerCommand(
      'mezz.internalGoToDefinition',
      async (args: { filePath: string; line?: number }) => {
        await openAtLine(resolveWorkspacePath(args.filePath), args.line);
      }
    )
  );

  const onServerReady = (s: MezzServer) => {
    selectionProvider.setServerPort(s.port);
    qualityProvider.setServerPort(s.port);
    scopeTreeProvider.setServerPort(s.port);
    analysisScopeTreeProvider.setServerPort(s.port);
    // Load the scope index as soon as the server is ready
    void scopeTreeProvider.refresh();
    void analysisScopeTreeProvider.refresh();
  };

  // Command: open the visualizer panel (starts server if needed)
  context.subscriptions.push(
    vscode.commands.registerCommand('mezz.openVisualizer', async () => {
      const workspaceRoot = getProjectRoot();
      if (!workspaceRoot) {
        vscode.window.showErrorMessage('Mezzanine: Open a folder or workspace first.');
        return;
      }

      server = await ensureServer(context, workspaceRoot, outputChannel, bundledContentPath, onServerReady);
      if (!server) return;

      const panel = VisualizerPanel.createOrShow(context, server.port, workspaceRoot);
      // Push the active editor's file so Quality's "Current file"
      // analysis option is enabled from the start, without requiring
      // the user to click around to trigger an editor-change event.
      const editor = vscode.window.activeTextEditor;
      if (editor) panel.setCurrentEditorFile(editor.document.uri.fsPath);
    })
  );

  // Command: visualize the currently active file in a compact (no-sidebar) view
  context.subscriptions.push(
    vscode.commands.registerCommand('mezz.visualizeCurrentFile', async () => {
      const workspaceRoot = getProjectRoot();
      if (!workspaceRoot) {
        vscode.window.showErrorMessage('Mezzanine: Open a folder or workspace first.');
        return;
      }

      server = await ensureServer(context, workspaceRoot, outputChannel, bundledContentPath, onServerReady);
      if (!server) return;

      const panel = VisualizerPanel.createOrShow(context, server.port, workspaceRoot);

      const editor = vscode.window.activeTextEditor;
      if (editor) {
        // setCurrentEditorFile always runs; focusFile is the stronger
        // scope-and-select action tied to this specific command.
        panel.setCurrentEditorFile(editor.document.uri.fsPath);
        panel.focusFile(
          editor.document.uri.fsPath,
          editor.selection.active.line + 1
        );
      }
    })
  );

  // Command: stop the running mezz server (and close the visualizer, since
  // its webview is pinned to the now-dead port).
  context.subscriptions.push(
    vscode.commands.registerCommand('mezz.stopServer', () => {
      if (!server) {
        vscode.window.showInformationMessage('Mezzanine: Server is not running.');
        return;
      }
      server.stop();
      server = undefined;
      VisualizerPanel.dispose();
      vscode.window.showInformationMessage('Mezzanine: Server stopped.');
    })
  );

  // Command: restart the mezz server. Disposes the visualizer panel because
  // its serverPort is captured at construction time and would otherwise
  // point at the old process; reopens it when the new server is ready.
  context.subscriptions.push(
    vscode.commands.registerCommand('mezz.restartServer', async () => {
      const workspaceRoot = getProjectRoot();
      if (!workspaceRoot) {
        vscode.window.showErrorMessage('Mezzanine: Open a folder or workspace first.');
        return;
      }
      const hadPanel = VisualizerPanel.currentPanel !== undefined;
      if (server) {
        server.stop();
        server = undefined;
      }
      VisualizerPanel.dispose();
      server = await ensureServer(context, workspaceRoot, outputChannel, bundledContentPath, onServerReady);
      if (!server) return;
      if (hadPanel) {
        const panel = VisualizerPanel.createOrShow(context, server.port, workspaceRoot);
        const editor = vscode.window.activeTextEditor;
        if (editor) panel.setCurrentEditorFile(editor.document.uri.fsPath);
      }
      vscode.window.showInformationMessage('Mezzanine: Server restarted.');
    })
  );

  // Command: pick the git repository (= project root) Mezzanine works against.
  // Covers the "workspace root is not the git repo" case: the repo lives in
  // a subfolder, git commands at the workspace root fail, and the Diff panel
  // can't list commits or build worktrees.
  context.subscriptions.push(
    vscode.commands.registerCommand('mezz.selectGitRepo', async () => {
      const repos = discoverGitRepos();
      const current = getProjectRoot();
      type RepoItem = vscode.QuickPickItem & { repoPath?: string; browse?: boolean };
      const items: RepoItem[] = repos.map((r) => ({
        label: `$(repo) ${path.basename(r)}`,
        description: r === current ? 'current' : undefined,
        detail: r,
        repoPath: r,
      }));
      items.push({
        label: '$(folder-opened) Browse…',
        detail: 'Pick a folder not listed above',
        browse: true,
      });

      const pick = await vscode.window.showQuickPick(items, {
        placeHolder: repos.length
          ? 'Select the git repository Mezzanine should analyze and diff against'
          : 'No git repositories found in the workspace — browse to one',
        matchOnDetail: true,
      });
      if (!pick) return;

      let chosen = pick.repoPath;
      if (pick.browse) {
        const uris = await vscode.window.showOpenDialog({
          canSelectFiles: false,
          canSelectFolders: true,
          canSelectMany: false,
          openLabel: 'Use as Mezzanine project root',
        });
        chosen = uris?.[0]?.fsPath;
      }
      if (!chosen || chosen === current) return;

      if (!fs.existsSync(path.join(chosen, '.git'))) {
        const proceed = await vscode.window.showWarningMessage(
          `Mezzanine: "${path.basename(chosen)}" is not a git repository — the Diff panel needs one. Use it anyway?`,
          'Use Anyway',
          'Cancel'
        );
        if (proceed !== 'Use Anyway') return;
      }

      projectRootOverride = chosen;
      await context.workspaceState.update(PROJECT_ROOT_KEY, chosen);

      if (!server?.running) {
        vscode.window.showInformationMessage(
          `Mezzanine: project root set to ${chosen}. It will be used when the visualizer starts.`
        );
        return;
      }

      // Server is live: re-root it in place. /api/root re-analyzes and swaps
      // the git repo_root atomically, then broadcasts a reload to webviews;
      // the native scope trees need an explicit refresh.
      const port = server.port;
      await vscode.window.withProgress(
        {
          location: vscode.ProgressLocation.Notification,
          title: `Mezzanine: re-analyzing ${path.basename(chosen)}…`,
        },
        async () => {
          try {
            const resp = await postSetRoot(port, chosen);
            if (resp.success) {
              void scopeTreeProvider.refresh();
              void analysisScopeTreeProvider.refresh();
              vscode.window.showInformationMessage(
                `Mezzanine: now using ${chosen}${resp.message ? ` — ${resp.message}` : ''}`
              );
            } else {
              vscode.window.showErrorMessage(
                `Mezzanine: failed to change project root — ${resp.message ?? 'unknown error'}`
              );
            }
          } catch (err) {
            vscode.window.showErrorMessage(
              `Mezzanine: failed to change project root — ${(err as Error).message}`
            );
          }
        }
      );
    })
  );

  // Track active editor changes
  context.subscriptions.push(
    vscode.window.onDidChangeActiveTextEditor((editor) => {
      if (!editor) return;
      // Always track the current file for the Quality "Current file" option,
      // regardless of autoVisualize/follow or the suppression window — this
      // update is scope-free and has no visible side effects on the graph.
      VisualizerPanel.currentPanel?.setCurrentEditorFile(editor.document.uri.fsPath);

      // Follow runs even when the visualizer panel isn't open: it still
      // narrows the analysis-scope tree, which is the user-visible signal
      // they care about for cheap-analysis mode on large repos.
      if (followActiveAnalysisScope && Date.now() >= suppressEditorEventsUntil) {
        applyFollow(analysisScopeTreeProvider, editor);
      }

      if (!VisualizerPanel.currentPanel) return;
      if (Date.now() < suppressEditorEventsUntil) return;
      const config = vscode.workspace.getConfiguration('mezz');
      if (!config.get<boolean>('autoVisualize', true)) return;

      VisualizerPanel.currentPanel.focusFile(
        editor.document.uri.fsPath,
        editor.selection.active.line + 1
      );
    })
  );

  // Track cursor position changes (debounced) to keep the selected node in
  // sync with where the user's cursor is in the editor.
  let cursorDebounce: NodeJS.Timeout | undefined;
  context.subscriptions.push(
    vscode.window.onDidChangeTextEditorSelection((event) => {
      // Ignore selections in non-file editors (Output, Debug Console,
      // git diff views, etc.) — they produce noise and can never match a
      // workspace file anyway.
      if (event.textEditor.document.uri.scheme !== 'file') return;
      const rawLine = event.selections[0]?.active.line;
      const rawPath = event.textEditor.document.uri.fsPath;
      outputChannel.appendLine(
        `[cursor-sync] selection changed: ${rawPath}:${rawLine === undefined ? '?' : rawLine + 1}`
      );
      if (Date.now() < suppressEditorEventsUntil) {
        outputChannel.appendLine(
          `[cursor-sync] SUPPRESSED (within ${suppressEditorEventsUntil - Date.now()}ms suppression window)`
        );
        return;
      }
      // While the user has a non-empty selection (actively highlighting or
      // holding an existing highlight), don't fire cursor-follow. Otherwise
      // the graph re-selects an entity → "follow selection" pushes the
      // cursor back to that entity's definition line → the user's highlight
      // is discarded and drag-select becomes unusable. Also cancel any
      // pending debounce from the click that started the drag, so nothing
      // fires after the user releases.
      const primary = event.selections[0];
      if (primary && !primary.isEmpty) {
        if (cursorDebounce) {
          clearTimeout(cursorDebounce);
          cursorDebounce = undefined;
        }
        outputChannel.appendLine(
          `[cursor-sync] SKIPPED — selection is non-empty (user is highlighting)`
        );
        return;
      }
      if (!VisualizerPanel.currentPanel) {
        outputChannel.appendLine(`[cursor-sync] SKIPPED — no visualizer panel open`);
        return;
      }
      const config = vscode.workspace.getConfiguration('mezz');
      if (!config.get<boolean>('autoVisualize', true)) {
        outputChannel.appendLine(`[cursor-sync] SKIPPED — mezz.autoVisualize is false`);
        return;
      }

      if (cursorDebounce) clearTimeout(cursorDebounce);
      cursorDebounce = setTimeout(() => {
        const filePath = event.textEditor.document.uri.fsPath;
        const line = event.selections[0]?.active.line;
        if (line === undefined) {
          outputChannel.appendLine(`[cursor-sync] debounce fired but no active line`);
          return;
        }
        outputChannel.appendLine(
          `[cursor-sync] debounce fired → focusCursor(${filePath}, ${line + 1})`
        );
        VisualizerPanel.currentPanel?.focusCursor(filePath, line + 1);
      }, 150);
    })
  );

  // Handle go-to-definition from the main webview (node click)
  context.subscriptions.push(
    VisualizerPanel.onGoToDefinition((location) =>
      openAtLine(location.filePath, location.line)
    )
  );

  // Clean up server on workspace folder change
  context.subscriptions.push(
    vscode.workspace.onDidChangeWorkspaceFolders(async () => {
      if (server) {
        server.stop();
        server = undefined;
      }
    })
  );

  // Educator hover provider — surfaces Mezzanine educator rules on the editor.
  // The provider is registered eagerly; it self-gates on the
  // `mezz.educator.hoverEnabled` setting and the server's running state,
  // and returns no contribution when zero rules match (design Q11).
  const educatorHover = new EducatorHoverProvider(
    () => (server?.running ? server.port : undefined),
    () => getProjectRoot()
  );
  context.subscriptions.push(
    vscode.languages.registerHoverProvider({ language: 'java' }, educatorHover)
  );

  // Educator sidebar mirror — same content as the hover but tracks the cursor
  // in a webview panel. The view's own visibility is the only gate: when the
  // user expands the Educator row, the panel tracks; when it's collapsed,
  // scheduleUpdate exits silently. No setting to toggle.
  const educatorView = new EducatorViewProvider(
    () => (server?.running ? server.port : undefined),
    () => getProjectRoot(),
    outputChannel
  );
  context.subscriptions.push(
    vscode.window.registerWebviewViewProvider(EducatorViewProvider.viewType, educatorView)
  );

  // Cursor & active-editor changes always reach the view provider; it
  // self-gates on `webviewView.visible` so the listeners stay simple.
  context.subscriptions.push(
    vscode.window.onDidChangeActiveTextEditor((editor) => {
      if (!editor) return;
      educatorView.scheduleUpdate(editor);
    }),
    vscode.window.onDidChangeTextEditorSelection((event) => {
      if (event.textEditor.document.uri.scheme !== 'file') return;
      educatorView.scheduleUpdate(event.textEditor);
    })
  );

  // Educator Problems — dedicated TreeView under the Mezzanine sidebar that lists
  // scan hits per file. Replaces the earlier DiagnosticCollection integration
  // so Educator findings stay out of the shared Problems view (which would
  // otherwise mix them with Java/SonarQube/compiler diagnostics).
  const educatorProblems = new EducatorProblemsView(
    () => (server?.running ? server.port : undefined),
    () => getProjectRoot(),
    outputChannel
  );
  // createTreeView (not registerTreeDataProvider) — we need the TreeView
  // handle to drive `.badge`, the small counter shown next to the panel
  // title that surfaces the active file's hit count.
  const educatorProblemsView = vscode.window.createTreeView(EducatorProblemsView.viewId, {
    treeDataProvider: educatorProblems,
  });
  educatorProblems.attachTreeView(educatorProblemsView);
  // Seed the badge from whatever editor is active at activation time.
  educatorProblems.setActiveUri(activeJavaUri(vscode.window.activeTextEditor));
  context.subscriptions.push(educatorProblemsView);
  context.subscriptions.push(
    vscode.workspace.onDidOpenTextDocument((doc) => {
      void educatorProblems.refresh(doc);
    }),
    vscode.workspace.onDidSaveTextDocument((doc) => {
      void educatorProblems.refresh(doc);
    }),
    vscode.workspace.onDidCloseTextDocument((doc) => {
      educatorProblems.clear(doc);
    }),
    vscode.window.onDidChangeActiveTextEditor((editor) => {
      educatorProblems.setActiveUri(activeJavaUri(editor));
    })
  );
  for (const doc of vscode.workspace.textDocuments) {
    void educatorProblems.refresh(doc);
  }
  context.subscriptions.push(
    vscode.commands.registerCommand('mezz.educator.refreshProblems', () => {
      for (const doc of vscode.workspace.textDocuments) {
        void educatorProblems.refresh(doc);
      }
    }),
    // Click handler for tree-item navigation — used by the `command` field
    // on each hit's `TreeItem`. Opens the file and selects the hit range.
    vscode.commands.registerCommand(
      'mezz.educator.openHit',
      async (args: { file: string; line: number; col: number }) => {
        const uri = vscode.Uri.file(args.file);
        const pos = new vscode.Position(args.line, args.col);
        const range = new vscode.Range(pos, pos);
        await vscode.window.showTextDocument(uri, {
          selection: range,
          preserveFocus: false,
        });
      }
    )
  );

  outputChannel.appendLine('Mezzanine Code Visualizer extension activated.');
}

export function deactivate() {
  if (server) {
    server.stop();
    server = undefined;
  }
  VisualizerPanel.dispose();
}

/** Effective project root: the git repository the user selected via
 *  "Mezzanine: Select Git Repository…" when set, else the first workspace folder.
 *  The server analyzes and runs git at this root, and every path it returns
 *  is relative to it — so all path resolution must go through here too. */
function getProjectRoot(): string | undefined {
  if (projectRootOverride) return projectRootOverride;
  const folders = vscode.workspace.workspaceFolders;
  return folders?.[0]?.uri.fsPath;
}

/** Directories never worth descending into when scanning for git repos. */
const REPO_SCAN_SKIP = new Set([
  'node_modules', 'target', 'dist', 'out', 'build', 'vendor',
  'venv', '.venv', '__pycache__',
]);

/** Git repositories in the workspace: each workspace folder that is itself
 *  a repo, plus subfolders (up to `maxDepth` levels down) containing a
 *  `.git` entry. `.git` may be a file (worktrees, submodules), so test
 *  existence rather than directory-ness. Found repos are not descended
 *  into — nested repos below another repo are out of scope. */
function discoverGitRepos(maxDepth = 3): string[] {
  const found = new Set<string>();
  const scan = (dir: string, depth: number) => {
    try {
      if (fs.existsSync(path.join(dir, '.git'))) {
        found.add(dir);
        return;
      }
      if (depth === 0) return;
      for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
        if (!entry.isDirectory()) continue;
        if (entry.name.startsWith('.') || REPO_SCAN_SKIP.has(entry.name)) continue;
        scan(path.join(dir, entry.name), depth - 1);
      }
    } catch {
      // Unreadable directory — skip it.
    }
  };
  for (const folder of vscode.workspace.workspaceFolders ?? []) {
    scan(folder.uri.fsPath, maxDepth);
  }
  return [...found].sort();
}

/** POST /api/root — re-root the running server's analysis + git repo. */
function postSetRoot(
  port: number,
  newPath: string
): Promise<{ success: boolean; message?: string }> {
  return new Promise((resolve, reject) => {
    const body = JSON.stringify({ path: newPath });
    const req = http.request(
      {
        host: '127.0.0.1', port,
        path: '/api/root',
        method: 'POST',
        // Re-rooting re-analyzes the whole codebase; give it time.
        timeout: 300000,
        headers: {
          'Content-Type': 'application/json',
          'Content-Length': Buffer.byteLength(body),
        },
      },
      (res) => {
        let data = '';
        res.setEncoding('utf-8');
        res.on('data', (chunk) => (data += chunk));
        res.on('end', () => {
          if (res.statusCode !== 200) {
            reject(new Error(data || `HTTP ${res.statusCode}`));
            return;
          }
          try { resolve(JSON.parse(data)); } catch (e) { reject(e as Error); }
        });
      }
    );
    req.on('error', reject);
    req.on('timeout', () => { req.destroy(); reject(new Error('timeout')); });
    req.end(body);
  });
}

/** URI of `editor` when it points at a Java file on disk; `undefined` for
    non-Java editors and untitled buffers. Drives the Problems-panel badge —
    only Java files have Educator findings, so other editors clear it. */
function activeJavaUri(editor: vscode.TextEditor | undefined): vscode.Uri | undefined {
  if (!editor) return undefined;
  if (editor.document.languageId !== 'java') return undefined;
  if (editor.document.uri.scheme !== 'file') return undefined;
  return editor.document.uri;
}

async function ensureServer(
  context: vscode.ExtensionContext,
  workspaceRoot: string,
  outputChannel: vscode.OutputChannel,
  contentFallback: string,
  onReady?: (server: MezzServer) => void
): Promise<MezzServer | undefined> {
  if (server?.running) {
    onReady?.(server);
    return server;
  }

  const config = vscode.workspace.getConfiguration('mezz');
  const binary = resolveMezzBinary(context, outputChannel);
  outputChannel.appendLine(`[mezz] engine: ${binary.command} (${binary.source})`);
  const port = config.get<number>('serverPort', 3200);
  const includeTests = config.get<boolean>('includeTests', false);
  const includeDocs = config.get<boolean>('includeDocs', false);
  const language = config.get<string>('language', '');

  server = new MezzServer(binary.command, workspaceRoot, port, includeTests, includeDocs, outputChannel, contentFallback, language);

  try {
    await server.start();
    onReady?.(server);
    return server;
  } catch (err) {
    vscode.window.showErrorMessage(startupFailureMessage(binary, err));
    return undefined;
  }
}

interface CommitItemApi {
  hash: string;
  short_hash: string;
  message: string;
  author: string;
  date: string;
}

async function pickAndTriggerDiff(port: number): Promise<void> {
  const commits = await fetchCommits(port, 80).catch(async (err) => {
    // The usual cause: the analyzed root isn't inside a git repository
    // (e.g. the repo is a subfolder of the workspace). Offer the fix inline.
    const action = await vscode.window.showErrorMessage(
      `Mezzanine: could not fetch commits — ${(err as Error).message}. ` +
        'If your git repository is a subfolder of the workspace, select it explicitly.',
      'Select Git Repository…'
    );
    if (action) void vscode.commands.executeCommand('mezz.selectGitRepo');
    return [] as CommitItemApi[];
  });
  if (commits.length === 0) return;

  const toItems = (c: CommitItemApi): vscode.QuickPickItem => ({
    label: `$(git-commit) ${c.short_hash}  ${c.message.split('\n')[0]}`,
    description: c.author,
    detail: new Date(c.date).toLocaleString(),
    // Stash the hash via picked label parsing
  });

  const fromPick = await vscode.window.showQuickPick(
    [
      { label: '$(git-branch) HEAD', description: 'current branch tip', detail: '' } as vscode.QuickPickItem,
      ...commits.map(toItems),
    ],
    { placeHolder: 'Pick the BASE commit (from)', matchOnDescription: true, matchOnDetail: true }
  );
  if (!fromPick) return;
  const fromRef = fromPick.label.includes('HEAD') ? 'HEAD' : fromPick.label.replace(/^.*?([a-f0-9]{4,})\s.*/, '$1');

  const toPick = await vscode.window.showQuickPick(
    [
      { label: '$(git-branch) HEAD', description: 'current branch tip', detail: '' } as vscode.QuickPickItem,
      { label: '$(edit) WORKING', description: 'uncommitted working tree', detail: '' } as vscode.QuickPickItem,
      ...commits.map(toItems),
    ],
    { placeHolder: 'Pick the HEAD commit (to)', matchOnDescription: true, matchOnDetail: true }
  );
  if (!toPick) return;
  const toRef = toPick.label.includes('HEAD')
    ? 'HEAD'
    : toPick.label.includes('WORKING')
    ? 'WORKING'
    : toPick.label.replace(/^.*?([a-f0-9]{4,})\s.*/, '$1');

  VisualizerPanel.currentPanel?.sendCommand('triggerDiff', { fromRef, toRef });
}

function fetchCommits(port: number, limit: number): Promise<CommitItemApi[]> {
  return new Promise((resolve, reject) => {
    const req = http.request(
      {
        host: '127.0.0.1', port,
        path: `/api/commits?limit=${limit}`,
        method: 'GET', timeout: 5000,
      },
      (res) => {
        if (res.statusCode !== 200) { reject(new Error(`HTTP ${res.statusCode}`)); res.resume(); return; }
        let body = ''; res.setEncoding('utf-8');
        res.on('data', (chunk) => (body += chunk));
        res.on('end', () => {
          try { resolve(JSON.parse(body)); } catch (e) { reject(e); }
        });
      }
    );
    req.on('error', reject);
    req.on('timeout', () => { req.destroy(); reject(new Error('timeout')); });
    req.end();
  });
}

/**
 * Resolve a path reported by the graph to something on disk.
 *
 * The watch server relativises paths against the analyzed root, so the common
 * case is a join. But `strip_prefix` falls back to the raw absolute path when
 * a file sits outside that root (symlinked root, a `Select Git Repository…`
 * override that disagrees with it), and `Uri.joinPath` *appends* an absolute
 * segment rather than substituting it — turning `/elsewhere/a.ts` into
 * `<root>/elsewhere/a.ts`, which exists nowhere. Test for it instead.
 */
function resolveWorkspacePath(filePath: string): string {
  const workspaceRoot = getProjectRoot();
  if (!workspaceRoot || path.isAbsolute(filePath)) return filePath;
  return vscode.Uri.joinPath(vscode.Uri.file(workspaceRoot), filePath).fsPath;
}

/**
 * Should this selection pull the editor to it?
 *
 * Two selections must not, and neither is a failure worth reporting:
 *
 *  • Folder nodes. At folder aggregation `collapseGraph` sets `file_path` to
 *    the *directory* the entities share, and `openTextDocument` rejects on a
 *    directory. There is no sensible file to pick, so don't try.
 *  • A selection the caret is already inside. Cursor-sync turns a click in the
 *    editor into a graph selection, which arrives back here — following it
 *    would drag the caret from where the user clicked up to the entity's
 *    declaration line. Same round-trip the drag-select guard in
 *    `onDidChangeTextEditorSelection` blocks, reached by a plain click.
 */
function isOpenableSelection(payload: SelectionPayload): boolean {
  if (payload.kind === 'folder') return false;
  const editor = vscode.window.activeTextEditor;
  if (!editor) return true;
  if (editor.document.uri.fsPath !== resolveWorkspacePath(payload.filePath)) return true;
  const caret = editor.selection.active.line + 1;
  return caret < payload.line || caret > (payload.endLine ?? payload.line);
}

async function openAtLine(
  filePath: string,
  line1Based?: number,
  opts: { preserveFocus?: boolean } = {}
): Promise<void> {
  const uri = vscode.Uri.file(filePath);
  const line = Math.max(0, (line1Based ?? 1) - 1);
  const doc = await vscode.workspace.openTextDocument(uri);
  const editor = await vscode.window.showTextDocument(doc, {
    viewColumn: vscode.ViewColumn.One,
    preserveFocus: opts.preserveFocus ?? false,
  });
  const pos = new vscode.Position(line, 0);
  editor.selection = new vscode.Selection(pos, pos);
  editor.revealRange(new vscode.Range(pos, pos), vscode.TextEditorRevealType.InCenter);
}

/** Prompt for a scope-tree filter string; Esc keeps the current filter,
 *  an empty submit clears it. */
async function promptScopeFilter(
  provider: ScopeTreeProvider,
  view: vscode.TreeView<unknown>,
  contextKey: string
): Promise<void> {
  const value = await vscode.window.showInputBox({
    prompt: 'Filter the scope tree (case-insensitive match on the path); leave empty to clear',
    placeHolder: 'e.g. analyzer or src/output',
    value: provider.filter,
  });
  if (value === undefined) return;
  applyScopeFilter(provider, view, contextKey, value);
}

/** Apply a filter to a scope tree and keep the view chrome in sync: the
 *  view description shows the active filter, and the context key swaps
 *  the title-bar icon between "filter" and "clear filter". */
function applyScopeFilter(
  provider: ScopeTreeProvider,
  view: vscode.TreeView<unknown>,
  contextKey: string,
  value: string
): void {
  provider.setFilter(value);
  view.description = provider.filter ? `filter: ${provider.filter}` : undefined;
  void vscode.commands.executeCommand('setContext', contextKey, !!provider.filter);
}

/** Multi-select QuickPick over every path in the scope index — VS Code's
 *  fuzzy matching makes this the fastest way to scope a large repo. The
 *  current selection comes pre-checked; the picked set replaces it. */
async function pickScopes(provider: ScopeTreeProvider, title: string): Promise<void> {
  if (provider.getIndexNodes().length === 0) {
    // Index not loaded yet (visualizer/server not started or still
    // analyzing). refresh() surfaces its own warning on failure.
    await provider.refresh();
    if (provider.getIndexNodes().length === 0) return;
  }
  const selected = new Set(provider.getSelection());
  const items = provider
    .getIndexNodes()
    .filter((n) => n.path !== '')
    .sort((a, b) => a.path.localeCompare(b.path))
    .map((n) => ({
      label: `$(${n.type === 'folder' ? 'folder' : 'file-code'}) ${n.path}`,
      description: `${n.entity_count} entities`,
      picked: selected.has(n.path),
      path: n.path,
    }));
  const picks = await vscode.window.showQuickPick(items, {
    canPickMany: true,
    title,
    placeHolder: 'Type to fuzzy-match paths; check the files/folders to include',
  });
  if (!picks) return;
  provider.replaceSelection(picks.map((p) => p.path));
}

function getFollowMode(): 'file' | 'folder' {
  const v = vscode.workspace
    .getConfiguration('mezz')
    .get<string>('analysisScopeFollowMode', 'folder');
  return v === 'file' ? 'file' : 'folder';
}

/** Resolve the editor's path to a workspace-relative scope key.
 *  Mirrors the keys used by the scope index (posix separators, '' for root).
 *  Returns null for non-file editors or paths outside the workspace. */
function resolveScopeTarget(
  editor: vscode.TextEditor,
  mode: 'file' | 'folder'
): string | null {
  if (editor.document.uri.scheme !== 'file') return null;
  const workspaceRoot = getProjectRoot();
  if (!workspaceRoot) return null;
  const rel = path.relative(workspaceRoot, editor.document.uri.fsPath);
  if (!rel || rel.startsWith('..') || path.isAbsolute(rel)) return null;
  const posix = rel.split(path.sep).join('/');
  if (mode === 'file') return posix;
  const slash = posix.lastIndexOf('/');
  return slash >= 0 ? posix.slice(0, slash) : '';
}

/** Push the editor's path to both the analysis-scope tree and the
 *  visualizer's visual scope. Called from the editor-change listener
 *  (when follow is on), from the toggle-on command, and from the
 *  file/folder mode-switch commands. */
function applyFollow(
  treeProvider: ScopeTreeProvider,
  editor: vscode.TextEditor | undefined
): void {
  if (!editor) return;
  const target = resolveScopeTarget(editor, getFollowMode());
  if (target === null) return;
  treeProvider.setSelection([target]);
  VisualizerPanel.currentPanel?.sendCommand('setAnalysisScopes', [target]);
  VisualizerPanel.currentPanel?.setScopes([target]);
}
