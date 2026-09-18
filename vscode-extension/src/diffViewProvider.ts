import * as vscode from 'vscode';
import type { BranchLabel, DiffState } from './panel';

/**
 * Native "Diff" sidebar: mirrors the CommitPicker + diff overlay controls
 * from the web app. Uses VS Code's QuickPick for commit selection (nicer
 * than rolling a bespoke commit list in a webview).
 *
 * The webview only shows the current diff state + filter toggles; the
 * "Compare Commits" button fires a command the extension handles with the
 * native UI.
 */
export class DiffViewProvider implements vscode.WebviewViewProvider {
  static readonly viewType = 'mezz.diff';

  private view?: vscode.WebviewView;
  private lastState?: DiffState;
  /** The branch the canvas is drawing, or `null` for "say nothing" (UI-114).
   *  Kept beside `lastState` and replayed with it, so a view that is opened
   *  after the webview has already reported comes up labelled. */
  private lastBranch: BranchLabel | null = null;
  private pickCommitsHandler?: () => void;
  private currentChangesHandler?: () => void;

  onPickCommits(handler: () => void): void {
    this.pickCommitsHandler = handler;
  }
  onCurrentChanges(handler: () => void): void {
    this.currentChangesHandler = handler;
  }

  update(state: DiffState): void {
    this.lastState = state;
    this.view?.webview.postMessage({ type: 'state', state });
  }

  /**
   * Say which branch the graph is (UI-114).
   *
   * Its own message rather than a field on `DiffState`, because it is true
   * whether or not a diff is loaded — and this panel's empty state, "no diff
   * loaded", is exactly when a reader most needs to know what they are
   * looking at.
   */
  updateBranch(label: BranchLabel | null): void {
    this.lastBranch = label;
    this.view?.webview.postMessage({ type: 'branch', label });
  }

  resolveWebviewView(
    webviewView: vscode.WebviewView,
    _context: vscode.WebviewViewResolveContext,
    _token: vscode.CancellationToken
  ): void {
    this.view = webviewView;
    webviewView.webview.options = { enableScripts: true };
    webviewView.webview.html = this.getHtml();

    webviewView.webview.onDidReceiveMessage((msg: { type: string; command?: string; value?: unknown }) => {
      if (msg?.type === 'ready') {
        if (this.lastState) {
          webviewView.webview.postMessage({ type: 'state', state: this.lastState });
        }
        webviewView.webview.postMessage({ type: 'branch', label: this.lastBranch });
      } else if (msg?.type === 'pickCommits') {
        this.pickCommitsHandler?.();
      } else if (msg?.type === 'currentChanges') {
        this.currentChangesHandler?.();
      } else if (msg?.type === 'selectRepo') {
        void vscode.commands.executeCommand('mezz.selectGitRepo');
      } else if (msg?.type === 'command' && msg.command) {
        vscode.commands.executeCommand('mezz.internalFilterCommand', {
          command: msg.command,
          value: msg.value,
        });
      }
    });
  }

  private getHtml(): string {
    return /* html */ `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<style>
  body { font-family: var(--vscode-font-family); font-size: var(--vscode-font-size); color: var(--vscode-foreground); padding: 8px 10px; margin: 0; }
  .empty { color: var(--vscode-descriptionForeground); font-style: italic; padding: 12px 0; }
  #branch:empty { display: none; }
  #branch {
    display: flex; align-items: center; gap: 5px;
    margin-bottom: 8px; padding: 3px 7px;
    border: 1px solid var(--vscode-panel-border); border-radius: 10px;
    font-size: 0.8em; font-family: var(--vscode-editor-font-family, monospace);
    color: var(--vscode-descriptionForeground);
    width: fit-content; max-width: 100%;
  }
  #branch .name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  /* Detached is not an error, so it is not red — but it is a state most
     readers arrived at without meaning to. */
  #branch.detached { border-style: dashed; }
  .actions { display: grid; grid-template-columns: 1fr 1fr; gap: 4px; margin-bottom: 10px; }
  button.action {
    padding: 5px 8px; background: transparent; color: var(--vscode-foreground);
    border: 1px solid var(--vscode-panel-border); border-radius: 3px;
    cursor: pointer; font-size: 0.85em; font-family: inherit;
  }
  button.action:hover { background: var(--vscode-list-hoverBackground); }
  button.action.accent { background: var(--vscode-button-background); color: var(--vscode-button-foreground); border-color: transparent; }
  button.action.accent:hover { background: var(--vscode-button-hoverBackground); }
  button.action:disabled { opacity: 0.5; cursor: wait; }
  /* Full width under the two it is the alternative to, so it reads as the
     one thing left to press rather than as a third comparison to start. */
  .stop-row { margin-bottom: 10px; }
  .stop-row button.action { width: 100%; }
  /* Not an error colour: stopping keeps the overlay that is already there. */
  button.action.stop:disabled { opacity: 0.6; }

  .summary {
    background: var(--vscode-textCodeBlock-background);
    border-radius: 3px;
    padding: 8px 10px; margin-bottom: 10px;
  }
  .summary-head {
    display: flex; align-items: center; justify-content: space-between;
    font-size: 0.85em; font-family: var(--vscode-editor-font-family, monospace);
    margin-bottom: 6px;
  }
  .refs { color: var(--vscode-foreground); }
  .refs .arrow { margin: 0 4px; color: var(--vscode-descriptionForeground); }
  button.close-btn {
    background: transparent; border: none; color: var(--vscode-descriptionForeground);
    cursor: pointer; font-size: 1.1em; padding: 0 4px;
  }
  button.close-btn:hover { color: var(--vscode-foreground); }
  .counts {
    display: flex; gap: 12px;
    font-size: 0.85em; font-family: var(--vscode-editor-font-family, monospace);
  }
  .count-add    { color: #73c990; }
  .count-remove { color: #f48771; }
  .count-mod    { color: #e5a650; }
  .count-note {
    font-size: 0.72em; color: var(--vscode-descriptionForeground);
    margin-top: 3px;
  }
  .count-note [data-tip] { cursor: help; text-decoration: underline dotted; text-underline-offset: 2px; }
  .summary-actions {
    display: flex; gap: 4px; margin-top: 8px;
  }
  .summary-actions button {
    flex: 1;
    padding: 4px 8px; font-size: 0.82em;
    background: transparent; color: var(--vscode-foreground);
    border: 1px solid var(--vscode-panel-border); border-radius: 3px;
    cursor: pointer; font-family: inherit;
  }
  .summary-actions button:hover { background: var(--vscode-list-hoverBackground); }
  .summary-actions button.accent {
    background: var(--vscode-button-background); color: var(--vscode-button-foreground);
    border-color: transparent;
  }
  .summary-actions button.accent:hover { background: var(--vscode-button-hoverBackground); }
  .ripple-hint {
    margin-top: 6px;
    padding: 5px 7px;
    background: rgba(229, 166, 80, 0.10);
    border-left: 2px solid #e5a650;
    border-radius: 2px;
    color: var(--vscode-descriptionForeground);
    line-height: 1.4;
  }
  .count-add[data-tip], .count-remove[data-tip], .count-mod[data-tip] { cursor: help; }
  #diff-tip {
    position: fixed; z-index: 1000; max-width: 300px;
    padding: 8px 10px; font-size: 0.82em; line-height: 1.45;
    background: var(--vscode-editorHoverWidget-background, #252526);
    color: var(--vscode-editorHoverWidget-foreground, #cccccc);
    border: 1px solid var(--vscode-editorHoverWidget-border, #454545);
    border-radius: 4px;
    box-shadow: 0 4px 10px rgba(0, 0, 0, 0.35);
    pointer-events: none; opacity: 0; transform: translateY(4px);
    transition: opacity 0.12s, transform 0.12s;
  }
  #diff-tip.show { opacity: 1; transform: translateY(0); }
  #diff-tip .tt { font-weight: 600; margin-bottom: 4px; color: var(--vscode-foreground); }
  .filters { margin-bottom: 10px; }
  .filters .section-title {
    font-size: 0.72em; text-transform: uppercase; letter-spacing: 0.05em;
    color: var(--vscode-descriptionForeground); margin-bottom: 4px;
  }
  .toggle-row {
    display: flex; align-items: center; padding: 2px 0; font-size: 0.85em;
  }
  .toggle-row label { cursor: pointer; display: inline-flex; align-items: center; gap: 6px; }
  .dim-row { display: flex; align-items: center; gap: 6px; margin-top: 4px; font-size: 0.82em; }
  .dim-row input[type="range"] { flex: 1; accent-color: var(--vscode-button-background); }
  .dim-row .dim-val { width: 3em; text-align: right; color: var(--vscode-descriptionForeground); font-family: var(--vscode-editor-font-family, monospace); font-size: 0.9em; }
  .filters.disabled { opacity: 0.55; }
  .error {
    background: rgba(244, 67, 54, 0.15);
    color: #f48771;
    padding: 6px 8px; border-radius: 3px; margin-bottom: 8px;
    font-size: 0.82em;
  }
  .scope-hint {
    display: flex; flex-direction: column; gap: 6px;
    background: rgba(255, 167, 38, 0.10);
    border-left: 2px solid #e5a650;
    padding: 6px 8px; border-radius: 2px; margin-bottom: 10px;
    font-size: 0.8em;
    color: var(--vscode-descriptionForeground);
  }
  .scope-hint b { color: var(--vscode-foreground); }
  .scope-hint button.scope-btn {
    align-self: flex-start;
    padding: 4px 10px;
    background: var(--vscode-button-background);
    color: var(--vscode-button-foreground);
    border: none; border-radius: 3px;
    cursor: pointer; font-size: 1em; font-family: inherit;
  }
  .scope-hint button.scope-btn:hover { background: var(--vscode-button-hoverBackground); }
</style>
</head>
<body>
  <!-- Which branch the graph is. Its own element, above #root and never
       rewritten by render(), so it survives every diff state including
       "Loading" and "no diff loaded" (UI-114). -->
  <div id="branch"></div>
  <div id="root">
    <div class="empty">Loading\u2026</div>
  </div>
  <div id="diff-tip"><div class="tt"></div><div class="tb"></div></div>
<script>
  const vscode = acquireVsCodeApi();
  const root = document.getElementById('root');
  const branchRow = document.getElementById('branch');
  const tip = document.getElementById('diff-tip');
  const tipTitle = tip.querySelector('.tt');
  const tipBody = tip.querySelector('.tb');
  let state = null;

  const DIFF_GLOSSARY = {
    added: {
      title: 'Added',
      body: 'Entities present in HEAD (or WORKING) but not in the base. For working-tree diffs this often reflects untracked files \u2014 run "git status --untracked-files=all" to see them.',
    },
    removed: {
      title: 'Removed',
      body: 'Entities present in the base but gone in HEAD. Renames also look like "removed + added" since we match by stable id, not fuzzy text.',
    },
    modified: {
      title: 'Modified',
      body: 'Entity persists across both refs but its source or its metrics differ. Split into two sub-kinds: core (source actually changed) and impact (only its fan-in/fan-out shifted).',
    },
    core: {
      title: 'Core changes',
      body: 'Entities whose source code or intrinsic metrics (CC, LOC, nesting, \u2026) actually changed between the two refs. This is "something you edited".',
    },
    impact: {
      title: 'Impact changes',
      body: 'Entities whose source is identical but whose relational metrics (fan-in / fan-out) shifted because other entities around them were added, removed, or modified. One new untracked file with 10 dependencies can "impact" 10 other entities without anyone editing them. Toggle "Core changes only" to hide.',
    },
  };

  function showTip(key, target) {
    const d = DIFF_GLOSSARY[key];
    if (!d) return;
    tipTitle.textContent = d.title;
    tipBody.textContent = d.body;
    tip.classList.add('show');
    const rect = target.getBoundingClientRect();
    tip.style.visibility = 'hidden'; tip.style.left = '0'; tip.style.top = '0'; tip.style.visibility = '';
    const tr = tip.getBoundingClientRect();
    let left = rect.left;
    let top = rect.bottom + 6;
    if (left + tr.width > window.innerWidth - 8) left = Math.max(8, window.innerWidth - tr.width - 8);
    if (top + tr.height > window.innerHeight - 8) top = rect.top - tr.height - 6;
    tip.style.left = left + 'px';
    tip.style.top = top + 'px';
  }
  function hideTip() { tip.classList.remove('show'); }
  function bindTips(el) {
    el.querySelectorAll('[data-tip]').forEach((node) => {
      node.addEventListener('mouseenter', () => showTip(node.getAttribute('data-tip'), node));
      node.addEventListener('mouseleave', hideTip);
    });
  }

  function escape(s) {
    return String(s == null ? '' : s).replace(/[&<>"']/g, (c) => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
  }

  // What Stop promises, in the two sentences that keep it apart from the \u00D7
  // above the summary: the run ends, the overlay does not (UI-141).
  var STOP_TITLE = 'Stop this comparison. The engine ends at its next checkpoint; '
    + 'the diff you were already looking at stays on screen.';

  function renderActions(computing, stopping) {
    // A row of its own, under the grid. Both buttons above it are disabled
    // for the whole comparison \u2014 a minute of it, on a repository large enough
    // to want out of \u2014 and this is the one control that does anything.
    var stop = computing
      ? '<div class="stop-row"><button class="action stop" id="btn-stop"'
        + (stopping ? ' disabled' : '') + ' title="' + STOP_TITLE + '">'
        + (stopping ? '\u23F3 Stopping\u2026' : '\u23F9 Stop') + '</button></div>'
      : '';
    return '<div class="actions">'
      + '<button class="action" id="btn-current"' + (computing ? ' disabled' : '') + ' title="Compare HEAD with the current working directory">'
      + (computing ? '\u23F3 ' : '') + 'Working changes</button>'
      + '<button class="action accent" id="btn-pick"' + (computing ? ' disabled' : '') + ' title="Pick two commits to compare">'
      + (computing ? '\u23F3 ' : '') + 'Compare commits</button>'
      + '</div>'
      + stop;
  }

  function renderScopeHint() {
    if (!state?.active) return '';
    if (state.changedFileCount === 0) return '';
    if (state.hasScope) return '';
    return '<div class="scope-hint">'
      + '<div>Diff loaded, but <b>no scope is selected</b> \u2014 the graph is empty.</div>'
      + '<div>Use the <b>Scopes</b> view to pick which files to visualize, or jump to every changed file:</div>'
      + '<button class="scope-btn" id="btn-scope-changes">Scope to changed files (' + state.changedFileCount + ')</button>'
      + '</div>';
  }

  function renderSummary() {
    if (!state?.active || !state.summary) return '';
    const s = state.summary;
    const core = s.modifiedSource != null ? s.modifiedSource : s.modified;
    const impact = s.modifiedImpact != null ? s.modifiedImpact : 0;
    // The summary reflects the active rung so counts match what the graph
    // actually renders. Added / removed are always shown (no core/impact
    // split). Only the Edits rung narrows to core; the wider rungs keep
    // impact-only entities, for a reason they can name.
    const editsOnly = state.level === 'edits';
    const modifiedShown = editsOnly ? core : s.modified;
    const modTip = editsOnly ? 'core' : 'modified';
    const hiddenNote = editsOnly && impact > 0
      ? '<div class="count-note">\u21B3 <span data-tip="impact">' + impact + ' impact</span> hidden by <b>Edits</b></div>'
      : (!editsOnly && (s.modifiedSource != null || s.modifiedImpact != null))
        ? '<div class="count-note">'
            + '<span data-tip="core">' + core + ' core</span>'
            + ' \u00B7 '
            + '<span data-tip="impact">' + impact + ' impact</span>'
          + '</div>'
        : '';
    // Suggest narrowing only when the rung is wide AND impact dominates.
    const rippleHint = (!editsOnly && impact >= 10 && core <= impact / 10)
      ? '<div class="count-note ripple-hint">'
        + '\u2139\uFE0F Most of the modified count is <b>ripple</b> (fan-in/fan-out shifts caused by added/removed entities, not source edits).'
        + ' Drop to <b>Edits</b> below to hide them.'
        + '</div>'
      : '';
    const hasChanges = state.changedFileCount > 0;
    // It clears the overlay *and* tells the engine to stop recomputing it on
    // every save, which is the half that was missing (UI-100) — so say so,
    // because "clear" invites the reading that a diff is still being tracked.
    const clearTip = state.toRef === 'working'
      ? 'Leave diff mode — stop following the working tree and clear the overlay'
      : 'Leave diff mode — clear the overlay';
    const actions = '<div class="summary-actions">'
      + (hasChanges
          ? '<button class="accent" id="btn-scope-all" title="Replace the current scope with every file that has changes">'
            + '\u2919 Scope to changes (' + state.changedFileCount + ')'
            + '</button>'
          : '')
      + '<button id="btn-fit" title="Zoom and pan to fit the graph">\u25A1 Fit view</button>'
      + '</div>';
    return '<div class="summary">'
      + '<div class="summary-head">'
      +   '<span class="refs">' + escape(state.fromRef) + '<span class="arrow">\u2192</span>' + escape(state.toRef) + '</span>'
      +   '<button class="close-btn" id="btn-clear" title="' + clearTip + '">\u00D7</button>'
      + '</div>'
      + '<div class="counts">'
      +   '<span class="count-add" data-tip="added">+' + s.added + ' added</span>'
      +   '<span class="count-remove" data-tip="removed">-' + s.removed + ' removed</span>'
      +   '<span class="count-mod" data-tip="' + modTip + '">~' + modifiedShown + ' modified</span>'
      + '</div>'
      + hiddenNote
      + rippleHint
      + actions
      + '</div>';
  }

  function renderFilters() {
    if (!state?.active) return '';
    const dimPct = Math.round((state.dimOpacity || 0) * 100);
    // UI-112. Above the narrowest rung the ladder draws code the reader did
    // not touch, and at full strength it is drawn exactly like the code they
    // did. The narrowest rung recruits nothing, so the row is absent there
    // rather than present and inert. (No backticks in here — this whole file
    // section is itself a template literal.)
    const ctxPct = Math.round((state.contextOpacity ?? 1) * 100);
    const recruits = state.level !== 'edits';
    const disabledAttr = state.filtersEnabled ? '' : ' disabled';
    const disabledClass = state.filtersEnabled ? '' : ' disabled';
    const masterOffHint = !state.filtersEnabled
      ? '<div class="count-note" style="margin-bottom:6px">'
        + '\u26A0 <b>Apply diff filters</b> is <b>off</b> in View Options \u2014 these toggles won\u2019t affect the graph.'
        + '</div>'
      : '';
    const selectionHint = (state.filtersEnabled && state.hasSelection)
      ? '<div class="count-note" style="margin-bottom:6px">'
        + 'A node is selected \u2014 filtered entities are <b>hidden</b> rather than dimmed, and the Dim opacity slider has no effect.'
        + ' <button class="scope-btn" id="btn-clear-sel" style="padding:2px 8px; margin-top:4px">Clear selection</button>'
        + '</div>'
      : '';
    // One ordered ladder, not two checkboxes: each rung says what it adds to
    // the one before it, which is what the pair it replaced could not.
    const rungs = [
      ['edits', 'Edits', 'Only what you edited, and only the relationships that changed'],
      ['rewiring', 'Rewiring', 'Adds the far end of every relationship that appeared, edited or not'],
      ['neighbourhood', 'Neighbourhood', 'Adds every direct neighbour, and draws all the wiring between them'],
    ];
    const ladder = rungs
      .map((r) =>
        '<div class="toggle-row"><label title="' + escape(r[2]) + '">'
        + '<input type="radio" name="diff-level"'
        + (state.level === r[0] ? ' checked' : '') + disabledAttr
        + ' data-level="' + r[0] + '"> ' + escape(r[1])
        + '</label></div>')
      .join('');
    // Never let the panel imply the canvas drew every reported change.
    const undrawable = (state.undrawableEdges > 0 && state.level !== 'neighbourhood')
      ? '<div class="count-note">'
        + state.undrawableEdges + ' relationship change(s) cannot be drawn — a disappeared edge has'
        + ' no line in the current graph. Select an entity to read them in Details.'
        + '</div>'
      : '';
    return '<div class="filters' + disabledClass + '">'
      + '<div class="section-title">Detail level</div>'
      + masterOffHint
      + selectionHint
      + ladder
      + undrawable
      + (recruits
          ? '<div class="dim-row"><span>Context opacity</span><input type="range" min="10" max="100" step="5" value="' + (ctxPct) + '" id="ctx-slider"' + disabledAttr + ' title="How strongly the entities this rung recruited are drawn, against the edits it grew from. It cannot remove anything \u2014 stepping down a rung is what does that."><span class="dim-val">' + ctxPct + '%</span></div>'
          : '')
      + (!state.hasSelection
          ? '<div class="dim-row"><span>Rest opacity</span><input type="range" min="0" max="15" value="' + (dimPct) + '" id="dim-slider"' + disabledAttr + ' title="Opacity of the entities the ladder left out"><span class="dim-val">' + dimPct + '%</span></div>'
          : '')
      + '</div>';
  }

  function render() {
    if (!state) {
      root.innerHTML = '<div class="empty">Loading\u2026</div>';
      return;
    }
    // Diff errors are usually git errors ("not a git repository", bad refs).
    // When the repo is a subfolder of the workspace, the fix is picking it
    // explicitly — offer that right where the error appears.
    const errorHtml = state.error
      ? '<div class="error">' + escape(state.error)
        + '<div style="margin-top:6px"><button class="action" id="btn-select-repo">Select Git Repository…</button></div>'
        + '</div>'
      : '';
    hideTip();
    root.innerHTML =
      errorHtml
      + renderActions(state.computing, state.stopping)
      + (state.active
          ? (renderScopeHint() + renderSummary() + renderFilters())
          : '<div class="empty">No diff loaded. Compare two commits, or show working-directory changes.</div>');
    bindTips(root);

    document.getElementById('btn-select-repo')?.addEventListener('click', () => vscode.postMessage({ type: 'selectRepo' }));
    document.getElementById('btn-current')?.addEventListener('click', () => vscode.postMessage({ type: 'currentChanges' }));
    document.getElementById('btn-pick')?.addEventListener('click', () => vscode.postMessage({ type: 'pickCommits' }));
    // Through the webview, like every other diff control here: the store that
    // knows a comparison is in flight lives there, and the engine is the same
    // one either surface is talking to (UI-141).
    document.getElementById('btn-stop')?.addEventListener('click', () => vscode.postMessage({ type: 'command', command: 'cancelDiff' }));
    document.getElementById('btn-clear')?.addEventListener('click', () => vscode.postMessage({ type: 'command', command: 'clearDiff' }));
    document.getElementById('btn-scope-changes')?.addEventListener('click', () => vscode.postMessage({ type: 'command', command: 'scopeToChangedFiles' }));
    document.getElementById('btn-scope-all')?.addEventListener('click', () => vscode.postMessage({ type: 'command', command: 'scopeToChangedFiles' }));
    document.getElementById('btn-fit')?.addEventListener('click', () => vscode.postMessage({ type: 'command', command: 'fitView' }));
    document.getElementById('btn-clear-sel')?.addEventListener('click', () => vscode.postMessage({ type: 'command', command: 'clearSelection' }));

    root.querySelectorAll('input[type="checkbox"][data-toggle]').forEach((cb) => {
      cb.addEventListener('change', () => {
        vscode.postMessage({ type: 'command', command: cb.getAttribute('data-toggle'), value: cb.checked });
      });
    });

    root.querySelectorAll('input[type="radio"][data-level]').forEach((rb) => {
      rb.addEventListener('change', () => {
        if (!rb.checked) return;
        vscode.postMessage({ type: 'command', command: 'setDiffLevel', value: rb.getAttribute('data-level') });
      });
    });

    const dim = document.getElementById('dim-slider');
    if (dim) {
      dim.addEventListener('input', (e) => {
        const v = Number(e.target.value) / 100;
        vscode.postMessage({ type: 'command', command: 'setDiffDimOpacity', value: v });
      });
    }

    const ctx = document.getElementById('ctx-slider');
    if (ctx) {
      ctx.addEventListener('input', (e) => {
        const v = Number(e.target.value) / 100;
        vscode.postMessage({ type: 'command', command: 'setDiffContextOpacity', value: v });
      });
    }
  }

  function renderBranch(label) {
    branchRow.classList.toggle('detached', !!(label && label.detached));
    if (!label) {
      // Nothing to say: not a checkout, or an engine with no /api/branch.
      branchRow.innerHTML = '';
      branchRow.removeAttribute('title');
      return;
    }
    branchRow.title = label.title;
    branchRow.innerHTML = '<span aria-hidden="true">\u2387</span>'
      + '<span class="name">' + escape(label.text) + '</span>';
  }

  window.addEventListener('message', (event) => {
    const msg = event.data;
    if (msg?.type === 'state') {
      state = msg.state;
      render();
    } else if (msg?.type === 'branch') {
      renderBranch(msg.label ?? null);
    }
  });

  vscode.postMessage({ type: 'ready' });
</script>
</body>
</html>`;
  }
}
