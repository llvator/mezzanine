import * as vscode from 'vscode';
import * as fs from 'fs';
import * as path from 'path';

/**
 * Where the `mezz` engine comes from, in priority order.
 *
 * The extension used to spawn the bare string `mezz` and hope PATH held it,
 * which made "install Rust, then compile for five minutes" a prerequisite for
 * clicking an icon in the activity bar. Marketplace builds now carry the
 * engine, so the common case has no prerequisite at all.
 *
 * The order below is not the obvious one — bundled beats PATH — and that is
 * deliberate. `mezz` on crates.io is somebody else's program (a task runner),
 * so a PATH hit is not evidence of *this* mezz. Preferring a binary we shipped
 * ourselves means a machine that happens to have the other one installed
 * still gets a working extension rather than a baffling failure from a
 * program that has never heard of `watch --content-fallback`.
 */
export type BinarySource = 'setting' | 'bundled' | 'path';

export interface ResolvedBinary {
  /** What to hand to `spawn`. Either an absolute path or the bare name. */
  command: string;
  source: BinarySource;
}

/** `mezz.exe` on Windows, `mezz` everywhere else. */
function exeName(): string {
  return process.platform === 'win32' ? 'mezz.exe' : 'mezz';
}

/**
 * The engine shipped inside this extension, if this build has one.
 *
 * Only the platform-specific Marketplace packages carry it: CI builds one
 * VSIX per target and drops the matching binary in `bin/`. A VSIX packaged
 * from a checkout (`npm run package`) has no `bin/`, falls through to PATH,
 * and so keeps working for anyone developing against a `cargo install`ed
 * build — the same command, resolved the way it always was.
 */
function bundled(context: vscode.ExtensionContext): string | undefined {
  const candidate = path.join(context.extensionPath, 'bin', exeName());
  return fs.existsSync(candidate) ? candidate : undefined;
}

/**
 * Make the bundled engine executable.
 *
 * A VSIX is a zip, and the mode bits do not reliably survive the round trip
 * through packaging and extraction — the file arrives readable and not
 * executable, and the spawn fails with EACCES rather than anything that
 * names the cause. Setting the bit on every activation is cheaper than
 * detecting whether this particular install needed it.
 *
 * Failure is not fatal: a read-only extensions directory is somebody's
 * deliberate lockdown, and the spawn below will produce the real diagnosis.
 */
function ensureExecutable(binary: string, output: vscode.OutputChannel): void {
  if (process.platform === 'win32') return;
  try {
    const mode = fs.statSync(binary).mode;
    if ((mode & 0o111) !== 0o111) {
      fs.chmodSync(binary, 0o755);
      output.appendLine(`[mezz] made the bundled engine executable: ${binary}`);
    }
  } catch (err) {
    output.appendLine(`[mezz] could not set the executable bit on ${binary}: ${err}`);
  }
}

/**
 * Resolve the engine to run, most specific source first.
 *
 * `mezz.binaryPath` wins outright and is not checked for existence: someone
 * who set it wants that path used, and a typo should surface as a spawn
 * error naming what they typed rather than as a silent fall-through to a
 * different engine than the one they asked for.
 */
export function resolveMezzBinary(
  context: vscode.ExtensionContext,
  output: vscode.OutputChannel
): ResolvedBinary {
  const configured = vscode.workspace.getConfiguration('mezz').get<string>('binaryPath', '').trim();
  if (configured) {
    return { command: configured, source: 'setting' };
  }

  const shipped = bundled(context);
  if (shipped) {
    ensureExecutable(shipped, output);
    return { command: shipped, source: 'bundled' };
  }

  return { command: exeName(), source: 'path' };
}

/**
 * What to tell the user when the engine will not start.
 *
 * Each source fails for its own reason and has its own fix, so a single
 * "make sure mezz is on your PATH" is wrong in two cases out of three — it
 * is actively misleading for a Marketplace install, where PATH was never
 * consulted and the user has no reason to have heard of the CLI.
 */
export function startupFailureMessage(resolved: ResolvedBinary, err: unknown): string {
  const detail = err instanceof Error ? err.message : String(err);
  switch (resolved.source) {
    case 'setting':
      return `Mezzanine: could not start the engine at '${resolved.command}', which is what ` +
             `'mezz.binaryPath' points at. Correct or clear that setting.\n${detail}`;
    case 'bundled':
      return `Mezzanine: the engine bundled with this extension would not start ` +
             `(${resolved.command}). This is a packaging or permissions problem rather ` +
             `than anything you did — please report it.\n${detail}`;
    case 'path':
      return `Mezzanine: no engine found. This build of the extension does not bundle one, ` +
             `so it looked for '${resolved.command}' on your PATH. Install the CLI, or ` +
             `set 'mezz.binaryPath' to a built binary.\n${detail}`;
  }
}
