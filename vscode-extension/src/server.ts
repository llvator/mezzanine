import * as vscode from 'vscode';
import { ChildProcess, spawn } from 'child_process';
import * as http from 'http';
import * as net from 'net';
import * as fs from 'fs';

const STARTUP_TIMEOUT_MS = 120_000;
const POLL_INTERVAL_MS = 500;
const PORT_SCAN_LIMIT = 50;

/**
 * Manages the mezz watch server as a child process.
 * Reuses the existing Axum HTTP server — the extension just spawns it
 * and the webview talks to it over HTTP (same protocol as the standalone app).
 */
export class MezzServer {
  private process: ChildProcess | undefined;
  private _running = false;
  // Mutable: starts as the configured/preferred port, may shift to a free
  // one if the preferred port is already serving a different workspace
  // (e.g. another VS Code window). Finalized before `start()` resolves.
  public port: number;

  constructor(
    private readonly binaryPath: string,
    private readonly workspaceRoot: string,
    preferredPort: number,
    private readonly includeTests: boolean,
    /** Analyze Markdown too, passed as `mezz watch --include-docs`. Widens the
     *  analysis, so it survives a `language` filter — see the engine's
     *  `AnalysisConfig::accepts_language`. */
    private readonly includeDocs: boolean,
    private readonly output: vscode.OutputChannel,
    /** Path to the Educator content corpus shipped with the extension. Passed
     *  to `mezz watch` as `--content-fallback` so rules/lessons work in
     *  workspaces that don't have their own `content/` directory. */
    private readonly contentFallback?: string,
    /** Optional single-language filter, passed to `mezz watch --language`.
     *  Empty/undefined means analyze all detected languages. */
    private readonly language?: string
  ) {
    this.port = preferredPort;
  }

  get running(): boolean {
    return this._running;
  }

  async start(): Promise<void> {
    if (this._running) return;

    // Only reuse an existing server when it is mezz AND is analyzing this
    // exact workspace. Otherwise pick a free port so each VS Code window
    // can run its own instance against its own folder.
    if (await isPortReachable(this.port)) {
      const remoteRoot = await fetchRemoteRoot(this.port);
      if (remoteRoot && pathsMatch(remoteRoot, this.workspaceRoot)) {
        this.output.appendLine(
          `Port ${this.port} is already serving mezz for this workspace — reusing it.`
        );
        this._running = true;
        return;
      }

      const detail = remoteRoot
        ? `serving a different workspace (${remoteRoot})`
        : 'occupied by a non-mezz process';
      const freePort = await findFreePort(this.port + 1);
      this.output.appendLine(
        `Port ${this.port} is ${detail}; switching to free port ${freePort}.`
      );
      this.port = freePort;
    }

    // Show output panel so the user sees analysis progress
    this.output.show(true);
    this.output.appendLine('─────────────────────────────────────────────');
    this.output.appendLine(`Starting mezz server for: ${this.workspaceRoot}`);

    const args = ['watch', this.workspaceRoot, '--port', String(this.port)];
    if (this.includeTests) args.push('--include-tests');
    if (this.includeDocs) args.push('--include-docs');
    if (this.language && this.language.trim()) {
      args.push('--language', this.language.trim());
    }
    if (this.contentFallback && fs.existsSync(this.contentFallback)) {
      args.push('--content-fallback', this.contentFallback);
    }
    this.output.appendLine(`Command: ${this.binaryPath} ${args.join(' ')}`);

    const spawnError = await new Promise<Error | null>((resolve) => {
      try {
        this.process = spawn(this.binaryPath, args, {
          cwd: this.workspaceRoot,
          stdio: ['ignore', 'pipe', 'pipe'],
        });
      } catch (err) {
        return resolve(err as Error);
      }

      this.process.stderr?.on('data', (data: Buffer) => this.output.append(data.toString()));
      this.process.stdout?.on('data', (data: Buffer) => this.output.append(data.toString()));

      this.process.on('error', (err) => {
        this._running = false;
        resolve(err);
      });

      this.process.on('exit', (code, signal) => {
        this._running = false;
        this.output.appendLine(`\n[mezz exited: code=${code} signal=${signal}]`);
      });

      // Give the process a tick to either fail fast (e.g. ENOENT) or get going
      setTimeout(() => resolve(null), 200);
    });

    if (spawnError) {
      // Deliberately does not advise a fix: the caller knows where this
      // binary came from and says something accurate about it, whereas from
      // here every install looks the same. See startupFailureMessage.
      throw new Error(`Could not launch '${this.binaryPath}': ${spawnError.message}.`);
    }

    // Poll the HTTP endpoint until the server responds
    const deadline = Date.now() + STARTUP_TIMEOUT_MS;
    while (Date.now() < deadline) {
      if (this.process?.exitCode !== null && this.process?.exitCode !== undefined) {
        throw new Error(
          `mezz process exited with code ${this.process.exitCode} before the server became ready. ` +
          `See the "Mezzanine Code Visualizer" output panel for details.`
        );
      }
      if (await isPortReachable(this.port)) {
        this._running = true;
        this.output.appendLine(`\n✓ Server ready on http://localhost:${this.port}`);
        return;
      }
      await sleep(POLL_INTERVAL_MS);
    }

    this.stop();
    throw new Error(
      `mezz server did not become ready within ${STARTUP_TIMEOUT_MS / 1000}s. ` +
      `The initial analysis may take longer on large codebases — check the output panel for progress.`
    );
  }

  stop(): void {
    if (this.process) {
      this.process.kill('SIGTERM');
      this.process = undefined;
    }
    this._running = false;
  }
}

function isPortReachable(port: number): Promise<boolean> {
  return new Promise((resolve) => {
    const req = http.request(
      {
        host: '127.0.0.1',
        port,
        path: '/api/index',
        method: 'GET',
        timeout: 1000,
      },
      (res) => {
        res.resume();
        // Any response means the server is up (even 404/500 is fine)
        resolve(true);
      }
    );
    req.on('error', () => resolve(false));
    req.on('timeout', () => {
      req.destroy();
      resolve(false);
    });
    req.end();
  });
}

/** Asks a presumed-mezz server which workspace it is analyzing.
 *  Returns undefined if the response shape is not recognizable as mezz's. */
function fetchRemoteRoot(port: number): Promise<string | undefined> {
  return new Promise((resolve) => {
    const req = http.request(
      {
        host: '127.0.0.1',
        port,
        path: '/api/root',
        method: 'GET',
        timeout: 1500,
      },
      (res) => {
        if (res.statusCode !== 200) {
          res.resume();
          return resolve(undefined);
        }
        let body = '';
        res.setEncoding('utf-8');
        res.on('data', (chunk) => (body += chunk));
        res.on('end', () => {
          try {
            const parsed = JSON.parse(body);
            const path = typeof parsed?.path === 'string' ? parsed.path : undefined;
            resolve(path);
          } catch {
            resolve(undefined);
          }
        });
      }
    );
    req.on('error', () => resolve(undefined));
    req.on('timeout', () => {
      req.destroy();
      resolve(undefined);
    });
    req.end();
  });
}

/** Resolves symlinks on both sides so `/tmp` vs `/private/tmp` (and similar
 *  macOS firmlinks) don't trigger a false mismatch. Falls back to the raw
 *  string if realpath fails for either side. */
function pathsMatch(a: string, b: string): boolean {
  const norm = (p: string): string => {
    try {
      return fs.realpathSync(p);
    } catch {
      return p;
    }
  };
  return norm(a) === norm(b);
}

/** Finds the first free TCP port at or above `start` by attempting to bind. */
async function findFreePort(start: number): Promise<number> {
  for (let p = start; p < start + PORT_SCAN_LIMIT; p++) {
    if (await isTcpPortFree(p)) return p;
  }
  // Last resort: let the OS assign one.
  return new Promise((resolve, reject) => {
    const srv = net.createServer();
    srv.unref();
    srv.on('error', reject);
    srv.listen(0, '127.0.0.1', () => {
      const addr = srv.address();
      srv.close(() => {
        if (addr && typeof addr === 'object') resolve(addr.port);
        else reject(new Error('Could not determine a free port.'));
      });
    });
  });
}

function isTcpPortFree(port: number): Promise<boolean> {
  return new Promise((resolve) => {
    const srv = net.createServer();
    srv.unref();
    srv.once('error', () => resolve(false));
    srv.listen(port, '127.0.0.1', () => {
      srv.close(() => resolve(true));
    });
  });
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
