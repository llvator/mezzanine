# The nao visualizer

The browser UI: a Svelte app that renders the code graph, the scope tree, the
quality report and the diff overlay. It is a **client of the engine's HTTP
API**, not a part of the engine. Nothing in `src/` imports anything from here,
and nothing here is compiled into the `nao` binary.

## Two build targets

```bash
npm run build                  # → ui/dist            — the browser UI
VSCODE_BUILD=1 npm run build   # → ../vscode-extension/webview-dist
```

Or, from the repo root, `./scripts/build.sh --ui` and `--webview`.

Same source, two destinations. `install.sh` refreshes the webview one; the
browser one is yours to build when you want it.

## The standalone build

`ui/dist` is self-contained: plain `index.html` plus hashed assets, no server
runtime, no build step at load time. Serve it from anywhere.

```bash
npm run build
npx vite preview --port 4173          # or python3 -m http.server, or any static host
```

Then start an engine and tell it to answer that origin:

```bash
nao watch . --port 3200 --allow-origin http://localhost:4173
```

Open `http://localhost:4173`. The first thing you see is the connect screen —
type `3200` and it checks the engine before committing, then remembers it.
`?api=http://localhost:3200` skips the screen for a session, which is the
useful form for a bookmark or a demo.

The `--allow-origin` flag is not ceremony. Both servers bind loopback, and
loopback is not a boundary against a browser: the browser is on the same
machine, so without an allowlist any page you have open could read
`/api/details`, which returns source code. See
[guide/getting-started.md](../guide/getting-started.md#nao-cli) for the whole
story, including the pairing token that non-loopback origins also need.

**On a hosted page**, an `https://` site reaching `http://localhost` is gated
by the browser's Local Network Access permission — measured, with versions, in
ADR 0006.
Loopback-to-loopback, which is what the recipe above is, is unaffected.

## Development

```bash
nao watch . --port 3000            # terminal 1
npm run dev -- --port 5199         # terminal 2
```

`vite.config.ts` proxies `/api` and `/events` to port 3000, so the dev server
is a single origin and no flags are needed. `?api=` works here too and makes
the proxy redundant if you'd rather point at an engine directly.

## Checks

```bash
npx svelte-check                       # type errors
npm run build                          # bundles cleanly
node scripts/ux-probe.mjs --all        # layout, themes, contrast, connect screen
```

`ux-probe.mjs` drives a real Chrome over CDP with no dependencies. It asserts
what `svelte-check` cannot — panel widths, wrapped toolbar rows, clipped nodes,
label contrast in every theme, and the connect screen's failure states. Run
`node scripts/ux-probe.mjs` with no arguments to list the suites. See
[CONTRIBUTING.md](../CONTRIBUTING.md#layout-probes) before adding a check.

## Where things live

| | |
|---|---|
| `src/endpoint.ts` | Which engine this page talks to, and how it was chosen |
| `src/vscodeAdapter.ts` | `apiUrl()`, and everything the VS Code webview needs |
| `src/stores/` | Data loading, scope, filters, diff, quality, live reload |
| `src/viewmodels/` | Derived view state, kept out of the components |
| `src/components/` | The panels and the canvas |
| `scripts/ux-probe.mjs` | The CDP probe harness |

Colours come from the theme's CSS custom properties. Don't write a literal and
don't invent a property name — CONTRIBUTING has the rule and the reason.
