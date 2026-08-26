import * as vscode from 'vscode';
import * as http from 'http';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { spawnSync } from 'child_process';

/**
 * Spawn a Claude Code session in a VS Code terminal, seeded with an entity's
 * refactor prompt (UI-037).
 *
 * The terminal *is* the monitoring UI. That is the whole design: the agent
 * runs in the user's own shell, under their own permissions, with no port
 * listening and no engine involvement beyond fetching the prompt. Nothing
 * here needs the trust-boundary decision that a headless engine-owned runner
 * would (SRV-011) — this is exactly what the user could have typed.
 */

/** POST /api/scope — fetch the engine-built refactor prompt for one entity. */
function fetchRefactorPrompt(port: number, entityId: string): Promise<string> {
  return new Promise((resolve, reject) => {
    const body = JSON.stringify({ entity_id: entityId, mode: 'refactor', depth: 1 });
    const req = http.request(
      {
        host: '127.0.0.1', port,
        path: '/api/scope',
        method: 'POST',
        timeout: 30000,
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
          try {
            const parsed = JSON.parse(data);
            const prompt = parsed?.exports?.refactor_prompt;
            if (typeof prompt !== 'string' || !prompt.trim()) {
              // An engine older than SRV-010 answers /api/scope perfectly well
              // but has no such export. Say which of the two it is.
              reject(new Error('this engine does not produce refactor prompts (needs SRV-010)'));
              return;
            }
            resolve(prompt);
          } catch (e) { reject(e as Error); }
        });
      }
    );
    req.on('error', reject);
    req.on('timeout', () => { req.destroy(); reject(new Error('timeout')); });
    req.end(body);
  });
}

/**
 * Write the prompt somewhere the agent can read it.
 *
 * Deliberately NOT passed as a shell argument: a refactor prompt is kilobytes
 * of Markdown carrying newlines, backticks and quotes. Interpolating that into
 * a command line invites `ARG_MAX` truncation and quoting corruption, and the
 * corruption is silent — the agent just receives a mangled ask. A file keeps
 * the prompt byte-identical to what the copy button produces.
 */
function writePromptFile(entityName: string, prompt: string): string {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'mezz-refactor-'));
  const safe = entityName.replace(/[^\w.-]+/g, '_').slice(0, 60) || 'entity';
  const file = path.join(dir, `${safe}.prompt.md`);
  fs.writeFileSync(file, prompt, 'utf-8');
  return file;
}

/** Single-quote a path for POSIX shells. Windows uses a different form. */
function shellQuote(p: string): string {
  return `'${p.replace(/'/g, `'\\''`)}'`;
}

/**
 * Is `binary` runnable? `createTerminal` cannot tell us — it always succeeds
 * and the shell prints "command not found" a moment later, which is precisely
 * the terminal-flashes-an-error failure this feature is supposed to avoid.
 * Resolving up front turns that into a message the user can act on.
 */
function isOnPath(binary: string): boolean {
  const probe = process.platform === 'win32' ? 'where' : 'which';
  try {
    return spawnSync(probe, [binary], { encoding: 'utf-8' }).status === 0;
  } catch {
    return false;
  }
}

/**
 * A launch that failed for a reason worth showing the user. Carries the
 * prompt when we got that far, so the caller can offer it as a fallback
 * rather than making the user re-run anything.
 */
export class AgentLaunchError extends Error {
  constructor(message: string, readonly prompt?: string) {
    super(message);
    this.name = 'AgentLaunchError';
  }
}

export interface SpawnAgentOptions {
  port: number;
  entityId: string;
  entityName: string;
  cwd: string;
  /** Overridable so a user whose `claude` isn't on PATH can still launch. */
  binary?: string;
}

/**
 * Fetch the prompt, then open a terminal running Claude Code against it.
 *
 * Returns the terminal so callers can test or dispose it. Throws
 * `AgentLaunchError` for the two expected failures — no engine-side prompt,
 * or no Claude Code binary — leaving the caller to surface them.
 */
export async function spawnAgentTerminal(opts: SpawnAgentOptions): Promise<vscode.Terminal> {
  let prompt: string;
  try {
    prompt = await fetchRefactorPrompt(opts.port, opts.entityId);
  } catch (e) {
    throw new AgentLaunchError(e instanceof Error ? e.message : String(e));
  }

  const binary = opts.binary?.trim() || 'claude';
  if (!isOnPath(binary)) {
    throw new AgentLaunchError(
      `'${binary}' is not on PATH — install Claude Code, or set mezz.claudeBinary to its full path`,
      prompt
    );
  }

  const file = writePromptFile(opts.entityName, prompt);

  const terminal = vscode.window.createTerminal({
    name: `mezz: refactor ${opts.entityName}`,
    cwd: opts.cwd,
    // Marks the terminal as ours in the panel's dropdown.
    iconPath: new vscode.ThemeIcon('sparkle'),
  });

  // A positional prompt starts an *interactive* session with that prompt
  // queued — `-p` would make it headless and print-and-exit, which is the
  // opposite of what a terminal is for here. The agent reads the file itself,
  // so the command line stays short whatever the prompt's size.
  terminal.sendText(
    `${binary} "Read ${shellQuote(file)} and carry out the refactoring task it describes."`
  );
  terminal.show();
  return terminal;
}
