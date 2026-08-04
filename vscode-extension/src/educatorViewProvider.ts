import * as vscode from 'vscode';
import {
  LessonHit,
  PositionResponse,
  RuleHit,
  fetchPosition,
  relativeToWorkspace,
} from './educatorClient';

/**
 * Sidebar mirror of the Educator hover (design Q11). Subscribes to the active
 * editor's cursor and renders the same Specific/General buckets the hover
 * shows. Lives behind `nao.educator.sidebarEnabled` (default off) so users
 * who hate hover popups have an opt-in alternative without losing the
 * educator entirely.
 */
export class EducatorViewProvider implements vscode.WebviewViewProvider {
  static readonly viewType = 'nao.educator';

  private view?: vscode.WebviewView;
  private debounceTimer: NodeJS.Timeout | undefined;
  private requestSeq = 0;

  constructor(
    private readonly getServerPort: () => number | undefined,
    private readonly getWorkspaceRoot: () => string | undefined,
    private readonly output?: vscode.OutputChannel
  ) {}

  private log(line: string): void {
    this.output?.appendLine(`[educator-sidebar] ${line}`);
  }

  resolveWebviewView(
    webviewView: vscode.WebviewView,
    _context: vscode.WebviewViewResolveContext,
    _token: vscode.CancellationToken
  ): void {
    this.view = webviewView;
    webviewView.webview.options = { enableScripts: true };
    webviewView.webview.html = renderShell();
    this.log('view resolved');

    // When the user collapses then re-expands the Educator row, the view
    // stays alive but `visible` flips. Refresh on becoming visible so the
    // panel always reflects the current cursor on re-expand.
    webviewView.onDidChangeVisibility(() => {
      this.log(`visibility changed → visible=${webviewView.visible}`);
      if (webviewView.visible && vscode.window.activeTextEditor) {
        this.scheduleUpdate(vscode.window.activeTextEditor);
      }
    });

    if (vscode.window.activeTextEditor) {
      this.scheduleUpdate(vscode.window.activeTextEditor);
    }
  }

  /**
   * Push the editor's current cursor position into the panel. Debounced
   * (~150ms) so rapid cursor movement doesn't hammer the position endpoint.
   * In-flight requests are tagged with a sequence number — stale results
   * arriving after the user has moved on are discarded.
   *
   * Gating: the view's own `visible` property is the single gate. Collapsed
   * sidebar row → silent no-op. Expanded → tracks the cursor.
   */
  scheduleUpdate(editor: vscode.TextEditor): void {
    if (!this.view || !this.view.visible) {
      // Panel collapsed or not yet resolved — stay silent. No log, no work.
      return;
    }
    if (editor.document.languageId !== 'java') {
      this.log(`scheduleUpdate: non-java editor (${editor.document.languageId}), posting idle`);
      this.postIdle('No rules apply here — Educator currently covers Java only.');
      return;
    }
    if (this.debounceTimer) clearTimeout(this.debounceTimer);
    this.debounceTimer = setTimeout(() => {
      void this.refresh(editor);
    }, 150);
  }

  private async refresh(editor: vscode.TextEditor): Promise<void> {
    const seq = ++this.requestSeq;
    const port = this.getServerPort();
    const root = this.getWorkspaceRoot();
    if (!port || !root) {
      this.log(`refresh skipped — port=${port} root=${root ? 'set' : 'unset'}`);
      this.postIdle('Open the Nao visualizer to start the server.');
      return;
    }
    const rel = relativeToWorkspace(editor.document.uri.fsPath, root);
    if (!rel) {
      this.log(`refresh: file outside workspace (${editor.document.uri.fsPath})`);
      this.postIdle('File is outside the analyzed workspace.');
      return;
    }
    const cursor = editor.selection.active;
    this.log(`refresh: GET /api/educator/position?file=${rel}&line=${cursor.line}&col=${cursor.character} (seq=${seq})`);
    try {
      const response = await fetchPosition(
        port,
        rel,
        cursor.line,
        cursor.character
      );
      if (seq !== this.requestSeq) {
        this.log(`refresh seq=${seq} dropped (newer request in flight)`);
        return;
      }
      this.log(
        `refresh seq=${seq} → ${response.specific.length} specific, ${response.general.length} general, ${response.lessons.length} lesson(s)`
      );
      // Render the panel even when buckets are empty — the `Cursor on:`
      // header is always informative (it names the construct under the
      // cursor) and makes it obvious when no lesson/rule attaches.
      this.view?.webview.postMessage({
        type: 'rules',
        body: renderHtml(response),
      });
    } catch (err) {
      if (seq !== this.requestSeq) return;
      this.log(`refresh seq=${seq} failed: ${(err as Error).message}`);
      this.postIdle(`Educator query failed: ${(err as Error).message}`);
    }
  }

  private postIdle(message: string): void {
    this.view?.webview.postMessage({
      type: 'idle',
      message,
    });
  }
}

/**
 * Render the sidebar grouped by *construct in the ancestor stack*, not by
 * content-type (Specific / General / Lesson). The cursor's innermost construct
 * shows first as a non-collapsible placeholder card; each enclosing ancestor
 * becomes a collapsible `<details>` section below it. Inside each section the
 * hits (rules + lessons) render with a "specific" / "general" / "lesson"
 * bucket chip so the content type is still visible. Sections with no attached
 * hits show "No rules or lessons about <kind>." so it's clear nothing targets
 * the construct rather than the panel being silently empty.
 */
function renderHtml(response: PositionResponse): string {
  if (!response.cursor_kind) {
    return [
      `<section class="construct cursor empty">`,
      `  <header class="construct-header">`,
      `    <span class="construct-label">Cursor on:</span>`,
      `    <span class="construct-empty-label">no construct recognized at this position</span>`,
      `  </header>`,
      `</section>`,
    ].join('\n');
  }
  const groups = groupHitsByConstruct(response);
  return groups.map((g, idx) => renderConstructSection(g, idx === 0)).join('\n');
}

interface ConstructGroup {
  kind: string;
  text: string;
  specific: RuleHit[];
  general: RuleHit[];
  lessons: LessonHit[];
}

function groupHitsByConstruct(response: PositionResponse): ConstructGroup[] {
  // One group per entry in the ancestor stack, preserving innermost-first order.
  // The server dedupes hits per rule/lesson id, so each hit's `attached_to`
  // points at the first (innermost) ancestor that matched — we assign each hit
  // to the first group whose kind matches.
  const groups: ConstructGroup[] = response.stack.map((instance) => ({
    kind: instance.kind,
    text: instance.text,
    specific: [],
    general: [],
    lessons: [],
  }));
  const findGroup = (attachedTo: string): ConstructGroup | undefined =>
    groups.find((g) => g.kind === attachedTo);
  for (const hit of response.specific) {
    findGroup(hit.attached_to)?.specific.push(hit);
  }
  for (const hit of response.general) {
    findGroup(hit.attached_to)?.general.push(hit);
  }
  for (const hit of response.lessons) {
    findGroup(hit.attached_to)?.lessons.push(hit);
  }
  return groups;
}

function renderConstructSection(group: ConstructGroup, isCursor: boolean): string {
  const kindLabel = humaniseConstructKind(group.kind);
  const totalHits = group.specific.length + group.general.length + group.lessons.length;
  const isEmpty = totalHits === 0;

  const header: string[] = [];
  header.push(`  <header class="construct-header">`);
  if (isCursor) {
    header.push(`    <span class="construct-label">Cursor on:</span>`);
  } else {
    header.push(`    <span class="construct-arrow" title="Ancestor of the cursor's innermost construct">↑</span>`);
  }
  header.push(`    <span class="construct-kind">${escapeHtml(kindLabel)}</span>`);
  if (group.text) {
    header.push(`    <code class="construct-text">${escapeHtml(group.text)}</code>`);
  }
  header.push(`  </header>`);

  const body: string[] = [];
  if (isEmpty) {
    body.push(
      `  <div class="construct-empty">No rules or lessons about ${escapeHtml(kindLabel)}.</div>`
    );
  } else {
    for (const hit of group.specific) {
      body.push(renderRuleInside(hit, 'specific'));
    }
    for (const hit of group.general) {
      body.push(renderRuleInside(hit, 'general'));
    }
    for (const hit of group.lessons) {
      body.push(renderLessonInside(hit));
    }
  }

  // The cursor's innermost construct is the focal point — non-collapsible.
  // Ancestor sections are collapsible (expanded by default) so the reader can
  // fold away enclosing-context content when they only care about the cursor.
  if (isCursor) {
    return [
      `<section class="construct cursor${isEmpty ? ' empty' : ''}">`,
      ...header,
      ...body,
      `</section>`,
    ].join('\n');
  }
  return [
    `<details class="construct ancestor${isEmpty ? ' empty' : ''}" open>`,
    `  <summary class="construct-summary">`,
    ...header,
    `  </summary>`,
    ...body,
    `</details>`,
  ].join('\n');
}

/** Render a rule hit inside a construct section. The attachment chip is
 *  omitted (the enclosing section already names the construct it attaches
 *  to); a `specific` / `general` chip records which bucket the rule fell
 *  into so the content type stays visible. */
function renderRuleInside(hit: RuleHit, bucket: 'specific' | 'general'): string {
  const severityIcon =
    hit.severity === 'error'
      ? '🛑'
      : hit.severity === 'warning'
      ? '⚠️'
      : 'ℹ️';
  const body = markdownToHtml(stripFirstHeading(hit.body_markdown).trim());
  return [
    `<article class="rule" data-severity="${escapeHtml(hit.severity)}">`,
    `  <h3 class="rule-title"><span class="severity">${severityIcon}</span> ${escapeHtml(hit.title)}</h3>`,
    `  <div class="chips">`,
    `    <span class="bucket-chip ${bucket}">${bucket}</span>`,
    `    <span class="kind">${escapeHtml(hit.kind)}</span>`,
    `  </div>`,
    `  <div class="rule-body">${body}</div>`,
    `</article>`,
  ].join('\n');
}

/** Render a lesson hit inside a construct section. Same attachment-chip
 *  rationale as [`renderRuleInside`]. */
function renderLessonInside(hit: LessonHit): string {
  const body = markdownToHtml(stripFirstHeading(hit.body_markdown).trim());
  return [
    `<article class="lesson-card" data-level="${escapeHtml(hit.level)}">`,
    `  <h3 class="lesson-title">📚 ${escapeHtml(hit.title)}</h3>`,
    `  <div class="chips">`,
    `    <span class="bucket-chip lesson">lesson</span>`,
    `    <span class="lesson-level">${escapeHtml(hit.level)}</span>`,
    `  </div>`,
    `  <div class="lesson-body">${body}</div>`,
    `</article>`,
  ].join('\n');
}

function humaniseConstructKind(kind: string): string {
  return kind.replaceAll('_', ' ');
}

function stripFirstHeading(markdown: string): string {
  const lines = markdown.split('\n');
  const idx = lines.findIndex((l) => l.trimStart().startsWith('# '));
  if (idx === -1) return markdown;
  return [...lines.slice(0, idx), ...lines.slice(idx + 1)].join('\n');
}

/**
 * Tiny markdown-to-HTML converter. Handles the subset our rule bodies use:
 * `## Section` → `<h4>`, fenced code blocks (\`\`\`java …) → `<pre><code>`,
 * blank-line paragraph breaks, `**bold**` and `` `inline code` ``. Everything
 * else is passed through as a paragraph. Replace with `marked`/`markdown-it`
 * the moment a rule body needs more — but EDU-002's corpus does not.
 */
function markdownToHtml(md: string): string {
  const lines = md.split('\n');
  const out: string[] = [];
  let i = 0;
  let paragraph: string[] = [];

  const flushParagraph = () => {
    if (paragraph.length === 0) return;
    out.push(`<p>${formatInline(paragraph.join(' '))}</p>`);
    paragraph = [];
  };

  while (i < lines.length) {
    const line = lines[i];
    if (line.startsWith('```')) {
      flushParagraph();
      const fence = line.trim();
      const lang = fence.replace(/^`+/, '').trim();
      const codeLines: string[] = [];
      i++;
      while (i < lines.length && !lines[i].startsWith('```')) {
        codeLines.push(lines[i]);
        i++;
      }
      const langAttr = lang
        ? ` class="lang-${escapeHtml(lang)}"`
        : '';
      out.push(
        `<pre><code${langAttr}>${escapeHtml(codeLines.join('\n'))}</code></pre>`
      );
      i++;
      continue;
    }
    if (line.startsWith('## ')) {
      flushParagraph();
      out.push(`<h4>${formatInline(line.slice(3).trim())}</h4>`);
      i++;
      continue;
    }
    if (line.startsWith('# ')) {
      flushParagraph();
      out.push(`<h3>${formatInline(line.slice(2).trim())}</h3>`);
      i++;
      continue;
    }
    if (line.trim() === '') {
      flushParagraph();
      i++;
      continue;
    }
    paragraph.push(line);
    i++;
  }
  flushParagraph();
  return out.join('\n');
}

function formatInline(s: string): string {
  // Escape first; the bold/code replacements use safe markers that can't appear
  // in already-escaped text.
  const escaped = escapeHtml(s);
  return escaped
    .replace(/`([^`]+)`/g, '<code>$1</code>')
    .replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
}

function escapeHtml(s: string): string {
  return s.replace(/[&<>"']/g, (c) => {
    switch (c) {
      case '&': return '&amp;';
      case '<': return '&lt;';
      case '>': return '&gt;';
      case '"': return '&quot;';
      case "'": return '&#39;';
      default: return c;
    }
  });
}

function renderShell(): string {
  return /* html */ `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<style>
  body {
    font-family: var(--vscode-font-family);
    font-size: var(--vscode-font-size);
    color: var(--vscode-foreground);
    padding: 8px 10px;
    margin: 0;
    line-height: 1.45;
  }
  .idle {
    color: var(--vscode-descriptionForeground);
    font-style: italic;
    padding: 12px 0;
  }
  /* One construct section per entry in the cursor's ancestor stack.
     .cursor (the innermost) is non-collapsible and visually prominent.
     .ancestor sections are details elements — collapsible, expanded by
     default. .empty marks sections with no attached rules/lessons; their
     border switches to dashed so the empty state is visible at a glance. */
  .construct {
    margin-bottom: 12px;
    padding: 6px 8px 8px;
    border: 1px solid var(--vscode-panel-border);
    border-radius: 4px;
    background: var(--vscode-editor-background);
  }
  .construct.cursor {
    background: var(--vscode-editor-background);
  }
  .construct.ancestor {
    background: transparent;
    opacity: 0.95;
  }
  .construct.empty {
    border-style: dashed;
    background: transparent;
  }
  .construct-header {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 6px;
    font-size: 0.9em;
  }
  .construct-label {
    font-size: 0.75em;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--vscode-descriptionForeground);
  }
  .construct-arrow {
    color: var(--vscode-descriptionForeground);
    font-size: 0.9em;
  }
  .construct-kind {
    color: var(--vscode-foreground);
    font-weight: 600;
  }
  .construct-text {
    font-family: var(--vscode-editor-font-family, monospace);
    font-size: 0.85em;
    background: var(--vscode-textCodeBlock-background);
    padding: 1px 5px;
    border-radius: 3px;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 100%;
  }
  .construct-empty-label {
    color: var(--vscode-descriptionForeground);
    font-style: italic;
  }
  .construct-empty {
    margin-top: 6px;
    padding-top: 6px;
    border-top: 1px dashed var(--vscode-panel-border);
    font-size: 0.85em;
    color: var(--vscode-descriptionForeground);
    font-style: italic;
  }
  /* Native details/summary styling — replace the default disclosure triangle
     with our own so collapse/expand state matches the theme. */
  .construct.ancestor > summary {
    list-style: none;
    cursor: pointer;
    user-select: none;
  }
  .construct.ancestor > summary::-webkit-details-marker { display: none; }
  .construct.ancestor > summary::before {
    content: '▾';
    display: inline-block;
    width: 1em;
    margin-right: 2px;
    color: var(--vscode-descriptionForeground);
    font-size: 0.85em;
  }
  .construct.ancestor:not([open]) > summary::before {
    content: '▸';
  }
  .rule {
    border-left: 3px solid var(--vscode-panel-border);
    padding: 4px 0 4px 10px;
    margin-bottom: 10px;
  }
  .bucket.specific .rule { border-left-color: var(--vscode-charts-yellow, #f4b400); }
  .rule[data-severity="error"] { border-left-color: var(--vscode-charts-red, #d33); }
  .rule-title {
    margin: 0 0 4px 0;
    font-size: 1em;
  }
  .rule-title .severity { margin-right: 4px; }
  .rule-body h3, .rule-body h4 {
    margin: 8px 0 4px 0;
    font-size: 0.95em;
  }
  .rule-body p { margin: 0 0 6px 0; }
  .rule-body pre {
    background: var(--vscode-textCodeBlock-background);
    padding: 6px 8px;
    border-radius: 3px;
    overflow-x: auto;
    font-family: var(--vscode-editor-font-family, monospace);
    font-size: 0.9em;
    margin: 4px 0;
  }
  .rule-body code {
    font-family: var(--vscode-editor-font-family, monospace);
    font-size: 0.9em;
  }
  .rule-body p code, .rule-body h4 code {
    background: var(--vscode-textCodeBlock-background);
    padding: 0 3px;
    border-radius: 2px;
  }
  .bucket.lesson .bucket-title { opacity: 0.85; }
  .lesson-card {
    border-left: 3px solid var(--vscode-charts-blue, #3794ff);
    padding: 4px 0 4px 10px;
    margin-bottom: 12px;
  }
  .lesson-title {
    margin: 0 0 2px 0;
    font-size: 1em;
  }
  .lesson-level {
    display: inline-block;
    font-size: 0.7em;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--vscode-badge-foreground);
    background: var(--vscode-badge-background);
    padding: 1px 5px;
    border-radius: 2px;
    margin-bottom: 4px;
  }
  .lesson-body h3, .lesson-body h4 {
    margin: 8px 0 4px 0;
    font-size: 0.95em;
  }
  .lesson-body p { margin: 0 0 6px 0; }
  .lesson-body pre {
    background: var(--vscode-textCodeBlock-background);
    padding: 6px 8px;
    border-radius: 3px;
    overflow-x: auto;
    font-family: var(--vscode-editor-font-family, monospace);
    font-size: 0.9em;
    margin: 4px 0;
  }
  .lesson-body code {
    font-family: var(--vscode-editor-font-family, monospace);
    font-size: 0.9em;
  }
  .lesson-body p code, .lesson-body h4 code {
    background: var(--vscode-textCodeBlock-background);
    padding: 0 3px;
    border-radius: 2px;
  }
  /* Chips row between the title and body of a rule or lesson card. Hosts
     the rule kind / lesson level chip plus the attachment chip. */
  .chips {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px;
    margin: 2px 0 6px 0;
  }
  .chips .kind {
    display: inline-block;
    font-size: 0.7em;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    background: var(--vscode-badge-background);
    color: var(--vscode-badge-foreground);
    padding: 1px 5px;
    border-radius: 2px;
  }
  .attachment {
    display: inline-block;
    font-size: 0.7em;
    text-transform: lowercase;
    letter-spacing: 0.04em;
    padding: 1px 5px;
    border-radius: 2px;
    font-family: var(--vscode-editor-font-family, monospace);
  }
  .attachment.direct {
    background: var(--vscode-badge-background);
    color: var(--vscode-badge-foreground);
  }
  .attachment.ancestor {
    background: transparent;
    color: var(--vscode-descriptionForeground);
    border: 1px dashed var(--vscode-panel-border);
  }
  /* Ancestor hits are about an enclosing construct, not the thing under the
     cursor — mute them so direct hits dominate. */
  .rule[data-attachment="ancestor"],
  .lesson-card[data-attachment="ancestor"] {
    opacity: 0.75;
  }
</style>
</head>
<body>
  <div id="root">
    <div class="idle">Place the cursor in a Java file to see rules that apply here.</div>
  </div>
<script>
  const root = document.getElementById('root');
  window.addEventListener('message', (event) => {
    const msg = event.data;
    if (msg?.type === 'idle') {
      root.innerHTML = '<div class="idle">' + (msg.message || '') + '</div>';
    } else if (msg?.type === 'rules') {
      root.innerHTML = msg.body;
    }
  });
</script>
</body>
</html>`;
}
