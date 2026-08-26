import * as vscode from 'vscode';
import {
  PositionResponse,
  RuleHit,
  fetchPosition,
  isEmpty,
  relativeToWorkspace,
} from './educatorClient';

/**
 * Hover provider that surfaces Educator rules at the cursor.
 *
 * Calls `GET /api/educator/position`, partitions the response into Specific
 * (matched predicates) and General (attached without predicate) buckets, and
 * renders a markdown hover under a `### Mezzanine Educator` header.
 *
 * Returns `undefined` (no contribution) when both buckets are empty, when the
 * setting is disabled, when there is no workspace, or when the server is not
 * running — design Q11: no empty Educator sections.
 */
export class EducatorHoverProvider implements vscode.HoverProvider {
  constructor(
    private readonly getServerPort: () => number | undefined,
    private readonly getWorkspaceRoot: () => string | undefined
  ) {}

  async provideHover(
    document: vscode.TextDocument,
    position: vscode.Position,
    token: vscode.CancellationToken
  ): Promise<vscode.Hover | undefined> {
    const config = vscode.workspace.getConfiguration('mezz');
    if (!config.get<boolean>('educator.hoverEnabled', true)) {
      return undefined;
    }

    const port = this.getServerPort();
    if (!port) return undefined;

    const workspaceRoot = this.getWorkspaceRoot();
    if (!workspaceRoot) return undefined;

    const rel = relativeToWorkspace(document.uri.fsPath, workspaceRoot);
    if (!rel) return undefined;

    const response = await fetchPosition(
      port,
      rel,
      position.line,
      position.character
    ).catch(() => undefined);
    if (!response) return undefined;
    if (token.isCancellationRequested) return undefined;
    if (isEmpty(response)) return undefined;

    const md = new vscode.MarkdownString(renderHover(response));
    md.isTrusted = false;
    md.supportHtml = false;
    return new vscode.Hover(md);
  }
}

function renderHover(response: PositionResponse): string {
  const lines: string[] = [];
  lines.push('### Mezzanine Educator');

  if (response.specific.length > 0) {
    lines.push('');
    lines.push('**Specific**');
    for (const hit of response.specific) {
      lines.push('');
      lines.push(formatHit(hit));
    }
  }

  if (response.general.length > 0) {
    lines.push('');
    lines.push('---');
    lines.push('');
    lines.push('**General**');
    for (const hit of response.general) {
      lines.push('');
      lines.push(formatHit(hit));
    }
  }

  return lines.join('\n');
}

function formatHit(hit: RuleHit): string {
  const severityBadge =
    hit.severity === 'error'
      ? '🛑 '
      : hit.severity === 'warning'
      ? '⚠️ '
      : 'ℹ️ ';
  const body = stripFirstHeading(hit.body_markdown).trim();
  // Direct hits stay terse; ancestor hits get an "(in: <construct> `<snippet>`)"
  // suffix so the reader can tell the rule is about an enclosing construct
  // rather than the thing the cursor is on — and which actual element it
  // attaches to.
  const context =
    hit.attachment === 'ancestor'
      ? ` _(in: ${hit.attached_to.replaceAll('_', ' ')}${
          hit.attached_text ? ` \`${hit.attached_text}\`` : ''
        })_`
      : '';
  return `${severityBadge}**${hit.title}**${context}\n\n${body}`;
}

function stripFirstHeading(markdown: string): string {
  const lines = markdown.split('\n');
  const idx = lines.findIndex((l) => l.trimStart().startsWith('# '));
  if (idx === -1) return markdown;
  return [...lines.slice(0, idx), ...lines.slice(idx + 1)].join('\n');
}
