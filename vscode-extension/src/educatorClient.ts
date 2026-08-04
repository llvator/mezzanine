import * as http from 'http';
import * as path from 'path';

/**
 * Shared client for the Educator HTTP API. Both the hover provider and the
 * sidebar mirror talk to the daemon through this module so the wire shapes
 * and path-resolution rules live in one place.
 */

/**
 * Whether a hit attaches to the cursor's innermost construct (`direct`) or to
 * an enclosing one (`ancestor`). Lets the UI distinguish "this lesson is
 * about the thing under your cursor" from "this lesson is about the method
 * containing the thing under your cursor."
 */
export type Attachment = 'direct' | 'ancestor';

export interface RuleHit {
  rule_id: string;
  title: string;
  kind: string;
  severity: string;
  body_markdown: string;
  source_path: string;
  /** Construct-kind in the cursor stack that the rule actually matched against. */
  attached_to: string;
  /** Whether `attached_to` is the cursor's innermost construct or an ancestor. */
  attachment: Attachment;
  /** Single-line source snippet of the construct the rule matched against. */
  attached_text: string;
}

/** One lesson hit — teaching content, no severity/kind. */
export interface LessonHit {
  lesson_id: string;
  title: string;
  level: 'beginner' | 'intermediate' | 'advanced' | string;
  body_markdown: string;
  source_path: string;
  /** Construct-kind in the cursor stack that the lesson attaches to. */
  attached_to: string;
  /** Whether `attached_to` is the cursor's innermost construct or an ancestor. */
  attachment: Attachment;
  /** Single-line source snippet of the construct the lesson attaches to. */
  attached_text: string;
}

export interface ConstructInstance {
  kind: string;
  attrs: Record<string, string>;
  /** Single-line source snippet of the construct's span (truncated to ~80 chars). */
  text: string;
}

export interface PositionResponse {
  language: string;
  stack: ConstructInstance[];
  /**
   * Innermost construct-kind under the cursor (`stack[0].kind` when stack is
   * non-empty). A hit's `attachment` is `'direct'` iff its `attached_to`
   * equals this value.
   */
  cursor_kind: string | null;
  /** Source-text snippet of the cursor's innermost construct, when one is recognized. */
  cursor_text: string | null;
  specific: RuleHit[];
  general: RuleHit[];
  lessons: LessonHit[];
}

/**
 * True when nothing at all attaches at this position. Used by the **hover**
 * provider to drop empty contributions. The sidebar uses its own emptiness
 * predicate that respects lessons too.
 */
export function isEmpty(resp: PositionResponse): boolean {
  return resp.specific.length === 0 && resp.general.length === 0;
}

/** True when no rule *and* no lesson attaches — used by the sidebar. */
export function isFullyEmpty(resp: PositionResponse): boolean {
  return isEmpty(resp) && resp.lessons.length === 0;
}

/**
 * Convert an absolute file path to the workspace-relative form the server
 * expects. Returns `undefined` when the file is outside the workspace —
 * callers should skip the request rather than asking the server about files
 * it isn't analyzing.
 */
export function relativeToWorkspace(
  filePath: string,
  workspaceRoot: string
): string | undefined {
  if (!filePath.startsWith(workspaceRoot)) return undefined;
  return path
    .relative(workspaceRoot, filePath)
    .split(path.sep)
    .join('/');
}

export function fetchPosition(
  port: number,
  file: string,
  line: number,
  col: number,
  timeoutMs = 1000
): Promise<PositionResponse> {
  const query = `file=${encodeURIComponent(file)}&line=${line}&col=${col}`;
  return getJson<PositionResponse>(port, `/api/educator/position?${query}`, timeoutMs);
}

/** One rule hit in a full-file scan. Mirrors `educator::ScanHit` server-side. */
export interface ScanHit {
  line: number;
  col: number;
  end_line: number;
  end_col: number;
  kind: string;
  bucket: 'specific' | 'general';
  rule_id: string;
  title: string;
  severity: 'info' | 'warning' | 'error';
  rule_kind: string;
}

export interface ScanResponse {
  language: string;
  hits: ScanHit[];
}

export function fetchScan(
  port: number,
  file: string,
  timeoutMs = 5000
): Promise<ScanResponse> {
  const query = `file=${encodeURIComponent(file)}`;
  return getJson<ScanResponse>(port, `/api/educator/scan?${query}`, timeoutMs);
}

function getJson<T>(port: number, path: string, timeoutMs: number): Promise<T> {
  return new Promise((resolve, reject) => {
    const req = http.request(
      {
        host: '127.0.0.1',
        port,
        path,
        method: 'GET',
        timeout: timeoutMs,
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
