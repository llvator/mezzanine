/**
 * What this bundle is, and what it is talking to.
 *
 * Two frontend bundles ship from one source tree — `ui/dist` for `mezz watch`'s
 * browser UI, `webview-dist` for the VS Code webview — and they are refreshed
 * by different commands. Add the engine, installed separately again, and
 * "which of these three is stale?" was a question the running app gave no way
 * to answer. It does now: the stamp in the corner names all of them.
 */

// Injected by `define` in vite.config.ts. Declared here rather than in an
// ambient .d.ts so the one file that reads them also documents them.
declare const __MEZZ_UI_COMMIT__: string;
declare const __MEZZ_UI_BUILT_AT__: string;
declare const __MEZZ_UI_TARGET__: string;

export interface UiBuild {
  /** Short commit the bundle was built from, or `unknown` outside git. */
  commit: string;
  /** ISO timestamp of the build. */
  builtAt: string;
  /** Which of the two bundles this is: `browser` or `webview`. */
  target: string;
}

/** `dev` under `vite dev`, where the defines are still substituted but the
 *  bundle is rebuilt per request and a build time means little. */
export const uiBuild: UiBuild = {
  commit: typeof __MEZZ_UI_COMMIT__ === 'string' ? __MEZZ_UI_COMMIT__ : 'dev',
  builtAt: typeof __MEZZ_UI_BUILT_AT__ === 'string' ? __MEZZ_UI_BUILT_AT__ : '',
  target: typeof __MEZZ_UI_TARGET__ === 'string' ? __MEZZ_UI_TARGET__ : 'dev',
};

/** Local time, to the minute — the stamp is read next to a terminal where a
 *  rebuild just happened, so the useful comparison is against the clock on
 *  the wall rather than against UTC. */
export function formatBuiltAt(iso: string): string {
  if (!iso) return 'dev server';
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString(undefined, {
    month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit',
  });
}
