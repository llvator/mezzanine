#!/usr/bin/env node
/**
 * ux-probe — layout and visual assertions for the standalone web UI.
 *
 * Exists because the acceptance criteria in UI-009…UI-022 are pixel- and
 * layout-level ("no label is clipped", "the toolbar is at most two rows",
 * "scroll height under 2x client height"). Those rot into "looks fine to me"
 * without something to run, and the tickets are meant to be grabbable by
 * agents who can't apply visual judgement.
 *
 * Zero dependencies: Node's built-in WebSocket (Node >= 22) speaking CDP to
 * an already-installed Chrome. Nothing added to package.json.
 *
 *   Terminal 1:  nao watch . --port 3000
 *   Terminal 2:  cd ui && npm run dev -- --port 5199 --strictPort
 *   Terminal 3:  node ui/scripts/ux-probe.mjs --all
 *                node ui/scripts/ux-probe.mjs ui-011 ui-020
 *                node ui/scripts/ux-probe.mjs ui-011 --keep-open
 *
 * Exit code is the number of failed checks, so CI can gate on it.
 *
 * ── DOM contract ────────────────────────────────────────────────────────
 * Probes prefer `[data-probe="<name>"]` and fall back to today's structural
 * classes. The fallbacks are fragile by construction — generated Svelte
 * class names and index-based checkbox lookups. When a ticket restructures a
 * region, add the `data-probe` attribute for that region as part of the
 * change; the fallback then stops being load-bearing.
 *
 *   app-root · left-panel · details-panel · right-panel · canvas · toolbar
 *   stats-bar · mode-bar · sidebar-top · scope-tree · quality-table
 *   pin-toggle · search-input · search-results · legend
 *   scope-query · file-filter · file-tree
 *   quality-summary · metric-threshold · quality-population
 *   quality-chart · tick · chart-legend · chart-empty
 *
 * Checks that need human eyes are NOT modelled here. Each ticket lists them
 * separately under "Manual".
 */

import { existsSync } from 'node:fs';

const CDP_PORT = Number(process.env.PROBE_CDP_PORT ?? 9222);
const CDP = `http://127.0.0.1:${CDP_PORT}`;
const APP = process.env.PROBE_URL ?? 'http://localhost:5199/';
/** The engine, addressed directly rather than through the dev-server proxy.
 *  UI-034's suite needs a *cross-origin* nao to make a refusal happen. */
const ENGINE = process.env.PROBE_ENGINE ?? 'http://localhost:3000';

const CHROME = [
  '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
  '/Applications/Chromium.app/Contents/MacOS/Chromium',
  '/usr/bin/google-chrome',
  '/usr/bin/chromium',
].find((p) => existsSync(p));

// ── CDP plumbing ─────────────────────────────────────────────────────────

class Page {
  #ws; #id = 0; #pending = new Map();

  static async open(url) {
    const t = await (await fetch(`${CDP}/json/new?${encodeURIComponent(url)}`, { method: 'PUT' })).json();
    const p = new Page();
    p.targetId = t.id;
    p.#ws = new WebSocket(t.webSocketDebuggerUrl);
    p.#ws.addEventListener('message', (e) => {
      const m = JSON.parse(e.data);
      const slot = m.id && p.#pending.get(m.id);
      if (!slot) return;
      p.#pending.delete(m.id);
      m.error ? slot.reject(new Error(JSON.stringify(m.error))) : slot.resolve(m.result);
    });
    await new Promise((res, rej) => {
      p.#ws.addEventListener('open', res, { once: true });
      p.#ws.addEventListener('error', rej, { once: true });
    });
    await p.send('Page.enable');
    await p.send('Runtime.enable');
    return p;
  }

  send(method, params = {}) {
    const id = ++this.#id;
    this.#ws.send(JSON.stringify({ id, method, params }));
    return new Promise((resolve, reject) => {
      this.#pending.set(id, { resolve, reject });
      setTimeout(() => this.#pending.delete(id) && reject(new Error(`CDP timeout: ${method}`)), 30_000);
    });
  }

  /** Evaluate in page. `fn` is serialised, so it must be self-contained. */
  async eval(fn, ...args) {
    const expression = `(${fn.toString()})(${args.map((a) => JSON.stringify(a)).join(',')})`;
    const r = await this.send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description ?? 'eval threw');
    return r.result?.value;
  }

  async viewport(width, height) {
    await this.send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: 1, mobile: false });
  }

  async goto(url) {
    await this.send('Page.navigate', { url });
  }

  close() {
    return fetch(`${CDP}/json/close/${this.targetId}`).catch(() => {});
  }
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function ensureChrome() {
  try {
    await fetch(`${CDP}/json/version`);
    return null; // already running
  } catch { /* fall through */ }
  if (!CHROME) throw new Error('No Chrome/Chromium found. Set PROBE_CDP_PORT against your own instance.');
  const { spawn } = await import('node:child_process');
  const proc = spawn(CHROME, [
    '--headless=new', '--disable-gpu', '--hide-scrollbars',
    `--remote-debugging-port=${CDP_PORT}`,
    '--user-data-dir=/tmp/nao-ux-probe-profile',
    'about:blank',
  ], { stdio: 'ignore', detached: true });
  proc.unref();
  for (let i = 0; i < 40; i++) {
    await sleep(250);
    try { await fetch(`${CDP}/json/version`); return proc; } catch { /* retry */ }
  }
  throw new Error('Chrome did not expose a debugging port');
}

// ── In-page probe library ────────────────────────────────────────────────
// Installed once per navigation. Everything below runs in the browser.

function installProbeLib() {
  const q = (name, fallback) =>
    document.querySelector(`[data-probe="${name}"]`) ?? (fallback ? document.querySelector(fallback) : null);

  const FALLBACK = {
    'app-root': '.app-root',
    'left-panel': '.left-panel',
    'right-panel': '.right-panel',
    // NOT '.graph-container, svg' — querySelector returns the first match in
    // document order, and the sidebar's chevron icons are <svg> too.
    'canvas': '.graph-container',
    'toolbar': '.controls',
    'stats-bar': '.stats',
    'mode-bar': '.mode-bar-bottom',
    'sidebar-top': '.sidebar-top',
    'details-panel': '.details-panel',
    'legend': null,
    'search-input': null,
    'scope-tree': null,
    'quality-table': null,
  };

  const el = (name) => q(name, FALLBACK[name] ?? null);

  const box = (name) => {
    const e = el(name);
    if (!e) return null;
    const r = e.getBoundingClientRect();
    return { x: r.x, y: r.y, w: r.width, h: r.height, right: r.right, bottom: r.bottom };
  };

  // sRGB relative luminance → WCAG contrast ratio.
  const lum = (rgb) => {
    const c = rgb.map((v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; });
    return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
  };
  const parse = (s) => {
    const m = String(s).match(/rgba?\(([^)]+)\)/);
    if (m) { const p = m[1].split(',').map(Number); return [p[0], p[1], p[2], p[3] ?? 1]; }
    const h = String(s).trim().replace('#', '');
    if (h.length === 3) return [...h].map((c) => parseInt(c + c, 16)).concat(1);
    if (h.length === 6) return [0, 2, 4].map((i) => parseInt(h.slice(i, i + 2), 16)).concat(1);
    return null;
  };
  const contrast = (fg, bg) => {
    const a = parse(fg), b = parse(bg);
    if (!a || !b) return null;
    // Composite fg over bg if translucent.
    const f = a[3] < 1 ? [0, 1, 2].map((i) => a[i] * a[3] + b[i] * (1 - a[3])) : a.slice(0, 3);
    const [l1, l2] = [lum(f), lum(b.slice(0, 3))].sort((x, y) => y - x);
    return (l1 + 0.05) / (l2 + 0.05);
  };

  const canvasBg = () => {
    const c = el('canvas') ?? document.body;
    let n = c;
    while (n) {
      const bg = getComputedStyle(n).backgroundColor;
      if (bg && bg !== 'rgba(0, 0, 0, 0)' && bg !== 'transparent') return bg;
      n = n.parentElement;
    }
    return getComputedStyle(document.body).backgroundColor;
  };

  window.__probe = {
    box, contrast, canvasBg,
    el: (n) => !!el(n),
    count: (sel) => document.querySelectorAll(sel).length,
    text: (sel) => [...document.querySelectorAll(sel)].map((e) => e.textContent.replace(/\s+/g, ' ').trim()),

    /** Distinct top offsets among a container's children — i.e. wrapped rows. */
    rows(name) {
      const e = el(name);
      if (!e) return null;
      const tops = new Set([...e.children].map((c) => Math.round(c.getBoundingClientRect().top)));
      return tops.size;
    },

    scrollRatio(name) {
      const e = el(name);
      if (!e || !e.clientHeight) return null;
      return { scrollH: e.scrollHeight, clientH: e.clientHeight, ratio: +(e.scrollHeight / e.clientHeight).toFixed(2) };
    },

    overlap(a, b) {
      const A = box(a), B = box(b);
      if (!A || !B) return null;
      const ox = Math.min(A.right, B.right) - Math.max(A.x, B.x);
      const oy = Math.min(A.bottom, B.bottom) - Math.max(A.y, B.y);
      return ox > 0 && oy > 0 ? { x: Math.round(ox), y: Math.round(oy) } : null;
    },

    /** Graph nodes (circle + its labels) falling outside the viewport. */
    clippedNodes() {
      const vw = window.innerWidth, vh = window.innerHeight;
      const canvasBox = box('canvas') ?? { x: 0, y: 0, right: vw, bottom: vh };
      const out = [];
      for (const g of document.querySelectorAll('g.node')) {
        const r = g.getBoundingClientRect();
        if (!r.width) continue;
        if (r.left < canvasBox.x - 0.5 || r.top < canvasBox.y - 0.5 ||
            r.right > canvasBox.right + 0.5 || r.bottom > canvasBox.bottom + 0.5) {
          out.push(g.textContent.replace(/\s+/g, ' ').trim().slice(0, 40));
        }
      }
      return out;
    },

    /** Rendered graph nodes, excluding anything the layout has hidden. */
    renderedNodes() {
      return [...document.querySelectorAll('g.node')]
        .filter((g) => getComputedStyle(g).display !== 'none' && g.getBoundingClientRect().width > 0).length;
    },

    /** Distinct circle radii and fills — proxy for "is anything encoded?". */
    nodeEncoding() {
      const cs = [...document.querySelectorAll('g.node circle')];
      return {
        n: cs.length,
        radii: [...new Set(cs.map((c) => c.getAttribute('r')))].length,
        fills: [...new Set(cs.map((c) => c.getAttribute('fill')))].length,
      };
    },

    /** Legend size-ramp dot diameters against the canvas radii they stand
     *  for. The dots are drawn to scale but shrunk, so the test is that one
     *  ratio holds for every stop — a per-dot clamp passes "diameters differ"
     *  on some domains while still flattening the top of the ramp. */
    legendDots() {
      const dots = [...document.querySelectorAll('[data-probe="legend"] .size-dot')];
      const px = dots.map((d) => +d.getBoundingClientRect().width.toFixed(2));
      const cs = [...document.querySelectorAll('g.node circle')].map((c) => +c.getAttribute('r'));
      const rMax = cs.length ? Math.max(...cs) : 0;
      return {
        px,
        distinct: [...new Set(px)].length,
        ascending: px.every((w, i) => i === 0 || w > px[i - 1]),
        // Largest dot / largest node radius — the shared shrink factor.
        scale: rMax ? +(px[px.length - 1] / 2 / rMax).toFixed(3) : null,
      };
    },

    /** Node name-label fill vs the canvas background it sits on. */
    labelContrast() {
      const t = document.querySelector('g.node text.name-label');
      if (!t) return null;
      const fg = t.getAttribute('fill') ?? getComputedStyle(t).fill;
      const bg = canvasBg();
      return { fg, bg, ratio: +(contrast(fg, bg) ?? 0).toFixed(2) };
    },

    linkLabels: () => ({
      labels: document.querySelectorAll('.link-label-container').length,
      visible: [...document.querySelectorAll('.link-label-container')]
        .filter((e) => getComputedStyle(e).display !== 'none' && e.getBoundingClientRect().width > 0).length,
      edges: document.querySelectorAll('.links-group line').length,
    }),

    /** Scope-tree folder rows currently in the sidebar's visible box. */
    scopeRoots() {
      const top = el('sidebar-top');
      if (!top) return { total: 0, visible: 0, names: [] };
      const box_ = top.getBoundingClientRect();
      const rows = [...top.querySelectorAll('input[type=checkbox]')]
        .map((cb) => ({ cb, row: cb.closest('div'), label: cb.closest('div')?.textContent.replace(/\s+/g, ' ').trim() ?? '' }))
        .filter((r) => /📁/.test(r.label));
      const visible = rows.filter((r) => {
        const b = r.row.getBoundingClientRect();
        return b.top >= box_.top - 0.5 && b.bottom <= box_.bottom + 0.5 && b.height > 0;
      });
      // Strip the disclosure arrow, folder glyph and trailing entity count so
      // the name is comparable against prose elsewhere in the UI.
      const clean = (s) => s.replace(/[▶▼📁⚠]/g, ' ').replace(/\s+[\d.]+k?\s*$/, '').replace(/\s+/g, ' ').trim();
      return { total: rows.length, visible: visible.length, names: rows.map((r) => clean(r.label)) };
    },

    /** Toggle buttons in the toolbar and whether they expose state. */
    toggleState(labels) {
      const btns = [...document.querySelectorAll('.control-btn, [data-probe="toolbar"] button')];
      return labels.map((want) => {
        const b = btns.find((x) => x.textContent.trim() === want);
        if (!b) return { label: want, found: false };
        return {
          label: want, found: true,
          // `pressed` is the VALUE; `exposesState` is whether the control
          // communicates a value at all. Conflating the two — reading
          // hasAttribute('aria-pressed') as "on" — made aria-pressed="false"
          // count as pressed, so an honestly-off toggle read as on.
          pressed: b.classList.contains('active') || b.getAttribute('aria-pressed') === 'true',
          exposesState: b.classList.contains('active')
            || b.hasAttribute('aria-pressed') || b.hasAttribute('aria-checked'),
        };
      });
    },

    /**
     * Every visible text node in the sidebar/overlays with its effective
     * foreground/background contrast.
     *
     * Walks up for the first non-transparent background rather than trusting
     * the element's own, which is usually transparent. Uses the WCAG large-text
     * allowance (3:1 at >=24px, or >=18.66px bold) so headings aren't reported
     * as failures for being large.
     */
    contrastAudit(minRatio = 4.5) {
      const roots = ['[data-probe="sidebar-top"]', '[data-probe="details-panel"]',
                     '.sidebar', '.overlay-card'];
      // Resolve the palette to rgb() so computed styles can be matched to it.
      const rootStyle = getComputedStyle(document.documentElement);
      const probeEl = document.createElement('span');
      document.body.appendChild(probeEl);
      const tokenNames = new Map();
      for (const name of ['--text', '--text-secondary', '--text-muted', '--text-dim',
                          '--text-disabled', '--accent', '--border', '--border-subtle']) {
        const raw = rootStyle.getPropertyValue(name).trim();
        if (!raw) continue;
        probeEl.style.color = raw;
        const resolved = getComputedStyle(probeEl).color;
        if (!tokenNames.has(resolved)) tokenNames.set(resolved, name);
      }
      probeEl.remove();
      const seen = new Set();
      const fails = [];
      let checked = 0;
      for (const sel of roots) {
        for (const root of document.querySelectorAll(sel)) {
          for (const e of root.querySelectorAll('*')) {
            if (seen.has(e)) continue;
            seen.add(e);
            // Only elements that render their own text.
            const own = [...e.childNodes]
              .filter((n) => n.nodeType === 3 && n.textContent.trim())
              .map((n) => n.textContent.trim()).join(' ');
            if (!own) continue;
            const cs = getComputedStyle(e);
            if (cs.display === 'none' || cs.visibility === 'hidden' || +cs.opacity === 0) continue;
            const r = e.getBoundingClientRect();
            if (!r.width || !r.height) continue;
            // Composite every translucent layer down to an opaque colour.
            // Taking the first non-transparent background and using its rgb
            // as-is treats rgba(255,152,0,0.08) — an 8% tint over a white
            // panel — as solid orange, which invents failures that aren't
            // there. Tinted badges and banners are common here, so this was
            // not a rare edge case.
            const layers = [];
            for (let n = e; n; n = n.parentElement) {
              const c = getComputedStyle(n).backgroundColor;
              if (!c || c === 'transparent') continue;
              const p = parse(c);
              if (!p || p[3] === 0) continue;
              layers.push(p);
              if (p[3] >= 1) break;
            }
            if (layers.length === 0) layers.push(parse(getComputedStyle(document.body).backgroundColor) ?? [255, 255, 255, 1]);
            // Farthest ancestor first, then paint each nearer layer over it.
            let acc = layers[layers.length - 1].slice(0, 3);
            for (let i = layers.length - 2; i >= 0; i--) {
              const l = layers[i];
              acc = [0, 1, 2].map((k) => l[k] * l[3] + acc[k] * (1 - l[3]));
            }
            const bg = `rgb(${acc.map((v) => Math.round(v)).join(', ')})`;
            const ratio = contrast(cs.color, bg);
            if (ratio == null) continue;
            checked++;
            const px = parseFloat(cs.fontSize);
            const bold = (parseInt(cs.fontWeight, 10) || 400) >= 700;
            const limit = (px >= 24 || (px >= 18.66 && bold)) ? 3.0 : minRatio;
            if (ratio < limit) {
              fails.push({
                text: own.slice(0, 34),
                cls: (typeof e.className === 'string' ? e.className : '').split(' ')[0] || e.tagName.toLowerCase(),
                fg: cs.color, bg, ratio: +ratio.toFixed(2), need: limit,
                // Is this foreground one of the theme's own tokens, or a
                // colour written into a component? The two need different
                // fixes and different owners: a token failing is a palette
                // decision affecting every screen, a component literal is a
                // local bug. UI-023 owns the second; the first is UI-024.
                fromToken: tokenNames.get(cs.color) ?? null,
              });
            }
          }
        }
      }
      const hardcoded = fails.filter((x) => !x.fromToken);
      const tokenFails = fails.filter((x) => x.fromToken);
      const byToken = {};
      for (const x of tokenFails) byToken[x.fromToken] = (byToken[x.fromToken] ?? 0) + 1;
      return {
        checked,
        failed: fails.length,
        hardcodedFailed: hardcoded.length,
        tokenFailed: tokenFails.length,
        byToken,
        hardcoded: hardcoded.slice(0, 20),
      };
    },

    // ── scope query + file tree (UI-042..047) ────────────────────────────

    /** Result rows the scope panel is currently showing. `entity` is the
     *  entity name when the row came from the name corpus rather than the
     *  path corpus. */
    scopeRows() {
      return [...document.querySelectorAll('.scope-tree .tree-item')].map((r) => ({
        path: r.querySelector('.label')?.getAttribute('title') ?? '',
        entity: r.querySelector('.entity-name')?.textContent.trim() ?? null,
        checked: !!r.querySelector('.select-cb')?.checked,
        direct: r.classList.contains('selected'),
      }));
    },

    /** Match count, overflow notice and the pre-commit entity projection. */
    scopeStatus() {
      const bar = document.querySelector('.match-status');
      if (!bar) return { active: false };
      const count = bar.querySelector('.match-count')?.textContent.trim() ?? null;
      const proj = bar.querySelector('.projection');
      return {
        active: true,
        matches: count ? Number(count.match(/^(\d+)/)?.[1] ?? NaN) : 0,
        truncated: !!bar.querySelector('.truncated'),
        projection: proj?.textContent.trim() ?? null,
        projectionOver: !!proj?.classList.contains('over'),
      };
    },

    /** Entities the current selection covers, from the scope tree's own
     *  stats line — the number a commit has to agree with. */
    scopeEntities() {
      const t = document.querySelector('.sel-stats')?.textContent ?? '';
      const m = t.match(/In scope:\s*([\d,]+)/);
      return m ? Number(m[1].replace(/,/g, '')) : 0;
    },

    /** Folder rows the scope tree has open, so a test can assert the tree
     *  came back the way the user left it. */
    scopeOpenPaths() {
      return [...document.querySelectorAll('.scope-tree .tree-item')]
        .filter((r) => r.querySelector('.toggle')?.textContent.trim() === '▼')
        .map((r) => r.querySelector('.label')?.getAttribute('title'));
    },

    typeScopeQuery(text) {
      const i = document.querySelector('[data-probe="scope-query"]');
      if (!i) return false;
      i.value = text;
      i.dispatchEvent(new Event('input', { bubbles: true }));
      return true;
    },

    scopeKey(key, shift = false) {
      const i = document.querySelector('[data-probe="scope-query"]');
      if (!i) return false;
      i.dispatchEvent(new KeyboardEvent('keydown', { key, shiftKey: shift, bubbles: true }));
      return true;
    },

    /** Expand a scope folder by full path, idempotently — clicking blind
     *  toggles, so a helper that "expands" twice would collapse. */
    expandScope(path) {
      const row = [...document.querySelectorAll('.scope-tree .tree-item')]
        .find((r) => r.querySelector('.label')?.getAttribute('title') === path);
      const t = row?.querySelector('.toggle');
      if (!t) return false;
      if (t.textContent.trim() === '▶') t.click();
      return true;
    },

    /** Click a scope row's checkbox by full path. */
    toggleScopePath(path) {
      const row = [...document.querySelectorAll('.scope-tree .tree-item')]
        .find((r) => r.querySelector('.label')?.getAttribute('title') === path);
      const cb = row?.querySelector('.select-cb');
      if (!cb) return false;
      cb.click();
      return true;
    },

    /** Whether a scope row reads as selected, and the index count beside it. */
    scopeRowState(path) {
      const row = [...document.querySelectorAll('.scope-tree .tree-item')]
        .find((r) => r.querySelector('.label')?.getAttribute('title') === path);
      if (!row) return null;
      return {
        checked: !!row.querySelector('.select-cb')?.checked,
        direct: row.classList.contains('selected'),
        count: Number((row.querySelector('.count')?.textContent ?? '').trim().replace(/[^\d.]/g, '')),
      };
    },

    openFilesPanel() {
      if (document.querySelector('[data-probe="file-tree"]')) return true;
      const h = [...document.querySelectorAll('h2.section-header')]
        .find((x) => /^\s*Files\b/.test(x.textContent));
      if (!h) return false;
      h.click();
      return true;
    },

    /** File-tree rows with their nesting depth and checkbox tri-state. */
    fileRows() {
      return [...document.querySelectorAll('[data-probe="file-tree"] .tree-item')].map((r) => {
        const cb = r.querySelector('input[type=checkbox]');
        return {
          path: r.querySelector('.ft-label')?.getAttribute('title') ?? '',
          depth: (parseInt(r.style.paddingLeft, 10) || 0) / 12,
          state: cb ? (cb.indeterminate ? 'some' : cb.checked ? 'all' : 'none') : 'missing',
        };
      });
    },

    typeFileQuery(text) {
      const i = document.querySelector('[data-probe="file-filter"]');
      if (!i) return false;
      i.value = text;
      i.dispatchEvent(new Event('input', { bubbles: true }));
      return true;
    },

    setFileVisible(path, visible) {
      const row = [...document.querySelectorAll('[data-probe="file-tree"] .tree-item')]
        .find((r) => r.querySelector('.ft-label')?.getAttribute('title') === path);
      const cb = row?.querySelector('input[type=checkbox]');
      if (!cb) return false;
      cb.checked = visible;
      cb.dispatchEvent(new Event('change', { bubbles: true }));
      return true;
    },

    /** Expand the shallowest still-collapsed ancestor of `path`, and report
     *  whether it did anything. One level per call by design: the children of
     *  a folder don't exist in the DOM until Svelte has re-rendered after the
     *  click, so a synchronous walk down the whole path finds nothing below
     *  the first level. Callers loop with a wait — see `expandFileTree`. */
    expandFileTo(path) {
      const parts = path.split('/');
      for (let i = 1; i <= parts.length; i++) {
        const p = parts.slice(0, i).join('/');
        const row = [...document.querySelectorAll('[data-probe="file-tree"] .tree-item')]
          .find((r) => r.querySelector('.ft-label')?.getAttribute('title') === p);
        const t = row?.querySelector('.ft-toggle');
        if (t && t.textContent.trim() === '▶') { t.click(); return true; }
      }
      return false;
    },

    setLevel(label) {
      const b = [...document.querySelectorAll('button.control-btn')]
        .find((x) => x.textContent.trim() === label);
      if (!b) return false;
      b.click();
      return true;
    },

    // ── actions ──────────────────────────────────────────────────────────

    pickScope(name) {
      const top = el('sidebar-top');
      const hit = [...(top?.querySelectorAll('input[type=checkbox]') ?? [])]
        .find((cb) => new RegExp(`📁\\s*${name}\\b`).test(cb.closest('div')?.textContent ?? ''));
      if (!hit) return false;
      hit.click();
      return true;
    },

    pickTab(name) {
      const t = [...document.querySelectorAll('.tab')].find((x) => x.textContent.trim().toLowerCase() === name.toLowerCase());
      if (!t) return false;
      t.click();
      return true;
    },

    selectNode(name) {
      const g = [...document.querySelectorAll('g.node')].find((n) => n.textContent.includes(name));
      if (!g) return false;
      g.dispatchEvent(new MouseEvent('click', { bubbles: true }));
      return true;
    },

    clickText(text) {
      const hits = [...document.querySelectorAll('button, [role=button], .theme-card, label')]
        .filter((e) => e.textContent.replace(/\s+/g, ' ').trim().includes(text));
      if (!hits.length) return false;
      hits[hits.length - 1].click();
      return true;
    },

    /** Theme is persisted RAW by settings.ts (`setItem(KEY, id)`), not as
     *  JSON. Writing `"light"` with quotes silently falls back to the
     *  default theme and every contrast measurement then reads llvator. */
    setTheme(id) {
      try { localStorage.setItem('nao-theme', id); } catch { /* ignore */ }
      return localStorage.getItem('nao-theme') === id;
    },

    /** Which theme the app actually applied, read back off :root. */
    activeTheme() {
      const s = getComputedStyle(document.documentElement);
      return {
        stored: (() => { try { return localStorage.getItem('nao-theme'); } catch { return null; } })(),
        bgBody: s.getPropertyValue('--bg-body').trim(),
        text: s.getPropertyValue('--text').trim(),
      };
    },
  };
  return true;
}

// ── Runner ───────────────────────────────────────────────────────────────

const ok = (pass, actual, note) => ({ pass, actual, note });

/** Pull the "shown" figure out of the stats bar.
 *  Prefers the labelled `Shown: N` form; falls back to a bare leading count
 *  so the check still means something on an un-migrated bar. Must not match
 *  the "in scope" figure, which also ends in "entities". */
function parseShown(bar) {
  const labelled = bar.match(/Shown:\s*(\d[\d,]*)/i);
  if (labelled) return Number(labelled[1].replace(/,/g, ''));
  const bare = bar.match(/^\s*(\d[\d,]*)\s*(?:entities|nodes)/i);
  return bare ? Number(bare[1].replace(/,/g, '')) : NaN;
}

/** Walk a file-tree path open, one level per pass. `expandFileTo` deliberately
 *  expands a single level per call because the next level's rows only exist
 *  after Svelte re-renders; this drives it to the bottom. */
async function expandFileTree(page, path) {
  for (let i = 0; i <= path.split('/').length; i++) {
    const moved = await page.eval((p) => window.__probe.expandFileTo(p), path);
    if (!moved) return;
    await sleep(300);
  }
}

async function ready(page, ms = 6000) {
  await page.eval(installProbeLib);
  await sleep(ms);
  await page.eval(installProbeLib); // survive the app's own re-render
  // The canvas toolbar's collapsed state persists in localStorage and the
  // probe reuses whatever Chrome instance is already on the debug port, so a
  // previous session — or a human poking at the app — can leave it collapsed
  // and every toolbar assertion then fails on "button not found" rather than
  // on anything real. Expand it before measuring.
  await page.eval(() => {
    const handle = document.querySelector('.canvas-toolbar-handle');
    if (handle && !document.querySelector('[data-probe="toolbar"]')) handle.click();
    return true;
  });
  await sleep(300);
}

/**
 * Each suite: { title, ticket, setup(page), checks: [{ id, criterion, run }] }
 * `run` returns { pass, actual, note } — never throws for a normal failure.
 */
const SUITES = {
  'ui-009': {
    ticket: 'UI-009', title: 'Theme-aware graph canvas',
    async setup(page) {
      await page.viewport(1600, 1000);
      await page.goto(APP); await ready(page);
      await page.eval((n) => window.__probe.pickScope(n), 'ui');
      await sleep(8000);
    },
    checks: [
      {
        id: 'label-contrast-all-themes',
        criterion: 'Node names legible against the canvas in every theme',
        async run(page) {
          const results = {};
          const seenBg = new Set();
          const themes = ['midnight', 'obsidian', 'nord', 'light', 'llvator'];
          for (const t of themes) {
            await page.eval((id) => window.__probe.setTheme(id), t);
            await page.goto(APP); await ready(page);
            await page.eval((n) => window.__probe.pickScope(n), 'ui');
            await sleep(8000);
            const applied = await page.eval(() => window.__probe.activeTheme());
            results[t] = { ...(await page.eval(() => window.__probe.labelContrast())), applied };
            seenBg.add(applied.bgBody);
          }
          // Guard against the whole loop silently measuring one theme.
          if (seenBg.size < themes.length) {
            return ok(false, results, `only ${seenBg.size} distinct backgrounds across ${themes.length} themes — theme switching did not take`);
          }
          const bad = Object.entries(results).filter(([, r]) => !r || !r.ratio || r.ratio < 4.5);
          return ok(bad.length === 0, results, bad.length ? `below 4.5:1 in ${bad.map((b) => b[0]).join(', ')}` : '');
        },
      },
      {
        id: 'no-colour-literals',
        criterion: 'No hardcoded node/label/link colours remain in GraphView',
        async run() {
          const { readFileSync } = await import('node:fs');
          const src = readFileSync(new URL('../src/components/GraphView.svelte', import.meta.url), 'utf8');
          const hits = [...src.matchAll(/\.attr\(\s*'(?:fill|stroke)'\s*,\s*'(#[0-9a-fA-F]{3,8}|rgba?\([^)]*\))'/g)]
            .map((m) => m[1])
            .filter((c) => !/#FF9800/i.test(c)); // order-badge accent is intentionally semantic
          return ok(hits.length === 0, hits, hits.length ? `literals: ${hits.join(', ')}` : '');
        },
      },
    ],
  },

  'ui-010': {
    ticket: 'UI-010', title: 'Entity-count vocabulary',
    async setup(page) {
      await page.viewport(1600, 1000);
      await page.goto(APP); await ready(page);
      await page.eval((n) => window.__probe.pickScope(n), 'ui');
      await sleep(9000);
    },
    checks: [
      {
        id: 'stats-matches-graph',
        criterion: 'Stats bar matches nodes actually drawn (graph view)',
        async run(page) {
          const shown = await page.eval(() => window.__probe.renderedNodes());
          const bar = (await page.eval(() => window.__probe.text('[data-probe="stats-bar"], .stats')))[0] ?? '';
          const n = parseShown(bar);
          return ok(n === shown, { bar, shown, parsed: n });
        },
      },
      {
        id: 'stats-matches-tree',
        criterion: 'Stats bar follows the switch to Tree view',
        async run(page) {
          // Tree view is rooted on the selection; without one it renders the
          // whole graph and the check would pass for the wrong reason.
          const sel = await page.eval(() => window.__probe.selectNode('Sidebar.svelte'));
          if (!sel) return ok(false, null, 'could not select a root node for tree view');
          await sleep(1500);
          await page.eval(() => window.__probe.clickText('Tree View'));
          await sleep(6000);
          const isTree = await page.eval(() =>
            [...document.querySelectorAll('.control-btn, [data-probe="toolbar"] button')]
              .some((b) => b.textContent.trim() === 'Graph View'));
          if (!isTree) return ok(false, null, 'Tree view did not engage');
          const shown = await page.eval(() => window.__probe.renderedNodes());
          const bar = (await page.eval(() => window.__probe.text('[data-probe="stats-bar"], .stats')))[0] ?? '';
          const n = parseShown(bar);
          return ok(n === shown, { bar, shown, parsed: n });
        },
      },
      {
        id: 'counts-are-labelled',
        criterion: 'Every on-screen count names its population',
        async run(page) {
          const bar = (await page.eval(() => window.__probe.text('[data-probe="stats-bar"], .stats')))[0] ?? '';
          const named = /(shown|in scope|indexed|analysed|analyzed|of)\b/i.test(bar);
          return ok(named, bar, named ? '' : 'stats bar states a bare number');
        },
      },
    ],
  },

  'ui-011': {
    ticket: 'UI-011', title: 'Sidebar: active panel owns the height',
    async setup(page) {
      await page.viewport(1600, 1000);
      await page.goto(APP); await ready(page);
    },
    checks: [
      {
        id: 'scope-roots-visible',
        criterion: 'All top-level scope folders reachable without scrolling',
        async run(page) {
          const r = await page.eval(() => window.__probe.scopeRoots());
          return ok(r.total > 0 && r.visible === r.total, r, `${r.visible}/${r.total} visible`);
        },
      },
      {
        // Since UI-040 details is a column of its own, so "the active tab
        // owns the height" is structural rather than a state to reach: there
        // is nothing left in the sidebar to share the height with.
        id: 'active-tab-owns-height',
        criterion: 'The active tab fills the sidebar below the tab bar',
        async run(page) {
          const r = await page.eval(() => {
            const side = document.querySelector('.sidebar');
            const top = document.querySelector('[data-probe="sidebar-top"], .sidebar-top');
            const details = document.querySelector('.sidebar-bottom');
            if (!side || !top) return { found: false };
            const s = side.getBoundingClientRect(), t = top.getBoundingClientRect();
            return {
              found: true,
              // Tab bar is ~33px; anything more unclaimed means something
              // else is still taking height in here.
              slack: Math.round(s.bottom - t.bottom + (t.top - s.top)),
              detailsInSidebar: !!details,
            };
          });
          return ok(!!r.found && !r.detailsInSidebar && r.slack <= 40, r);
        },
      },
      {
        id: 'scroll-ratio-filters',
        criterion: 'Filters tab scroll height under 2x its client height',
        async run(page) {
          const r = await page.eval(() => window.__probe.scrollRatio('sidebar-top'));
          return ok(r && r.ratio < 2.0, r);
        },
      },
      {
        // The sidebar no longer reacts to selection at all — that is the
        // point of UI-040. What must not regress is the scope tree keeping
        // its rows when an entity is picked.
        id: 'selection-does-not-shrink-sidebar',
        criterion: 'Selecting a node leaves the sidebar height untouched',
        async run(page) {
          await page.eval((n) => window.__probe.pickScope(n), 'ui');
          await sleep(9000);
          const before = await page.eval(() => window.__probe.box('sidebar-top'));
          await page.eval(() => window.__probe.selectNode('Sidebar.svelte'));
          await sleep(1500);
          const after = await page.eval(() => window.__probe.box('sidebar-top'));
          const pass = before && after && Math.abs(before.h - after.h) <= 1;
          return ok(pass, { before: before && Math.round(before.h), after: after && Math.round(after.h) });
        },
      },
      {
        id: 'quality-rows-visible',
        criterion: 'Quality problem table shows >= 10 rows without resizing',
        async run(page) {
          await page.eval(() => window.__probe.pickTab('Quality'));
          await sleep(4000);
          const rows = await page.eval(() => {
            const t = document.querySelector('[data-probe="quality-table"]');
            const box = (document.querySelector('[data-probe="sidebar-top"], .sidebar-top'))?.getBoundingClientRect();
            const trs = [...(t ?? document).querySelectorAll('tbody tr, tr')].filter((r) => r.querySelector('td'));
            if (!box) return { total: trs.length, visible: trs.length };
            const vis = trs.filter((r) => {
              const b = r.getBoundingClientRect();
              return b.height > 0 && b.top >= box.top - 0.5 && b.bottom <= box.bottom + 0.5;
            });
            return { total: trs.length, visible: vis.length };
          });
          return ok(rows.visible >= 10, rows);
        },
      },
      {
        id: 'no-empty-sections',
        criterion: 'Entity Types / Relationship Types / Legend absent before a graph loads',
        async run(page) {
          await page.goto(APP); await ready(page);
          const heads = await page.eval(() => window.__probe.text('.sidebar-top .sub-title, .sidebar-top h2, .sidebar-top h3'));
          const orphans = heads.filter((h) => /^(Entity Types|Relationship Types|Legend)$/i.test(h));
          return ok(orphans.length === 0, { headings: heads, orphans });
        },
      },
    ],
  },

  'ui-012': {
    ticket: 'UI-012', title: 'Single entity-details panel',
    async setup(page) {
      await page.viewport(1600, 1000);
      await page.goto(APP); await ready(page);
      await page.eval((n) => window.__probe.pickScope(n), 'ui');
      await sleep(9000);
      await page.eval(() => window.__probe.selectNode('GraphView.svelte'));
      await sleep(2000);
    },
    checks: [
      {
        // The panel this ticket removed was a *second* entity-details panel.
        // Details has since become a column of its own (UI-040), so the
        // invariant is "exactly one of them", not "none on the right".
        id: 'one-details-column',
        criterion: 'Entity details live in exactly one panel',
        async run(page) {
          const n = await page.eval(() => window.__probe.count('[data-probe="details-panel"]'));
          return ok(n === 1, { panels: n });
        },
      },
      {
        id: 'details-rendered-once',
        criterion: 'Selected entity details appear in exactly one place',
        async run(page) {
          const r = await page.eval(() => {
            // Exclude the canvas: every node carries its own name as an SVG
            // <text> label, so counting all leaves reports panel + label and
            // never isolates the panel duplication this check is about.
            const inPanels = [...document.querySelectorAll('*')].filter(
              (e) => e.children.length === 0
                && !(e.ownerSVGElement || e.tagName === 'svg')
                && /^GraphView\.svelte$/.test(e.textContent.trim()),
            );
            return {
              occurrences: inPanels.length,
              where: inPanels.map((e) => e.closest('[data-probe],[class]')?.className || '?'),
            };
          });
          return ok(r.occurrences <= 1, r, r.occurrences > 1 ? 'name rendered in more than one panel' : '');
        },
      },
      {
        id: 'canvas-is-widest-column',
        criterion: 'Canvas stays wider than any single side column',
        async run(page) {
          const [c, l, d, r] = await Promise.all([
            page.eval(() => window.__probe.box('canvas')),
            page.eval(() => window.__probe.box('left-panel')),
            page.eval(() => window.__probe.box('details-panel')),
            page.eval(() => window.__probe.box('right-panel')),
          ]);
          const widest = Math.max(l?.w ?? 0, d?.w ?? 0, r?.w ?? 0);
          return ok(!!c && c.w > widest,
            { canvas: c && Math.round(c.w), widestPanel: Math.round(widest) });
        },
      },
      {
        id: 'mode-indicator',
        criterion: 'Panel states whether it is hovering or pinned, and exposes a pin control',
        async run(page) {
          const r = await page.eval(() => {
            const p = document.querySelector('[data-probe="details-panel"]');
            if (!p) return { found: false };
            const txt = p.textContent.replace(/\s+/g, ' ').trim();
            return {
              found: true,
              // "Selected Entity" as a static heading doesn't count — the
              // panel must say which of the two modes it is currently in.
              statesMode: /\b(hovering|pinned|following cursor)\b/i.test(txt),
              hasPinControl: !!p.querySelector('[data-probe="pin-toggle"]'),
              sample: txt.slice(0, 120),
            };
          });
          return ok(!!r.found && r.statesMode && r.hasPinControl, r);
        },
      },
    ],
  },

  'ui-013': {
    ticket: 'UI-013', title: 'Toolbar grouping and toggle state',
    async setup(page) {
      await page.viewport(1600, 1000);
      await page.goto(APP); await ready(page);
      await page.eval((n) => window.__probe.pickScope(n), 'ui');
      await sleep(9000);
    },
    checks: [
      {
        id: 'toolbar-rows-1600',
        criterion: 'Toolbar occupies at most two rows at 1600px',
        async run(page) {
          const rows = await page.eval(() => window.__probe.rows('toolbar'));
          return ok(rows !== null && rows <= 2, rows);
        },
      },
      {
        id: 'toolbar-rows-1280',
        criterion: 'Toolbar occupies at most two rows at 1280px',
        async run(page) {
          await page.viewport(1280, 800); await sleep(1200);
          const rows = await page.eval(() => window.__probe.rows('toolbar'));
          await page.viewport(1600, 1000);
          return ok(rows !== null && rows <= 2, rows);
        },
      },
      {
        id: 'depth-controls-named',
        criterion: 'Hover depth and tree depth are labelled in words',
        async run(page) {
          const txt = (await page.eval(() => window.__probe.text('[data-probe="toolbar"], .controls'))).join(' ');
          const named = /(highlight|hover)\s*depth/i.test(txt) && /(tree|relationship)\s*depth/i.test(txt);
          return ok(named, txt.slice(0, 200), named ? '' : 'only H1/H2/H3 and L1/L2/L3 present');
        },
      },
    ],
  },

  'ui-014': {
    ticket: 'UI-014', title: 'Metric-driven node encoding',
    async setup(page) {
      await page.viewport(1600, 1000);
      await page.goto(APP); await ready(page);
      await page.eval((n) => window.__probe.pickScope(n), 'ui');
      await sleep(9000);
    },
    checks: [
      {
        id: 'size-varies',
        criterion: 'Node radius varies with a metric at File level',
        async run(page) {
          const e = await page.eval(() => window.__probe.nodeEncoding());
          return ok(e.n > 5 && e.radii >= 4, e, `${e.radii} distinct radii across ${e.n} nodes`);
        },
      },
      {
        id: 'fill-varies',
        criterion: 'Node fill encodes severity at File level',
        async run(page) {
          const e = await page.eval(() => window.__probe.nodeEncoding());
          return ok(e.n > 5 && e.fills >= 3, e, `${e.fills} distinct fills across ${e.n} nodes`);
        },
      },
      {
        id: 'legend-present',
        criterion: 'Legend reflects the active encoding with a value range',
        async run(page) {
          const txt = (await page.eval(() => window.__probe.text('[data-probe="legend"]'))).join(' ');
          return ok(/\d/.test(txt) && txt.length > 0, txt.slice(0, 160), txt ? '' : 'no [data-probe="legend"] found');
        },
      },
      {
        id: 'legend-dots-to-scale',
        criterion: 'Legend sample dots grow with their values, on the canvas scale',
        async run(page) {
          const d = await page.eval(() => window.__probe.legendDots());
          const ok_ = d.px.length >= 2 && d.distinct === d.px.length && d.ascending;
          return ok(ok_, d, ok_ ? '' : `dot diameters ${JSON.stringify(d.px)} — clamped or flat`);
        },
      },
      {
        id: 'collision-tracks-size',
        criterion: 'Force collision radius is not a hardcoded constant',
        async run() {
          const { readFileSync } = await import('node:fs');
          const src = readFileSync(new URL('../src/components/GraphView.svelte', import.meta.url), 'utf8');
          return ok(!/forceCollide\(\)\.radius\(\s*\d+\s*\)/.test(src), null, 'forceCollide must derive from node size');
        },
      },
    ],
  },

  'ui-015': {
    ticket: 'UI-015', title: 'Edge-label noise',
    async setup(page) {
      await page.viewport(1600, 1000);
      await page.goto(APP); await ready(page);
      await page.eval((n) => window.__probe.pickScope(n), 'ui');
      await sleep(9000);
    },
    checks: [
      {
        id: 'labels-off-by-default',
        criterion: 'Link labels are off on first load',
        async run(page) {
          const l = await page.eval(() => window.__probe.linkLabels());
          return ok(l.visible === 0, l);
        },
      },
      {
        id: 'dominant-type-suppressed',
        criterion: 'With labels on, far fewer labels than edges',
        async run(page) {
          // Drive the toggle to ON and verify via aria-pressed, not via the
          // label count. A uniform graph legitimately shows zero labels once
          // the dominant kind is suppressed, so "no labels" cannot be used to
          // infer "the toggle didn't work".
          const on = async () => (await page.eval(() =>
            window.__probe.toggleState(['Link Labels'])))[0];
          let btn = await on();
          if (!btn?.found) return ok(false, btn, 'Link Labels button not found');
          if (!btn.pressed) {
            await page.eval(() => window.__probe.clickText('Link Labels'));
            await sleep(2500);
            btn = await on();
          }
          if (!btn.pressed) return ok(false, btn, 'could not turn link labels on');
          const l = await page.eval(() => window.__probe.linkLabels());
          const ratio = l.edges ? l.visible / l.edges : 1;
          return ok(ratio <= 0.35, { ...l, ratio: +ratio.toFixed(2) }, 'expect <= 35% of edges labelled');
        },
      },
    ],
  },

  'ui-016': {
    ticket: 'UI-016', title: 'Persistent entity search',
    async setup(page) {
      await page.viewport(1600, 1000);
      await page.goto(APP); await ready(page);
    },
    checks: [
      {
        id: 'search-visible-1600',
        criterion: 'Search field visible without scrolling at 1600x1000',
        async run(page) {
          const b = await page.eval(() => window.__probe.box('search-input'));
          return ok(!!b && b.h > 0 && b.y >= 0 && b.bottom <= 1000, b);
        },
      },
      {
        id: 'search-visible-1280',
        criterion: 'Search field visible without scrolling at 1280x800',
        async run(page) {
          await page.viewport(1280, 800); await sleep(1200);
          const b = await page.eval(() => window.__probe.box('search-input'));
          await page.viewport(1600, 1000);
          return ok(!!b && b.h > 0 && b.y >= 0 && b.bottom <= 800, b);
        },
      },
      {
        id: 'cmd-k-focuses',
        criterion: 'Cmd/Ctrl-K focuses the search field',
        async run(page) {
          await page.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'k', code: 'KeyK', windowsVirtualKeyCode: 75, modifiers: 4 });
          await page.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'k', code: 'KeyK', windowsVirtualKeyCode: 75, modifiers: 4 });
          await sleep(600);
          const focused = await page.eval(() => {
            const a = document.activeElement;
            return !!a && (a.matches('[data-probe="search-input"]') || a.closest('[data-probe="search-input"]') !== null);
          });
          return ok(focused, { focused });
        },
      },
    ],
  },

  'ui-017': {
    ticket: 'UI-017', title: 'Actionable empty state',
    async setup(page) {
      await page.viewport(1600, 1000);
      await page.goto(APP); await ready(page);
    },
    checks: [
      {
        id: 'empty-state-names-a-folder',
        criterion: 'Empty-state card names a concrete folder from the index',
        async run(page) {
          const card = (await page.eval(() => window.__probe.text('.overlay-card'))).join(' ');
          const roots = (await page.eval(() => window.__probe.scopeRoots())).names;
          const named = roots.some((r) => r && card.includes(r));
          return ok(named, { card: card.slice(0, 200), roots });
        },
      },
    ],
  },

  'ui-021': {
    ticket: 'UI-021', title: 'Label toggles show their state',
    async setup(page) {
      await page.viewport(1600, 1000);
      await page.goto(APP); await ready(page);
      await page.eval((n) => window.__probe.pickScope(n), 'ui');
      await sleep(9000);
    },
    checks: [
      {
        id: 'toggles-show-state',
        criterion: 'Node/Kind/Link label toggles each indicate on vs off',
        async run(page) {
          // Assert on the TRANSITION. A check that merely looked for `.active`
          // somewhere in the toolbar would pass on Auto-Fit and the
          // Entity/File/Module group, which already have it.
          const results = [];
          for (const label of ['Node Labels', 'Kind Labels', 'Link Labels']) {
            const before = (await page.eval((l) => window.__probe.toggleState([l]), label))[0];
            await page.eval((l) => window.__probe.clickText(l), label);
            await sleep(600);
            const after = (await page.eval((l) => window.__probe.toggleState([l]), label))[0];
            const changed = before?.found && after
              && after.exposesState && before.pressed !== after.pressed;
            results.push({ label, changed: !!changed, before, after });
            await page.eval((l) => window.__probe.clickText(l), label); // restore
            await sleep(400);
          }
          const bad = results.filter((r) => !r.changed).map((r) => r.label);
          return ok(bad.length === 0, results, bad.length ? `no state change on: ${bad.join(', ')}` : '');
        },
      },
      {
        id: 'off-state-is-honest',
        criterion: 'A toggle reading off means nothing is drawn',
        async run(page) {
          // Only the OFF direction can be asserted against the canvas. Since
          // UI-015, labels-on legitimately renders zero labels when the
          // dominant relationship kind is suppressed, so "reads on" does not
          // imply "labels visible" — treating it as a biconditional made this
          // check fail on correct behaviour in both features at once.
          // Reload first: the sibling check toggles all three controls, and
          // inheriting its end state is how this check kept failing on
          // something other than what it measures.
          await page.goto(APP); await ready(page, 5000);
          await page.eval((n) => window.__probe.pickScope(n), 'ui');
          await sleep(9000);
          const btn = (await page.eval(() => window.__probe.toggleState(['Link Labels'])))[0];
          if (!btn?.found) return ok(false, btn, 'Link Labels button not found');
          const off = !btn.pressed;
          const shown = await page.eval(() => window.__probe.linkLabels());
          return ok(off && shown.visible === 0,
            { buttonReadsOff: off, visibleLabels: shown.visible });
        },
      },
    ],
  },

  'ui-022': {
    ticket: 'UI-022', title: 'Auto-fit default and label extents',
    async setup(page) {
      await page.viewport(1600, 1000);
      await page.goto(APP);
      await page.eval(() => { try { localStorage.removeItem('nao-auto-fit'); } catch { /* ignore */ } });
      await page.goto(APP); await ready(page);
    },
    checks: [
      {
        id: 'autofit-default-on',
        criterion: 'Auto-fit defaults on with no stored preference',
        async run(page) {
          const on = await page.eval(() => {
            const b = [...document.querySelectorAll('.control-btn, [data-probe="toolbar"] button')]
              .find((x) => /auto[- ]?fit/i.test(x.textContent));
            return b ? b.classList.contains('active') || b.getAttribute('aria-pressed') === 'true' : null;
          });
          return ok(on === true, { autoFitActive: on });
        },
      },
      {
        id: 'no-clipping-1600',
        criterion: 'No node or label clipped after first render at 1600x1000',
        async run(page) {
          await page.eval((n) => window.__probe.pickScope(n), 'ui');
          await sleep(10000);
          const clipped = await page.eval(() => window.__probe.clippedNodes());
          return ok(clipped.length === 0, clipped, clipped.length ? `${clipped.length} clipped` : '');
        },
      },
      {
        id: 'no-clipping-1280',
        criterion: 'No node or label clipped at 1280x800',
        async run(page) {
          await page.viewport(1280, 800); await sleep(4000);
          const clipped = await page.eval(() => window.__probe.clippedNodes());
          await page.viewport(1600, 1000);
          return ok(clipped.length === 0, clipped, clipped.length ? `${clipped.length} clipped` : '');
        },
      },
    ],
  },

  'ui-018': {
    ticket: 'UI-018', title: 'Quality panel legibility',
    async setup(page) {
      await page.viewport(1600, 1000);
      await page.goto(APP); await ready(page);
      await page.eval((n) => window.__probe.pickScope(n), 'ui');
      await sleep(9000);
      await page.eval(() => window.__probe.pickTab('Quality'));
      await sleep(4000);
    },
    checks: [
      {
        id: 'score-states-scale',
        criterion: 'Repo score states its range and direction',
        async run(page) {
          const txt = (await page.eval(() => window.__probe.text('.sidebar-top'))).join(' ');
          const scale = /\/\s*1(\.0)?\b|out of|0\s*[-–]\s*1/i.test(txt);
          const dir = /(lower|higher)\s+is\s+better/i.test(txt);
          return ok(scale && dir, { scale, dir });
        },
      },
      {
        id: 'thresholds-shown',
        criterion: 'OK/WARN/BAD thresholds visible per metric, in that metric\'s units',
        async run(page) {
          // Requires an explicit hook: a loose regex over the whole panel
          // matches unrelated numerals (tri-state legends, score cutoffs) and
          // passes for the wrong reason.
          const r = await page.eval(() => {
            const rows = [...document.querySelectorAll('[data-probe="metric-threshold"]')];
            const summaryRows = document.querySelectorAll('[data-probe="quality-summary"] tbody tr').length;
            return {
              annotated: rows.length,
              summaryRows,
              sample: rows.slice(0, 3).map((e) => e.textContent.replace(/\s+/g, ' ').trim()),
            };
          });
          return ok(r.annotated > 0 && (r.summaryRows === 0 || r.annotated >= r.summaryRows), r,
            'mark each summary row with data-probe="metric-threshold"');
        },
      },
      {
        id: 'population-named',
        criterion: 'Panel names the population its summary covers, next to the count',
        async run(page) {
          const r = await page.eval(() => {
            const e = document.querySelector('[data-probe="quality-population"]');
            if (!e) return { found: false };
            const txt = e.textContent.replace(/\s+/g, ' ').trim();
            return {
              found: true, txt,
              hasCount: /\d/.test(txt),
              hasPopulation: /(analysed|analyzed|indexed|in scope|shown|whole repo)/i.test(txt),
            };
          });
          return ok(!!r.found && r.hasCount && r.hasPopulation, r,
            'expose the summary population via data-probe="quality-population"');
        },
      },
      {
        id: 'cycles-actionable',
        criterion: 'Dependency-cycle count is clickable',
        async run(page) {
          const r = await page.eval(() => {
            // Match on the phrase across the whole subtree, not on a leaf:
            // once the banner gains a count and a call-to-action, no single
            // leaf carries the sentence any more.
            const banner = [...document.querySelectorAll('*')].find(
              (e) => /participate in dependency cycles/i.test(e.textContent)
                && ![...e.children].some((c) => /participate in dependency cycles/i.test(c.textContent)),
            );
            if (!banner) return { present: false };
            const el = document.querySelector('[data-probe="cycle-action"]') ?? banner;
            return {
              present: true,
              isButton: el.tagName === 'BUTTON' || el.getAttribute('role') === 'button',
              hasHook: !!document.querySelector('[data-probe="cycle-action"]'),
            };
          });
          if (!r.present) {
            // No cycles in this scope means nothing to make actionable — not
            // a pass. Say so rather than reporting a green tick.
            return ok(false, r, 'no cycle banner rendered — scope this suite at a folder that has cycles');
          }
          return ok(!!r.isButton && !!r.hasHook, r);
        },
      },
    ],
  },

  'ui-019': {
    ticket: 'UI-019', title: 'Quality scatter charts',
    async setup(page) {
      await page.viewport(1600, 1000);
      await page.goto(APP); await ready(page);
      await page.eval((n) => window.__probe.pickScope(n), 'ui');
      await sleep(9000);
      await page.eval(() => window.__probe.pickTab('Quality'));
      await sleep(4000);
    },
    checks: [
      {
        id: 'charts-have-axes-or-are-gone',
        criterion: 'Charts have axes and a legend, or have been removed (option b)',
        async run(page) {
          const r = await page.eval(() => {
            // svg.scatter is the actual chart element (QualityReport.svelte).
            const charts = [...document.querySelectorAll('svg.scatter, [data-probe="quality-chart"]')];
            if (!charts.length) return { removed: true };
            const hasAxes = (s) => s.querySelectorAll('.axis, .tick, [data-probe="axis"]').length > 0;
            const hasLegend = (s) =>
              !!s.closest('section')?.querySelector('.legend, [data-probe="chart-legend"]');
            return {
              removed: false,
              charts: charts.length,
              withAxes: charts.filter(hasAxes).length,
              withLegend: charts.filter(hasLegend).length,
            };
          });
          if (r.removed) {
            return ok(true, r, 'charts removed — option (b); verify the outlier list replaced them');
          }
          return ok(r.withAxes === r.charts && r.withLegend === r.charts, r);
        },
      },
      {
        id: 'ticks-present',
        criterion: 'Axes carry tick marks with values, not just a max-value annotation',
        async run(page) {
          const r = await page.eval(() => {
            const charts = [...document.querySelectorAll('svg.scatter, [data-probe="quality-chart"]')];
            if (!charts.length) return { removed: true };
            return {
              removed: false,
              charts: charts.length,
              withTicks: charts.filter((s) => s.querySelectorAll('.tick, [data-probe="tick"]').length >= 2).length,
            };
          });
          if (r.removed) return ok(true, r, 'charts removed — option (b)');
          return ok(r.withTicks === r.charts, r);
        },
      },
      {
        id: 'empty-state',
        criterion: 'With no data the chart states it is empty rather than rendering a blank box',
        async run(page) {
          await page.goto(APP); await ready(page);
          await page.eval(() => window.__probe.pickTab('Quality'));
          await sleep(3000);
          const r = await page.eval(() => {
            const charts = [...document.querySelectorAll('svg.scatter, [data-probe="quality-chart"]')];
            if (!charts.length) return { removed: true };
            const empty = charts.filter((s) => s.querySelectorAll('circle').length === 0);
            return {
              removed: false,
              empty: empty.length,
              stated: empty.filter((s) =>
                /no data|nothing|empty|select/i.test(s.textContent) ||
                s.closest('section')?.querySelector('[data-probe="chart-empty"]')).length,
            };
          });
          if (r.removed) return ok(true, r, 'charts removed — option (b)');
          return ok(r.empty === 0 || r.stated === r.empty, r);
        },
      },
      {
        id: 'interactivity-preserved',
        criterion: 'REGRESSION GUARD: points keep their entity tooltip and click-to-select',
        async run(page) {
          const r = await page.eval(() => {
            const charts = [...document.querySelectorAll('svg.scatter, [data-probe="quality-chart"]')];
            if (!charts.length) return { removed: true };
            const pts = charts.flatMap((s) => [...s.querySelectorAll('circle')]);
            return {
              removed: false,
              total: pts.length,
              identified: pts.filter((p) => p.querySelector('title') || p.hasAttribute('data-entity')).length,
            };
          });
          if (r.removed) return ok(true, r, 'charts removed — outlier list must be clickable instead');
          return ok(r.total > 0 && r.identified === r.total, r,
            'these already work today — do not lose them');
        },
      },
    ],
  },

  'ui-023': {
    ticket: 'UI-023', title: 'Component light-theme contrast',
    async setup(page) {
      await page.viewport(1600, 1000);
      await page.goto(APP); await ready(page);
    },
    checks: [
      {
        id: 'sidebar-contrast-all-themes',
        criterion: 'No sidebar/overlay text below 4.5:1 (3:1 for large) in any theme',
        async run(page) {
          const out = {};
          const seenBg = new Set();
          const themes = ['midnight', 'obsidian', 'nord', 'light', 'llvator'];
          for (const theme of themes) {
            await page.eval((id) => window.__probe.setTheme(id), theme);
            await page.goto(APP); await ready(page, 5000);
            await page.eval((n) => window.__probe.pickScope(n), 'ui');
            await sleep(9000);
            await page.eval(() => window.__probe.selectNode('graph.ts'));
            await sleep(1500);
            const applied = await page.eval(() => window.__probe.activeTheme());
            seenBg.add(applied.bgBody);
            out[theme] = await page.eval(() => window.__probe.contrastAudit());
          }
          // Same guard as ui-009: without it the loop can measure one theme
          // four times and report a clean sweep.
          if (seenBg.size < themes.length) {
            return ok(false, out, `only ${seenBg.size} of ${themes.length} distinct backgrounds — theme switching did not take`);
          }
          // Asserts on component literals only. Theme tokens failing AA is a
          // palette decision with app-wide visual impact and is tracked
          // separately (UI-024) — reported here, not failed here.
          const hard = Object.values(out).reduce((n, r) => n + r.hardcodedFailed, 0);
          const tok = Object.values(out).reduce((n, r) => n + r.tokenFailed, 0);
          const summary = Object.fromEntries(Object.entries(out).map(
            ([k, r]) => [k, { hardcoded: r.hardcodedFailed, tokens: r.tokenFailed, byToken: r.byToken }]));
          return ok(hard === 0, { summary, examples: out.light.hardcoded },
            hard ? `${hard} component-literal pairs below the bar (${tok} token pairs deferred to UI-024)`
                 : `${tok} token pairs remain, deferred to UI-024`);
        },
      },
      {
        id: 'quality-tab-contrast',
        criterion: 'Quality tab text passes contrast in the light theme',
        async run(page) {
          await page.eval((id) => window.__probe.setTheme(id), 'light');
          await page.goto(APP); await ready(page, 5000);
          await page.eval((n) => window.__probe.pickScope(n), 'ui');
          await sleep(9000);
          await page.eval(() => window.__probe.pickTab('Quality'));
          await sleep(4000);
          const r = await page.eval(() => window.__probe.contrastAudit());
          return ok(r.hardcodedFailed === 0, r);
        },
      },
    ],
  },

  'ui-020': {
    ticket: 'UI-020', title: 'Layout integrity at 1280x800',
    async setup(page) {
      await page.viewport(1280, 800);
      await page.goto(APP); await ready(page);
      await page.eval((n) => window.__probe.pickScope(n), 'ui');
      await sleep(10000);
    },
    checks: [
      {
        id: 'canvas-dominates',
        criterion: 'Canvas is wider than the sum of visible side panels',
        async run(page) {
          // All three columns count. Details is allowed to narrow itself to
          // its minimum at this width, and Description to drop out entirely,
          // precisely so this stays true (UI-040).
          const [c, l, d, r] = await Promise.all([
            page.eval(() => window.__probe.box('canvas')),
            page.eval(() => window.__probe.box('left-panel')),
            page.eval(() => window.__probe.box('details-panel')),
            page.eval(() => window.__probe.box('right-panel')),
          ]);
          const panels = (l?.w ?? 0) + (d?.w ?? 0) + (r?.w ?? 0);
          return ok(!!c && c.w > panels, { canvas: c && Math.round(c.w), panels: Math.round(panels) });
        },
      },
      {
        id: 'toolbar-two-rows',
        criterion: 'Toolbar at most two rows',
        async run(page) {
          const rows = await page.eval(() => window.__probe.rows('toolbar'));
          return ok(rows !== null && rows <= 2, rows);
        },
      },
      {
        id: 'scope-rows-reachable',
        criterion: 'At least five scope-tree rows reachable',
        async run(page) {
          const r = await page.eval(() => window.__probe.scopeRoots());
          return ok(r.visible >= 5, r);
        },
      },
      {
        id: 'no-bottom-overlap',
        criterion: 'Stats bar and mode bar never overlap',
        async run(page) {
          const results = {};
          for (const w of [1600, 1440, 1280, 1152, 1024]) {
            await page.viewport(w, 800); await sleep(900);
            results[w] = await page.eval(() => window.__probe.overlap('stats-bar', 'mode-bar'));
          }
          await page.viewport(1280, 800);
          const bad = Object.entries(results).filter(([, v]) => v);
          return ok(bad.length === 0, results, bad.length ? `overlap at ${bad.map((b) => b[0]).join(', ')}px` : '');
        },
      },
      {
        id: 'no-clipping',
        criterion: 'No node or label clipped after load',
        async run(page) {
          // Re-establish the viewport before measuring. `no-bottom-overlap`
          // above sweeps five widths and the app does not re-fit on resize
          // (that is this suite's `live-reflow` criterion), so inheriting its
          // final state made this check report the sibling's side effect
          // rather than the state after load.
          await page.viewport(1280, 800);
          await sleep(1000);
          await page.eval(() => window.__probe.clickText('Fit View'));
          await sleep(1500);
          const clipped = await page.eval(() => window.__probe.clippedNodes());
          return ok(clipped.length === 0, clipped);
        },
      },
      {
        id: 'live-reflow',
        criterion: 'Resizing 1600 -> 1280 reflows without clipping, no reload',
        async run(page) {
          await page.viewport(1600, 1000); await sleep(3000);
          await page.viewport(1280, 800); await sleep(3000);
          const clipped = await page.eval(() => window.__probe.clippedNodes());
          const overlap = await page.eval(() => window.__probe.overlap('stats-bar', 'mode-bar'));
          return ok(clipped.length === 0 && !overlap, { clipped: clipped.length, overlap });
        },
      },
    ],
  },

  'ui-036': {
    ticket: 'UI-036', title: 'Copy-refactor-prompt actions',
    /**
     * The prompt button is the first affordance in the UI whose success
     * depends on a *round-trip* — it fetches from the engine, then writes to
     * the clipboard. Both halves fail silently by nature (a dead fetch and a
     * blocked clipboard look identical to a user), so the button encodes its
     * own outcome in its glyph and these checks read that back.
     *
     * ⚠ Point `nao watch` at a FROZEN copy of the repo, not the working tree:
     *
     *     git archive HEAD | tar -x -C /tmp/snap && nao watch /tmp/snap -p 3000
     *
     * These checks span several seconds each. Watching a tree you are still
     * editing means re-analysis and live-reload rebuild the quality table
     * mid-check, and the failures that produces look exactly like real ones
     * ("button not found" after a click that worked).
     */
    async setup(page) {
      await page.viewport(1440, 900);
      await page.goto(APP); await ready(page);
      await page.eval(() => window.__probe.pickTab('Quality'));
      await sleep(2500);
    },
    checks: [
      {
        id: 'prompt-button-present',
        criterion: 'Every entity row offers a labelled copy-prompt button',
        async run(page) {
          // Scoped to rows that carry the per-entity copy cell. The Quality
          // tab renders three tables (summary, entities, files/modules) and
          // a bare `tbody tr` sweep counts all of them — only the entity
          // leaderboard takes an entity_id, which is what the prompt needs.
          const r = await page.eval(() => {
            const rows = [...document.querySelectorAll('tbody tr')]
              .filter((tr) => tr.querySelector('td.copy-cell'));
            const withBtn = rows.filter((tr) =>
              tr.querySelector('button[aria-label="Copy refactoring prompt"]'));
            return { entityRows: rows.length, withBtn: withBtn.length };
          });
          return ok(r.entityRows > 0 && r.withBtn === r.entityRows, r);
        },
      },
      {
        id: 'actions-are-labelled',
        criterion: 'Row actions read as words, and no glyph means two things',
        async run(page) {
          const r = await page.eval(() => {
            const row = [...document.querySelectorAll('tbody tr')]
              .find((tr) => tr.querySelector('td.copy-cell'));
            if (!row) return { found: false };
            const btns = [...row.querySelectorAll('td.copy-cell button')];
            return {
              found: true,
              labels: btns.map((b) => b.textContent.trim()),
              // A label of one character is a glyph wearing a label's clothes.
              glyphOnly: btns.filter((b) => b.textContent.trim().length <= 2)
                .map((b) => b.textContent.trim()),
            };
          });
          return ok(
            r.found && r.glyphOnly.length === 0 && r.labels.length >= 2
              && r.labels.every((l) => /[A-Za-z]/.test(l)),
            r,
          );
        },
      },
      {
        id: 'prompt-copy-round-trip',
        criterion: 'Clicking it fetches and copies — button reports success, not failure',
        async run(page) {
          const before = await page.eval(() => {
            const b = document.querySelector('button[aria-label="Copy refactoring prompt"]');
            if (!b) return null;
            b.click();
            return b.textContent.trim();
          });
          await sleep(2500);
          const after = await page.eval(() => {
            const b = document.querySelector('button[aria-label="Copy refactoring prompt"]');
            return b ? { glyph: b.textContent.trim(), failed: b.classList.contains('failed') } : null;
          });
          // '✗' means the round-trip ran and lost — a genuine failure, not a
          // missing button. Distinguish it in the note so a red run here is
          // actionable rather than ambiguous.
          const note = after?.failed ? 'fetch or clipboard write failed' : '';
          return ok(!!after && !after.failed && after.glyph !== '…', { before, after }, note);
        },
      },
      {
        id: 'context-scope-prompt-action',
        criterion: 'Context Scope offers the prompt as its primary action',
        async run(page) {
          // Select via a quality row rather than a graph node: `ContextScope`
          // only mounts once `selectedNode` is pinned, and a synthetic click
          // on `g.node` doesn't set it. Clicking the row is also the path a
          // user actually takes from "this entity looks bad" to its context.
          const picked = await page.eval(() => {
            const row = [...document.querySelectorAll('tbody tr')]
              .find((tr) => tr.querySelector('td.copy-cell'));
            if (!row) return false;
            row.click();
            return true;
          });
          await sleep(1500);
          await page.eval(() => window.__probe.clickText('Context Scope'));
          await sleep(3000);
          const r = await page.eval(() => {
            const panel = document.querySelector('.context-scope');
            const siblings = [...document.querySelectorAll('.scope-actions button')];
            const btn = siblings.find((b) => /Refactor Prompt/i.test(b.textContent));
            if (!btn) {
              // Report which stage broke: no panel at all, panel present but
              // collapsed, or expanded with the button missing.
              return {
                found: false,
                panelInDom: !!panel,
                expanded: siblings.length > 0,
                actionCount: siblings.length,
              };
            }
            return {
              found: true,
              first: siblings.indexOf(btn) === 0,
              primary: btn.classList.contains('copy-btn-primary'),
            };
          });
          return ok(picked && r.found && r.first && r.primary, r);
        },
      },
      {
        id: 'older-engine-hides-action',
        criterion: 'Engine without the export: action hidden, raw-context buttons still work',
        async run(page) {
          // The UI can be pointed at any engine (UI-032), so "older engine"
          // is a real deployment, not a hypothetical. Simulated by stripping
          // the field in flight rather than by keeping an old binary around.
          await page.eval(() => {
            window.__realFetch = window.fetch;
            window.fetch = async (...args) => {
              const resp = await window.__realFetch(...args);
              const url = typeof args[0] === 'string' ? args[0] : args[0]?.url ?? '';
              if (!url.includes('/api/scope')) return resp;
              const body = await resp.clone().json();
              delete body.exports.refactor_prompt;
              return new Response(JSON.stringify(body), {
                status: 200, headers: { 'Content-Type': 'application/json' },
              });
            };
            return true;
          });
          // Changing depth re-runs fetchScope through the patched fetch.
          await page.eval(() => {
            const btn = [...document.querySelectorAll('.depth-btn')].find((b) => b.textContent.trim() === '2');
            if (btn) btn.click();
            return !!btn;
          });
          await sleep(2500);
          const r = await page.eval(() => {
            const btns = [...document.querySelectorAll('.scope-actions button')];
            return {
              promptBtn: btns.filter((b) => /Refactor Prompt/i.test(b.textContent)).length,
              otherBtns: btns.length,
              labels: btns.map((b) => b.textContent.trim()),
            };
          });
          await page.eval(() => {
            if (window.__realFetch) window.fetch = window.__realFetch;
            return true;
          });
          return ok(r.promptBtn === 0 && r.otherBtns === 4, r);
        },
      },
    ],
  },

  'ui-034': {
    ticket: 'UI-034', title: 'Connect screen',
    /**
     * Unlike every other suite, this one navigates per check: each state it
     * asserts *is* a different page load, and a connect screen that only
     * appears on a cold boot cannot be driven into view from a booted app.
     *
     * `ENGINE` is a `nao watch` started without `--allow-origin`, which is
     * the default and is exactly the refusal being tested — pointing this
     * page at it must produce the actionable message rather than a blank
     * graph.
     */
    async setup(page) {
      await page.viewport(1280, 900);
      await page.goto(APP);
      await sleep(1500);
      // A stored endpoint from an earlier run would pre-empt every check.
      await page.eval(() => {
        window.localStorage.removeItem('nao.apiBase');
        window.localStorage.removeItem('nao.apiToken');
        return true;
      });
    },
    checks: [
      {
        id: 'served-page-skips-the-screen',
        criterion: 'An engine-served page never shows the connect screen',
        async run(page) {
          await page.goto(APP);
          await sleep(6000);
          const state = await page.eval(() => ({
            screen: !!document.querySelector('[data-probe="connect-screen"]'),
            app: !!document.querySelector('.app-root'),
          }));
          return ok(!state.screen && state.app, state,
            state.screen ? 'connect screen shown where the endpoint was already known' : '');
        },
      },
      {
        id: 'refused-origin-names-the-flag',
        criterion: 'A refused origin is named as such and prints --allow-origin',
        async run(page) {
          await page.goto(`${APP}?api=${encodeURIComponent(ENGINE)}`);
          await sleep(6000);
          const state = await page.eval(() => {
            const err = document.querySelector('[data-probe="connect-error"]');
            return {
              kind: err?.getAttribute('data-connect-error') ?? null,
              remedy: document.querySelector('[data-probe="connect-screen"] pre')?.textContent ?? '',
              origin: window.location.origin,
            };
          });
          const namesFlag = state.remedy.includes('--allow-origin')
            && state.remedy.includes(state.origin);
          return ok(state.kind === 'refused' && namesFlag, state,
            state.kind !== 'refused'
              ? `diagnosed as "${state.kind}" — a reachable nao that blocked us must read as refused`
              : 'the remedy must be a copy-pasteable command naming this origin');
        },
      },
      {
        id: 'dead-port-is-not-a-refusal',
        criterion: 'Nothing listening reports unreachable, not refused',
        async run(page) {
          await page.goto(`${APP}?api=${encodeURIComponent('http://localhost:59997')}`);
          await sleep(6000);
          const kind = await page.eval(() =>
            document.querySelector('[data-probe="connect-error"]')?.getAttribute('data-connect-error') ?? null);
          return ok(kind === 'unreachable', { kind },
            'the three failures need three messages — conflating them is the bug');
        },
      },
      {
        id: 'port-alone-is-accepted',
        criterion: 'Typing a bare port resolves to http://localhost:<port>',
        async run(page) {
          await page.goto(`${APP}?api=${encodeURIComponent('http://localhost:59997')}`);
          await sleep(6000);
          const hint = await page.eval(() => {
            const input = document.querySelector('[data-probe="connect-input"]');
            if (!input) return null;
            input.value = '3200';
            input.dispatchEvent(new Event('input', { bubbles: true }));
            return true;
          });
          if (!hint) return ok(false, null, 'connect input not found');
          await sleep(400);
          const shown = await page.eval(() =>
            document.querySelector('[data-probe="connect-screen"] .hint')?.textContent ?? '');
          return ok(shown.includes('http://localhost:3200'), shown.trim(),
            'the port form is the common case and must be visibly resolved before connecting');
        },
      },
    ],
  },

  'ui-043': {
    ticket: 'UI-042/043/044', title: 'Scope query: whole-index search, ranking, commit',
    async setup(page) {
      await page.viewport(1600, 1000);
      await page.goto(APP); await ready(page);
    },
    checks: [
      {
        id: 'searches-collapsed-folders',
        criterion: 'A nested path is findable from a cold load, unexpanded',
        async run(page) {
          // The defect this exists for: the old walk recursed only into
          // `openFolders`, which starts as {''}, so from a fresh session the
          // box could only match top-level entries.
          const before = await page.eval(() => window.__probe.scopeRows().length);
          await page.eval(() => window.__probe.typeScopeQuery('displayPlan'));
          await sleep(400);
          const rows = await page.eval(() => window.__probe.scopeRows());
          await page.eval(() => window.__probe.typeScopeQuery(''));
          await sleep(300);
          const hit = rows.find((r) => /viewmodels\/displayPlan\.ts$/.test(r.path));
          return ok(!!hit, { topLevelRowsBefore: before, rows: rows.slice(0, 5) },
            hit ? '' : 'nested path not returned — search is gated on expansion state again');
        },
      },
      {
        id: 'query-leaves-expansion-alone',
        criterion: 'Clearing the query restores the tree exactly',
        async run(page) {
          await page.eval(() => window.__probe.expandScope('ui'));
          await sleep(300);
          const before = await page.eval(() => window.__probe.scopeRows().map((r) => r.path));
          await page.eval(() => window.__probe.typeScopeQuery('scopeTree'));
          await sleep(400);
          const during = await page.eval(() => window.__probe.scopeRows().map((r) => r.path));
          await page.eval(() => window.__probe.typeScopeQuery(''));
          await sleep(400);
          const after = await page.eval(() => window.__probe.scopeRows().map((r) => r.path));
          const same = JSON.stringify(before) === JSON.stringify(after);
          return ok(same && during.length > 0,
            { before: before.length, during: during.length, after: after.length },
            same ? '' : 'tree did not come back the way it was left');
        },
      },
      {
        id: 'paths-outrank-entities',
        criterion: 'A path match beats a same-named entity for a scope query',
        async run(page) {
          // Undiscounted, a short entity name outscores every path, and
          // `graph` returned an entity called `graph` ahead of graph.ts.
          await page.eval(() => window.__probe.typeScopeQuery('graph'));
          await sleep(500);
          const rows = await page.eval(() => window.__probe.scopeRows());
          await page.eval(() => window.__probe.typeScopeQuery(''));
          await sleep(300);
          const first = rows[0];
          return ok(!!first && !first.entity, { first, next: rows.slice(1, 4) },
            first?.entity ? `entity "${first.entity}" ranked above every path` : '');
        },
      },
      {
        id: 'operators',
        criterion: 'Negation, anchors and literals behave',
        async run(page) {
          const probe = async (q) => {
            await page.eval((t) => window.__probe.typeScopeQuery(t), q);
            await sleep(450);
            return page.eval(() => window.__probe.scopeRows().map((r) => r.path));
          };
          const anchored = await probe('^src .rs$');
          const negated = await probe('^src !parser');
          await page.eval(() => window.__probe.typeScopeQuery(''));
          await sleep(300);
          const anchorOk = anchored.length > 0
            && anchored.every((p) => p.startsWith('src') && p.endsWith('.rs'));
          const negateOk = negated.length > 0 && !negated.some((p) => p.includes('parser'));
          return ok(anchorOk && negateOk,
            { anchored: anchored.length, negated: negated.length,
              anchorLeaks: anchored.filter((p) => !p.startsWith('src') || !p.endsWith('.rs')).slice(0, 3),
              negateLeaks: negated.filter((p) => p.includes('parser')).slice(0, 3) },
            anchorOk ? (negateOk ? '' : 'negation let a match through') : 'anchors did not constrain');
        },
      },
      {
        id: 'enter-commits-what-it-projected',
        criterion: 'The pre-commit projection equals the resulting scope',
        async run(page) {
          await page.eval(() => window.__probe.typeScopeQuery('^ui/src/viewmodels'));
          await sleep(500);
          const status = await page.eval(() => window.__probe.scopeStatus());
          await page.eval(() => window.__probe.scopeKey('Enter'));
          await sleep(2500);
          const after = await page.eval(() => window.__probe.scopeEntities());
          await page.eval(() => window.__probe.scopeKey('Escape'));
          await sleep(400);
          const projected = Number(String(status.projection ?? '').match(/([\d.]+)k?\s*entities/)?.[1] ?? NaN);
          // The projection formats with `formatCount`, so compare loosely
          // above 1000 and exactly below it.
          const match = projected >= 1000 ? Math.abs(projected - after) / after < 0.05 : projected === after;
          return ok(status.matches > 0 && match, { projection: status.projection, inScope: after },
            match ? '' : 'projection and committed scope disagree');
        },
      },
      {
        id: 'empty-match-does-not-wipe-scope',
        criterion: 'Committing a query that matches nothing is a no-op',
        async run(page) {
          const before = await page.eval(() => window.__probe.scopeEntities());
          await page.eval(() => window.__probe.typeScopeQuery('zzz-no-such-path'));
          await sleep(400);
          const status = await page.eval(() => window.__probe.scopeStatus());
          await page.eval(() => window.__probe.scopeKey('Enter'));
          await sleep(1200);
          const after = await page.eval(() => window.__probe.scopeEntities());
          await page.eval(() => window.__probe.scopeKey('Escape'));
          await sleep(300);
          return ok(before > 0 && after === before, { before, after, matches: status.matches },
            after === before ? '' : 'a typo changed the scope');
        },
      },
    ],
  },

  'ui-046': {
    ticket: 'UI-046/047', title: 'File tree renders fully and remembers exclusions',
    async setup(page) {
      await page.viewport(1600, 1000);
      await page.goto(APP); await ready(page);
      await page.eval((n) => window.__probe.pickScope(n), 'ui');
      await sleep(6000);
      // Pin File level. `ui` is well past RENDER_BUDGET, so auto-level lands
      // on Module — where the collapsed nodes carry a *directory* path and
      // there are no per-file rows to assert on at all. That is correct
      // behaviour (nothing to hide at module granularity), but it makes the
      // panel untestable, and clicking a level button also switches
      // auto-level off so the suite can't be re-escalated underneath it.
      await page.eval(() => window.__probe.setLevel('File'));
      await sleep(3000);
      await page.eval(() => window.__probe.openFilesPanel());
      await sleep(600);
    },
    checks: [
      {
        id: 'deep-files-reachable',
        criterion: 'Files more than one folder deep render and can be toggled',
        async run(page) {
          // The old template unrolled two levels by hand, so anything under
          // `ui/src/<dir>/` was absent from the panel entirely.
          await expandFileTree(page, 'ui/src/viewmodels');
          const rows = await page.eval(() => window.__probe.fileRows());
          const deep = rows.filter((r) => r.depth >= 3);
          return ok(deep.length > 0, { rows: rows.length, maxDepth: Math.max(0, ...rows.map((r) => r.depth)), deep: deep.length },
            deep.length ? '' : 'no row deeper than two levels — the recursion is gone again');
        },
      },
      {
        id: 'folder-checkbox-tracks-its-subtree',
        criterion: 'Hiding one file puts its folders into the indeterminate state',
        async run(page) {
          const target = 'ui/src/viewmodels/displayPlan.ts';
          await expandFileTree(page, 'ui/src/viewmodels');
          const before = await page.eval(() => window.__probe.fileRows());
          const hidden = await page.eval((p) => window.__probe.setFileVisible(p, false), target);
          await sleep(900);
          const after = await page.eval(() => window.__probe.fileRows());
          const state = (rows, p) => rows.find((r) => r.path === p)?.state;
          const pass = hidden
            && state(before, 'ui/src/viewmodels') === 'all'
            && state(after, target) === 'none'
            && state(after, 'ui/src/viewmodels') === 'some';
          // leave it hidden — the next check needs it
          return ok(pass, {
            folderBefore: state(before, 'ui/src/viewmodels'),
            fileAfter: state(after, target),
            folderAfter: state(after, 'ui/src/viewmodels'),
          }, pass ? '' : 'folder box does not reflect its files');
        },
      },
      {
        id: 'exclusion-survives-level-toggle',
        criterion: 'A hidden file stays hidden across an aggregation change',
        async run(page) {
          // `seedFromGraph` used to reset the file filter on every publish,
          // and auto-level publishes on its own.
          const target = 'ui/src/viewmodels/displayPlan.ts';
          const stateAfterLevel = async (level) => {
            await page.eval((l) => window.__probe.setLevel(l), level);
            await sleep(2000);
            await expandFileTree(page, 'ui/src/viewmodels');
            return page.eval((p) => {
              const bar = document.querySelector('[data-probe="stats-bar"], .stats')?.textContent ?? '';
              return {
                state: window.__probe.fileRows().find((r) => r.path === p)?.state ?? 'missing',
                shown: Number((bar.match(/Shown:\s*([\d,]+)/i)?.[1] ?? '0').replace(/,/g, '')),
              };
            }, target);
          };
          const atEntity = await stateAfterLevel('Entity');
          const atFile = await stateAfterLevel('File');
          const pass = atEntity.state === 'none' && atFile.state === 'none';
          return ok(pass, { atEntity, atFile },
            pass ? '' : 'the exclusion was reset by a level change');
        },
      },
      {
        id: 'filter-keeps-ancestors',
        criterion: 'Filtering by filename shows the file, not just its folder',
        async run(page) {
          // The old rule rendered a folder only when the folder's own path
          // matched, so typing a filename hid the folder and the file too.
          await page.eval(() => window.__probe.typeFileQuery('displayPlan'));
          await sleep(600);
          const rows = await page.eval(() => window.__probe.fileRows());
          await page.eval(() => window.__probe.typeFileQuery(''));
          await sleep(400);
          const file = rows.find((r) => /displayPlan\.ts$/.test(r.path));
          const ancestors = rows.filter((r) => 'ui/src/viewmodels'.startsWith(r.path));
          return ok(!!file && ancestors.length >= 2, { rows: rows.map((r) => r.path) },
            file ? '' : 'the matching file was hidden with its folder');
        },
      },
    ],
  },

  'ui-045': {
    ticket: 'UI-045', title: 'Scope exclusion is a rule, not thirty materialised paths',
    async setup(page) {
      await page.viewport(1600, 1000);
      await page.goto(APP); await ready(page);
      await page.eval(() => window.__probe.clickText('Select All'));
      await sleep(6000);
      for (const p of ['ui', 'ui/src']) {
        await page.eval((x) => window.__probe.expandScope(x), p);
        await sleep(400);
      }
    },
    checks: [
      {
        id: 'excluding-a-folder-subtracts-exactly-it',
        criterion: 'Unchecking an inherited folder removes only that folder',
        async run(page) {
          const before = await page.eval(() => window.__probe.scopeEntities());
          const folder = await page.eval(() => window.__probe.scopeRowState('ui/src/viewmodels'));
          if (!folder) return ok(false, { folder }, 'ui/src/viewmodels row not found');
          await page.eval(() => window.__probe.toggleScopePath('ui/src/viewmodels'));
          await sleep(2500);
          const after = await page.eval(() => window.__probe.scopeEntities());
          const sibling = await page.eval(() => window.__probe.scopeRowState('ui/src/stores'));
          const parent = await page.eval(() => window.__probe.scopeRowState('ui'));
          const self = await page.eval(() => window.__probe.scopeRowState('ui/src/viewmodels'));
          const pass = before - after === folder.count
            && self?.checked === false && sibling?.checked === true && parent?.checked === true;
          return ok(pass, { before, after, delta: before - after, folderCount: folder.count, self, sibling, parent },
            pass ? '' : 'exclusion did not subtract exactly the folder, or took siblings with it');
        },
      },
      {
        id: 'exclusion-is-not-materialised',
        criterion: 'Excluding does not enumerate the siblings it left in scope',
        async run(page) {
          // The old model deleted the covering ancestor and re-added every
          // sibling at every level, so one click produced ~30 directly-named
          // entries. A rule list names nothing extra.
          const rows = await page.eval(() => window.__probe.scopeRows());
          const direct = rows.filter((r) => r.direct);
          return ok(direct.length <= 1, { visibleRows: rows.length, direct: direct.map((r) => r.path) },
            direct.length <= 1 ? '' : `${direct.length} rows named directly — exclusion was materialised`);
        },
      },
      {
        id: 'exclusion-is-reversible',
        criterion: 'Re-checking the folder restores the original scope',
        async run(page) {
          const before = await page.eval(() => window.__probe.scopeEntities());
          await page.eval(() => window.__probe.toggleScopePath('ui/src/viewmodels'));
          await sleep(2500);
          const after = await page.eval(() => window.__probe.scopeEntities());
          const self = await page.eval(() => window.__probe.scopeRowState('ui/src/viewmodels'));
          return ok(after > before && self?.checked === true, { before, after, self },
            after > before ? '' : 'toggling back did not restore the folder');
        },
      },
    ],
  },
};

// ── CLI ──────────────────────────────────────────────────────────────────

const argv = process.argv.slice(2);
const keepOpen = argv.includes('--keep-open');
const wanted = argv.includes('--all')
  ? Object.keys(SUITES)
  : argv.filter((a) => !a.startsWith('--')).map((a) => a.toLowerCase());

if (!wanted.length) {
  console.log(`ux-probe — layout assertions for the nao web UI

usage: node ui/scripts/ux-probe.mjs <suite...> | --all [--keep-open]

suites:
${Object.entries(SUITES).map(([k, s]) => `  ${k.padEnd(8)} ${s.ticket} — ${s.title}`).join('\n')}

requires: nao watch . --port 3000   and   cd ui && npm run dev -- --port 5199
env:      PROBE_URL (default ${APP}), PROBE_CDP_PORT (default ${CDP_PORT}),
          PROBE_ENGINE (default ${ENGINE}) — the engine's own origin, used by
          ui-034 to provoke a cross-origin refusal`);
  process.exit(0);
}

const unknown = wanted.filter((w) => !SUITES[w]);
if (unknown.length) {
  console.error(`unknown suite(s): ${unknown.join(', ')}`);
  process.exit(1);
}

try {
  await fetch(APP);
} catch {
  console.error(`Cannot reach ${APP} — start the Vite dev server and \`nao watch\` first.`);
  process.exit(1);
}

await ensureChrome();

let failed = 0, passed = 0;
const G = '\x1b[32m', R = '\x1b[31m', D = '\x1b[2m', Z = '\x1b[0m';

for (const key of wanted) {
  const suite = SUITES[key];
  console.log(`\n${suite.ticket} — ${suite.title}`);
  const page = await Page.open('about:blank');
  try {
    await suite.setup(page);
    for (const check of suite.checks) {
      let res;
      try {
        res = await check.run(page);
      } catch (err) {
        res = { pass: false, actual: null, note: `probe threw: ${err.message}` };
      }
      res.pass ? passed++ : failed++;
      const mark = res.pass ? `${G}PASS${Z}` : `${R}FAIL${Z}`;
      console.log(`  ${mark}  ${check.criterion}`);
      if (!res.pass) {
        if (res.actual !== null && res.actual !== undefined) {
          console.log(`        ${D}actual: ${JSON.stringify(res.actual)}${Z}`);
        }
        if (res.note) console.log(`        ${D}${res.note}${Z}`);
      }
    }
  } catch (err) {
    failed++;
    console.log(`  ${R}FAIL${Z}  suite setup: ${err.message}`);
  } finally {
    if (!keepOpen) await page.close();
  }
}

console.log(`\n${passed} passed, ${failed} failed`);
process.exit(failed);
