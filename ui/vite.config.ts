import { defineConfig } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'
import path from 'path'
import { execSync } from 'node:child_process'

const isVscodeBuild = process.env.VSCODE_BUILD === '1';

/**
 * Stamp the bundle with the commit it was built from.
 *
 * Two frontend builds ship from this config — `ui/dist` for `nao watch`'s
 * browser UI and `webview-dist` for the VS Code webview — and they are
 * refreshed by different commands (`build.sh --ui` vs `install.sh`). Telling
 * a stale bundle from a fresh one by looking at the page was guesswork
 * without this.
 *
 * `unknown` when there's no git, rather than failing a build over a stamp.
 */
function gitCommit(): string {
  try {
    return execSync('git rev-parse --short HEAD', {
      cwd: __dirname,
      stdio: ['ignore', 'pipe', 'ignore'],
    }).toString().trim() || 'unknown';
  } catch {
    return 'unknown';
  }
}

// https://vite.dev/config/
export default defineConfig({
  plugins: [svelte()],
  define: {
    __NAO_UI_COMMIT__: JSON.stringify(gitCommit()),
    __NAO_UI_BUILT_AT__: JSON.stringify(new Date().toISOString()),
    __NAO_UI_TARGET__: JSON.stringify(isVscodeBuild ? 'webview' : 'browser'),
  },
  build: isVscodeBuild
    ? {
        // VS Code webview build: output to the extension's webview-dist/ folder
        outDir: path.resolve(__dirname, '../vscode-extension/webview-dist'),
        emptyOutDir: true,
      }
    : undefined,
  server: {
    proxy: {
      // When running alongside `nao watch --port 3000`, proxy the SSE
      // endpoint and API endpoints so the Vite dev server acts as the single
      // origin. Data files are served from ui/public/ by Vite's static file
      // serving, so `nao analyze -o ui/public/data.json` works without the
      // watch server running.
      '/events': {
        target: 'http://localhost:3000',
        changeOrigin: true,
      },
      '/api': {
        target: 'http://localhost:3000',
        changeOrigin: true,
      },
    },
  },
})
