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
 *   stats-bar · canvas-bottom-bar · canvas-stats · mode-bar
 *   sidebar-top · scope-tree · quality-table
 *   pin-toggle · search-input · search-results · legend
 *   scope-query · file-filter · file-tree
 *   quality-summary · metric-threshold · quality-population
 *   quality-population-picker · visual-filter-state · filter-notice
 *   quality-chart · tick · chart-legend · chart-empty · chart-sampled
 *   overview-panel
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

  /**
   * One keystroke, delivered the way a keyboard delivers it.
   *
   * The keymap layer matches on `event.key`, but Chrome will not synthesise a
   * keydown it cannot place on a physical keyboard, so `code` and the virtual
   * key code have to be right too — a dispatch missing them arrives with an
   * empty `code` and the layer's own focus handling ignores it.
   */
  async key(k) {
    const NAMED = { '[': ['BracketLeft', 219], ']': ['BracketRight', 221], '/': ['Slash', 191] };
    const [code, vk] = NAMED[k]
      ?? (/^[0-9]$/.test(k) ? [`Digit${k}`, k.charCodeAt(0)] : [`Key${k.toUpperCase()}`, k.toUpperCase().charCodeAt(0)]);
    const base = { key: k, code, windowsVirtualKeyCode: vk, nativeVirtualKeyCode: vk };
    await this.send('Input.dispatchKeyEvent', { ...base, type: 'keyDown', text: k });
    await this.send('Input.dispatchKeyEvent', { ...base, type: 'keyUp' });
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

    /**
     * The overview panel, its dots, and the viewport box inside it (UI-083).
     *
     * Everything is reported in panel-relative coordinates, because every
     * claim worth making here is "the box is inside the panel" or "the box
     * shrank" — both of which are about the box's place in the panel and
     * neither of which cares where on screen the panel happens to be.
     */
    overview() {
      const panel = document.querySelector('[data-probe="overview-panel"]');
      if (!panel) return null;
      const map = panel.querySelector('svg.overview-map');
      const canvas = box('canvas');
      const p = panel.getBoundingClientRect();
      if (!map) return { open: false, insideCanvas: null, dots: 0, box: null };
      const m = map.getBoundingClientRect();
      const rect = map.querySelector('rect.viewport-box');
      const r = rect?.getBoundingClientRect();
      return {
        open: true,
        // The panel is an overlay on the canvas, not a column beside it.
        insideCanvas: canvas
          ? p.x >= canvas.x - 0.5 && p.y >= canvas.y - 0.5 &&
            p.right <= canvas.right + 0.5 && p.bottom <= canvas.bottom + 0.5
          : null,
        dots: map.querySelectorAll('circle').length,
        map: { w: +m.width.toFixed(1), h: +m.height.toFixed(1) },
        box: r ? {
          x: +(r.x - m.x).toFixed(1), y: +(r.y - m.y).toFixed(1),
          w: +r.width.toFixed(1), h: +r.height.toFixed(1),
          // The fraction of the panel the viewport covers — the number that
          // has to fall as the canvas zooms in.
          share: +((r.width * r.height) / (m.width * m.height)).toFixed(4),
        } : null,
      };
    },

    /** Press at a fraction of the overview panel, so the probe can assert
     *  that a click there steers the canvas. Pointer events rather than a
     *  CDP mouse click: the panel listens on `pointerdown` and captures. */
    clickOverview(fx, fy) {
      const map = document.querySelector('[data-probe="overview-panel"] svg.overview-map');
      if (!map) return null;
      const m = map.getBoundingClientRect();
      const clientX = m.x + m.width * fx;
      const clientY = m.y + m.height * fy;
      const opts = { clientX, clientY, bubbles: true, cancelable: true, pointerId: 1, button: 0, isPrimary: true };
      map.dispatchEvent(new PointerEvent('pointerdown', opts));
      map.dispatchEvent(new PointerEvent('pointerup', opts));
      return { clientX: +clientX.toFixed(1), clientY: +clientY.toFixed(1) };
    },

    /** The canvas's zoom transform, as the `k`/`x`/`y` d3 wrote onto the
     *  root group — the thing the overview's box is a reading of. */
    canvasTransform() {
      const g = document.querySelector('.graph-container svg > g');
      const t = g?.getAttribute('transform') ?? '';
      const tr = /translate\(([-\d.e]+)[, ]([-\d.e]+)\)/.exec(t);
      const sc = /scale\(([-\d.e]+)\)/.exec(t);
      return {
        raw: t,
        x: tr ? +(+tr[1]).toFixed(2) : 0,
        y: tr ? +(+tr[2]).toFixed(2) : 0,
        k: sc ? +(+sc[1]).toFixed(4) : 1,
      };
    },

    /** Toolbar zoom, by aria-label — `clickText('+')` would match half the
     *  buttons on the page. */
    zoomCanvas(dir = 'in') {
      const b = document.querySelector(`button[aria-label="Zoom ${dir}"]`);
      if (!b) return false;
      b.click();
      return true;
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

    /**
     * Every scope row with its *exact* entity count.
     *
     * `scopeRowState` reads the count's rendered text, which `formatCount`
     * abbreviates — "9.4k" parses to 9.4 and loses to a plain "691". The
     * title attribute carries the real number, which is what any comparison
     * between rows has to use.
     */
    scopeRowCounts() {
      return [...document.querySelectorAll('.scope-tree .tree-item')].map((r) => {
        const title = r.querySelector('.count')?.getAttribute('title') ?? '';
        return {
          path: r.querySelector('.label')?.getAttribute('title') ?? '',
          entities: Number((title.match(/^(\d+)\s+entities/) ?? [0, 0])[1]),
        };
      });
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

    // ── grouping (UI-052 / UI-053) ───────────────────────────────────────

    /** Live node positions read off the d3 data join. `__data__` is where d3
     *  parks the datum, so this reports where the simulation actually put
     *  each node rather than re-deriving it from the transform attribute. */
    /**
     * The group a node belongs to — `folderKeyOf` in
     * utils/forceCohesion.ts, mirrored.
     *
     * One definition here for the same reason there is one there. Three
     * probes ask this question, and one analyze run emits both
     * `./ui/src/stores` and `ui/src/stores` (AN-015), so a probe that
     * normalises differently from the app reports a folder as scattered at
     * exactly the moment the app has united it — a bug that exists only in
     * the measurement.
     */
    folderOfPath(p) {
      if (!p) return null;
      const i = p.lastIndexOf('/');
      const dir = i < 0 ? '' : p.slice(0, i);
      if (dir === '.') return '';
      return dir.startsWith('./') ? dir.slice(2) : dir;
    },

    nodePositions() {
      return [...document.querySelectorAll('g.node')]
        .filter((g) => g.style.display !== 'none')
        .map((g) => {
          const d = g.__data__ ?? {};
          return { id: d.id, folder: this.folderOfPath(d.file_path ?? ''), x: d.x, y: d.y };
        })
        .filter((n) => Number.isFinite(n.x) && Number.isFinite(n.y));
    },

    /** Mean distance between same-folder nodes, over mean distance between
     *  all pairs. 1.0 means folders are no more clustered than chance; lower
     *  is tighter. This is the one number that says whether grouping is
     *  visible, and it is invariant to zoom and to canvas size — which a raw
     *  distance is not. Ghosts are excluded: they have no folder. */
    cohesionRatio() {
      const ns = this.nodePositions().filter((n) => n.folder !== null);
      let intraSum = 0, intraN = 0, allSum = 0, allN = 0;
      for (let i = 0; i < ns.length; i++) {
        for (let j = i + 1; j < ns.length; j++) {
          const d = Math.hypot(ns[i].x - ns[j].x, ns[i].y - ns[j].y);
          allSum += d; allN++;
          if (ns[i].folder === ns[j].folder) { intraSum += d; intraN++; }
        }
      }
      // Report the counts even when the ratio is undefined. A bare null here
      // says "no pairs" but not whether that is because nothing rendered,
      // because every node is in its own folder, or because the datum is
      // missing — three very different bugs.
      const rendered = document.querySelectorAll('g.node').length;
      if (!intraN || !allN) return { ratio: null, nodes: ns.length, rendered, intraPairs: intraN };
      const intra = intraSum / intraN;
      const all = allSum / allN;
      return { intra, all, ratio: intra / all, nodes: ns.length, rendered, intraPairs: intraN };
    },

    /**
     * Mean distance between nodes whose folders share a real subtree, over
     * the same for nodes whose folders share nothing but the whole graph
     * (UI-069).
     *
     * `cohesionRatio` cannot see this: it asks whether a folder is tight, and
     * both populations here are *cross-folder* pairs, so a layout that
     * clusters leaves perfectly can still score 1.0 on kinship. 1.0 means the
     * tree above the leaf folder is invisible in the layout — which is what
     * the flat force produced. Lower means subtrees cohere.
     *
     * "A real subtree" is deeper than the drawn set's common ancestor, which
     * is the same thing the force means: a directory holding every drawn node
     * has the graph's centroid and can only act as a centering force, so a
     * pair that shares nothing deeper than that is unrelated as far as this
     * feature is concerned. Ghosts have no folder and are excluded, as
     * everywhere else.
     */
    kinshipRatio() {
      const ns = this.nodePositions().filter((n) => n.folder !== null);
      const segsOf = (f) => (f === '' ? [] : f.split('/'));
      const sharedDepth = (a, b) => {
        let i = 0;
        while (i < a.length && i < b.length && a[i] === b[i]) i++;
        return i;
      };
      const segs = ns.map((n) => segsOf(n.folder));
      // The common ancestor's depth. Everything at or above it holds the
      // whole canvas.
      let rootDepth = segs.length ? segs[0].length : 0;
      for (const s of segs) rootDepth = Math.min(rootDepth, sharedDepth(segs[0], s));

      let kinSum = 0, kinN = 0, farSum = 0, farN = 0;
      for (let i = 0; i < ns.length; i++) {
        for (let j = i + 1; j < ns.length; j++) {
          // Same folder is UI-052's business, and including it would let a
          // tight leaf folder carry the kinship number on its own.
          if (ns[i].folder === ns[j].folder) continue;
          const d = Math.hypot(ns[i].x - ns[j].x, ns[i].y - ns[j].y);
          if (sharedDepth(segs[i], segs[j]) > rootDepth) { kinSum += d; kinN++; }
          else { farSum += d; farN++; }
        }
      }
      const rendered = document.querySelectorAll('g.node').length;
      const folders = new Set(ns.map((n) => n.folder));
      // Report the populations even when the ratio is undefined: "no kin
      // pairs" means the scope has no two folders inside one subtree, which
      // is a fact about the scope and not a failure of the layout.
      if (!kinN || !farN) {
        return { ratio: null, kinPairs: kinN, farPairs: farN, folders: folders.size, nodes: ns.length, rendered };
      }
      const kin = kinSum / kinN;
      const far = farSum / farN;
      return {
        kin, far, ratio: kin / far,
        kinPairs: kinN, farPairs: farN, folders: folders.size, nodes: ns.length, rendered,
      };
    },

    /**
     * The share of nodes sitting nearer some *other* folder's centroid than
     * their own — split by whether the node has an edge leaving its folder
     * (UI-102).
     *
     * The number the other two cannot produce. `cohesionRatio` averages over
     * every node, so a folder whose three linked members were towed away
     * still scores well on the strength of the majority that stayed;
     * `kinshipRatio` only ever looks at cross-folder *pairs*. Neither can
     * separate "this folder is loose" from "three of its files were dragged
     * out by their edges", and the second is what a reader reports as folder
     * grouping not working on anything that isn't a free node.
     *
     * Nearest-centroid rather than distance-to-own-centroid, which was the
     * first build here and is wrong on real data. That version compared nodes
     * with a crossing edge against nodes without one — but on a real payload
     * the second population is mostly *free* nodes, which charge pushes to
     * the rim with nothing to pull them back, so it scored the strays as
     * closer to home than the control and the check could not fail. Membership
     * needs no control population: a node whose own region's centre is not the
     * nearest one is misplaced whatever else is on the canvas, and it is also
     * the literal thing the report describes.
     *
     * Only folders with `MIN_HULL_MEMBERS` members take part, as centroids
     * and as members. Below that UI-055 draws no outline, so there is no
     * region for a node to be inside or outside of, and a folder of one has
     * itself as its centroid and can never be misplaced.
     */
    folderMisplacement() {
      const MIN_MEMBERS = 3;
      const ns = this.nodePositions().filter((n) => n.folder !== null);
      const byId = new Map(ns.map((n) => [n.id, n]));

      // Nodes with at least one drawn edge whose other end is in a different
      // folder. Read off the links the canvas actually drew, not off the
      // payload: a filtered-out edge exerts no force and must not classify.
      const crossing = new Set();
      for (const l of document.querySelectorAll('line.link')) {
        if (l.style.display === 'none') continue;
        const d = l.__data__;
        if (!d) continue;
        const s = byId.get(typeof d.source === 'object' ? d.source?.id : d.source);
        const t = byId.get(typeof d.target === 'object' ? d.target?.id : d.target);
        if (!s || !t || s.folder === t.folder) continue;
        crossing.add(s.id); crossing.add(t.id);
      }

      const groups = new Map();
      for (const n of ns) {
        if (!groups.has(n.folder)) groups.set(n.folder, []);
        groups.get(n.folder).push(n);
      }
      const centroids = [];
      for (const [folder, members] of groups) {
        if (members.length < MIN_MEMBERS) continue;
        centroids.push({
          folder,
          x: members.reduce((a, n) => a + n.x, 0) / members.length,
          y: members.reduce((a, n) => a + n.y, 0) / members.length,
        });
      }
      const rendered = document.querySelectorAll('g.node').length;
      if (centroids.length < 2) {
        return { share: null, regions: centroids.length, nodes: ns.length, rendered };
      }

      let all = 0, allN = 0, cross = 0, crossN = 0, own = 0, ownN = 0;
      for (const c of centroids) {
        for (const n of groups.get(c.folder)) {
          let nearest = null, best = Infinity;
          for (const o of centroids) {
            const d = Math.hypot(n.x - o.x, n.y - o.y);
            if (d < best) { best = d; nearest = o.folder; }
          }
          const misplaced = nearest !== c.folder ? 1 : 0;
          all += misplaced; allN++;
          if (crossing.has(n.id)) { cross += misplaced; crossN++; } else { own += misplaced; ownN++; }
        }
      }
      return {
        share: all / allN,
        crossingShare: crossN ? cross / crossN : null,
        internalShare: ownN ? own / ownN : null,
        misplaced: all, measured: allN, crossingNodes: crossN, internalNodes: ownN,
        regions: centroids.length, nodes: ns.length, rendered,
      };
    },

    // ── hub demotion (UI-056) ────────────────────────────────────────────

    setDemote(on) {
      const cb = document.querySelector('[data-probe="demote-toggle"]');
      if (!cb) return false;
      if (cb.checked !== on) cb.click();
      return cb.checked === on;
    },

    setHubCount(n) {
      const b = document.querySelector(`[data-probe="hub-count-${n}"]`);
      if (!b) return false;
      b.click();
      return b.getAttribute('aria-pressed') === 'true';
    },

    /** Edges and nodes actually drawn, so a "fewer edges" claim is measured
     *  on the picture rather than on the plan. */
    drawnCounts() {
      const vis = (sel) => [...document.querySelectorAll(sel)].filter((e) => e.style.display !== 'none').length;
      return { links: vis('line.link'), nodes: vis('g.node') };
    },

    demotedList() {
      const p = document.querySelector('[data-probe="demoted-list"]');
      return p ? p.textContent.replace(/\s+/g, ' ').trim() : null;
    },

    nodeIsSelected(name) {
      const g = [...document.querySelectorAll('g.node')].find((n) => n.textContent.includes(name));
      return !!g?.classList.contains('selected');
    },

    clearSelection() {
      const b = [...document.querySelectorAll('button')]
        .find((x) => x.textContent.trim() === 'Clear Selection');
      if (b) b.click();
      return !!b;
    },

    /** Is a node still there and still clickable after being demoted? */
    nodeIsSelectable(name) {
      const g = [...document.querySelectorAll('g.node')]
        .filter((n) => n.style.display !== 'none')
        .find((n) => n.textContent.includes(name));
      if (!g) return { present: false };
      g.dispatchEvent(new MouseEvent('click', { bubbles: true }));
      return { present: true, selected: g.classList.contains('selected') };
    },

    // ── mixed-level expansion (UI-057 / UI-058) ──────────────────────────

    /** Shift + double-click a node by visible name — the expand gesture. */
    expandNode(name) {
      const g = [...document.querySelectorAll('g.node')]
        .filter((n) => n.style.display !== 'none')
        .find((n) => n.textContent.includes(name));
      if (!g) return false;
      g.dispatchEvent(new MouseEvent('dblclick', { bubbles: true, shiftKey: true }));
      return true;
    },

    expansionState() {
      const section = document.querySelector('[data-probe="expansion"]');
      const btn = document.querySelector('[data-probe="collapse-all"]');
      const hint = document.querySelector('[data-probe="expand-hint"]');
      return {
        count: btn ? Number((btn.textContent.match(/(\d+)/) ?? [])[1] ?? 0) : 0,
        hint: hint ? hint.textContent.trim() : null,
        // The gesture is spelled out in the section's own copy, not in a
        // tooltip — a hint nobody can see is not discoverability.
        gesture: section ? section.textContent.replace(/\s+/g, ' ').trim() : null,
      };
    },

    collapseAll() {
      const b = document.querySelector('[data-probe="collapse-all"]');
      if (!b) return false;
      b.click();
      return true;
    },

    /** Kinds of the nodes actually drawn — the proof that a view is mixed. */
    drawnNodeKinds() {
      const out = {};
      for (const g of document.querySelectorAll('g.node')) {
        if (g.style.display === 'none') continue;
        const k = g.__data__?.kind_raw ?? '?';
        out[k] = (out[k] ?? 0) + 1;
      }
      return out;
    },

    /** Kinds of the edges actually drawn (UI-058). */
    drawnLinkKinds() {
      const out = {};
      for (const l of document.querySelectorAll('line.link')) {
        if (l.style.display === 'none') continue;
        const k = l.__data__?.kind_raw ?? '?';
        out[k] = (out[k] ?? 0) + 1;
      }
      return out;
    },

    firstNodeOfKind(kind) {
      const g = [...document.querySelectorAll('g.node')]
        .filter((n) => n.style.display !== 'none')
        .find((n) => n.__data__?.kind_raw === kind);
      return g ? (g.__data__?.name ?? null) : null;
    },

    // ── the marked set ───────────────────────────────────────────────────

    /** ⌘-click the first `n` drawn nodes — the mark gesture. Returns the
     *  paths marked, which is what the drill will be scoped to. */
    markNodes(n) {
      const gs = [...document.querySelectorAll('g.node')]
        .filter((g) => g.style.display !== 'none')
        .slice(0, n);
      for (const g of gs) g.dispatchEvent(new MouseEvent('click', { bubbles: true, metaKey: true }));
      return gs.map((g) => g.__data__?.file_path ?? null);
    },

    /** What the canvas and the toolbar each say about the set. They are two
     *  surfaces for one store and disagreeing is the failure to catch. */
    markState() {
      const btn = document.querySelector('[data-probe="drill-marks"]');
      return {
        rings: document.querySelectorAll('circle.mark-ring').length,
        marked: document.querySelectorAll('g.node.marked').length,
        selected: document.querySelectorAll('g.node.selected').length,
        button: btn ? btn.textContent.replace(/\s+/g, ' ').trim() : null,
        level: [...document.querySelectorAll('.level-btn')]
          .find((b) => b.classList.contains('active'))?.textContent.trim() ?? null,
      };
    },

    drillMarks() {
      const b = document.querySelector('[data-probe="drill-marks"]');
      if (!b) return false;
      b.click();
      return true;
    },

    // ── hover highlight (UI-054) ─────────────────────────────────────────

    setHoverMode(mode) {
      const b = document.querySelector(`[data-probe="hover-mode-${mode}"]`);
      if (!b || b.disabled) return false;
      b.click();
      return b.getAttribute('aria-pressed') === 'true';
    },

    hoverModeState() {
      return ['connections', 'group'].map((m) => {
        const b = document.querySelector(`[data-probe="hover-mode-${m}"]`);
        return { mode: m, present: !!b, active: b?.getAttribute('aria-pressed') === 'true', disabled: !!b?.disabled };
      });
    },

    /** Are the depth buttons available? They only mean something in Links
     *  mode, and a control that silently does nothing is worse than one that
     *  says it is unavailable. */
    depthEnabled() {
      const group = [...document.querySelectorAll('.toolbar-group')]
        .find((g) => /Highlight depth/i.test(g.textContent ?? ''));
      const btns = [...(group?.querySelectorAll('button') ?? [])];
      return { count: btns.length, enabled: btns.filter((b) => !b.disabled).length };
    },

    /** Hover a node by visible name and report what stayed lit.
     *  Returns null when no such node is drawn. */
    hoverNode(name) {
      const g = [...document.querySelectorAll('g.node')]
        .filter((n) => n.style.display !== 'none')
        .find((n) => n.textContent.includes(name));
      if (!g) return null;
      g.dispatchEvent(new MouseEvent('mouseover', { bubbles: true }));
      const folderOf = (el) => this.folderOfPath(el.__data__?.file_path ?? '');
      const all = [...document.querySelectorAll('g.node')].filter((n) => n.style.display !== 'none');
      const lit = all.filter((n) => !n.classList.contains('dimmed'));
      return {
        hovered: name,
        hoveredFolder: folderOf(g),
        drawn: all.length,
        lit: lit.length,
        litFolders: [...new Set(lit.map(folderOf))],
      };
    },

    unhoverAll() {
      for (const g of document.querySelectorAll('g.node')) {
        g.dispatchEvent(new MouseEvent('mouseout', { bubbles: true }));
      }
      return document.querySelectorAll('g.node.dimmed').length;
    },

    /** Folder hulls (UI-055): what is outlined, what it is called, and
     *  where it sits in paint order. */
    hulls() {
      const gs = [...document.querySelectorAll('g.folder-hull')];
      return gs.map((g) => {
        const path = g.querySelector('path.hull-shape');
        const text = g.querySelector('text.hull-label');
        const cs = path ? getComputedStyle(path) : null;
        return {
          label: text?.textContent?.trim() ?? null,
          // The full directory, which the label does not carry — it shows the
          // last segment only. Ancestry questions need this (UI-070).
          path: g.getAttribute('data-path'),
          parent: g.classList.contains('parent'),
          d: path?.getAttribute('d')?.slice(0, 12) ?? null,
          fillOpacity: path ? Number(path.getAttribute('fill-opacity')) : null,
          fill: cs?.fill ?? null,
          // The shape's own value and the group's, separately: since UI-071
          // the layer opts in part by part — inert group, hittable shape and
          // label — and a single number could not tell that from a layer that
          // is hittable all over.
          pointerEvents: cs?.pointerEvents ?? null,
          groupPointerEvents: getComputedStyle(g).pointerEvents,
          fontSize: text ? getComputedStyle(text).fontSize : null,
        };
      });
    },

    /** For each drawn outline, how many visible nodes inside it belong to
     *  that folder versus another one.
     *
     *  Uses `isPointInFill` against the rendered curve rather than
     *  re-deriving the polygon, so it measures the shape the user sees.
     *  This is the check whose absence let the first build pass: outlines
     *  were counted, named and painted in the right order while being five
     *  overlapping sheets that enclosed each other's nodes. */
    hullPurity() {
      const nodes = this.nodePositions().filter((n) => n.folder !== null);

      return [...document.querySelectorAll('g.folder-hull')].map((g) => {
        const path = g.querySelector('path.hull-shape');
        const label = g.querySelector('text')?.textContent?.trim() ?? null;
        // The region's directory, read off the element rather than guessed
        // from the label. The label carries the last segment only, so
        // matching on it could not tell `ui/src/stores` from any other
        // `stores`, and — since UI-070 — could not tell a region's own
        // descendants from strangers either.
        const dir = g.getAttribute('data-path');
        if (!path || !path.isPointInFill) return { label, path: dir, own: 0, foreign: 0, share: null };
        let own = 0, foreign = 0;
        for (const n of nodes) {
          let inside = false;
          try { inside = path.isPointInFill(new DOMPoint(n.x, n.y)); } catch { inside = false; }
          if (!inside) continue;
          // A node under this region is its own: a parent outline is supposed
          // to contain its children's nodes.
          const mine = dir === null
            ? false
            : n.folder === dir || (dir === '' ? true : n.folder.startsWith(`${dir}/`));
          if (mine) own++; else foreign++;
        }
        return { label, path: dir, own, foreign, share: own + foreign ? foreign / (own + foreign) : null };
      });
    },

    /**
     * Every region name as a screen-space box, with the pairs that collide.
     *
     * Measured off `getBoundingClientRect` rather than the hull geometry
     * because the failure is typographic: two names are unreadable when the
     * *glyphs* overlap, and how wide a name is is a fact about the rendered
     * text that `computeFolderHulls` cannot know. A pair is reported when the
     * boxes intersect on both axes.
     */
    hullLabelBoxes() {
      const labels = [...document.querySelectorAll('g.folder-hull text.hull-label')].map((t) => {
        const r = t.getBoundingClientRect();
        const g = t.closest('g.folder-hull');
        return {
          label: t.textContent.trim(),
          path: g?.getAttribute('data-path') ?? null,
          parent: g?.classList.contains('parent') ?? false,
          x: Math.round(r.x), y: Math.round(r.y),
          w: Math.round(r.width), h: Math.round(r.height),
        };
      // A name scrolled off the canvas is not one the reader can misread.
      }).filter((l) => l.w > 0 && l.h > 0);

      const collisions = [];
      for (let i = 0; i < labels.length; i++) {
        for (let j = i + 1; j < labels.length; j++) {
          const a = labels[i], b = labels[j];
          const ox = Math.min(a.x + a.w, b.x + b.w) - Math.max(a.x, b.x);
          const oy = Math.min(a.y + a.h, b.y + b.h) - Math.max(a.y, b.y);
          if (ox > 0 && oy > 0) collisions.push({ a: a.path, b: b.path, overlapX: ox, overlapY: oy });
        }
      }
      return { labels, collisions };
    },

    /** Index of the hull layer among the canvas's top-level groups. Lower
     *  means painted earlier, i.e. further back. */
    hullPaintOrder() {
      const root = document.querySelector('.graph-container svg > g');
      if (!root) return null;
      const kids = [...root.children].map((c) => c.getAttribute('class'));
      return { order: kids, hulls: kids.indexOf('file-hulls'), nodes: kids.indexOf('nodes-group'), links: kids.indexOf('links-group') };
    },

    /** Font size of an entity name label, to compare against a hull label. */
    nameLabelFontSize() {
      const t = document.querySelector('g.node text.name-label');
      return t ? getComputedStyle(t).fontSize : null;
    },

    setHulls(on) {
      const cb = document.querySelector('[data-probe="hulls-toggle"]');
      if (!cb) return false;
      if (cb.checked !== on) cb.click();
      return cb.checked === on;
    },

    /** How many tiers of the folder tree get an outline (UI-070). */
    setHullDepth(n) {
      const b = document.querySelector(`[data-probe="hull-depth-${n}"]`);
      if (!b) return false;
      b.click();
      return b.getAttribute('aria-pressed') === 'true';
    },

    hullDepthState() {
      return [...document.querySelectorAll('[data-probe^="hull-depth-"]')].map((b) => ({
        depth: Number(b.getAttribute('data-probe').replace('hull-depth-', '')),
        label: b.textContent.trim(),
        active: b.getAttribute('aria-pressed') === 'true',
      }));
    },

    /** Which level of the tree is the innermost region (UI-103). */
    setGroupGrain(grain) {
      const b = document.querySelector(`[data-probe="group-grain-${grain}"]`);
      if (!b) return false;
      b.click();
      return b.getAttribute('aria-pressed') === 'true';
    },

    groupGrainState() {
      return [...document.querySelectorAll('[data-probe^="group-grain-"]')].map((b) => ({
        grain: b.getAttribute('data-probe').replace('group-grain-', ''),
        label: b.textContent.trim(),
        active: b.getAttribute('aria-pressed') === 'true',
        // Italic = chosen but not in force at this level, which is the
        // sidebar's only way of saying so.
        inert: b.classList.contains('grain-inert'),
      }));
    },

    // ── standing in a region (UI-071 / UI-089) ───────────────────────────

    /**
     * Is `(x, y)` inside this outline with room to spare, in local units?
     *
     * The margin is the whole point. The canvas strokes a Catmull-Rom curve
     * *through* the hull polygon, so the drawn shape bulges a few units past
     * the polygon the app hit-tests — and `isPointInFill` measures the drawn
     * shape. A sample near a boundary can therefore be inside for the probe
     * and outside for the card, which is a disagreement between two rulers
     * rather than a bug in either.
     */
    deepInside(shape, x, y, m) {
      return shape.isPointInFill(new DOMPoint(x, y))
        && shape.isPointInFill(new DOMPoint(x + m, y))
        && shape.isPointInFill(new DOMPoint(x - m, y))
        && shape.isPointInFill(new DOMPoint(x, y + m))
        && shape.isPointInFill(new DOMPoint(x, y - m));
    },

    /**
     * A point unambiguously inside `shape`, in client space, with the regions
     * it stands in.
     *
     * "Unambiguously" is enforced against *every* drawn region, not just this
     * one: a sample that sits on some other outline's edge would have the
     * card and the probe disagreeing about that region, and the check would
     * fail for a reason that has nothing to do with what it is testing.
     *
     * The point is also required to be one where a region is the topmost
     * element, so a check that clicks there exercises the region's own
     * handler and not a node sitting over it.
     *
     * `paths` comes back in paint order — outermost first — which is the
     * order the card is supposed to list.
     */
    regionSampleIn(shapes, shape, ctm) {
      const MARGIN = 12;
      const dirOf = (s) => s.parentElement.getAttribute('data-path');
      const bb = shape.getBBox();
      for (let gy = 1; gy < 12; gy++) {
        for (let gx = 1; gx < 12; gx++) {
          const x = bb.x + (bb.width * gx) / 12;
          const y = bb.y + (bb.height * gy) / 12;
          if (!this.deepInside(shape, x, y, MARGIN)) continue;
          const c = new DOMPoint(x, y).matrixTransform(ctm);
          const top = document.elementFromPoint(c.x, c.y);
          if (!top || !top.classList.contains('hull-shape')) continue;
          let ambiguous = false;
          const paths = [];
          for (const other of shapes) {
            const inside = other.isPointInFill(new DOMPoint(x, y));
            if (inside !== this.deepInside(other, x, y, MARGIN)) { ambiguous = true; break; }
            if (inside) paths.push(dirOf(other));
          }
          if (ambiguous || paths.length === 0) continue;
          return {
            x: c.x,
            y: c.y,
            // The region that owns this point is the one painted last among
            // those containing it — the same rule the double-click follows,
            // and not necessarily the shape whose box was being sampled.
            tightest: paths[paths.length - 1],
            paths,
          };
        }
      }
      return null;
    },

    /** A point inside the innermost region that yields one. Innermost
     *  outward: a tight region can be so full of nodes that every sample
     *  lands on one, and giving up there would report "no region drawn"
     *  against a canvas covered in them. */
    regionProbePoint() {
      const shapes = [...document.querySelectorAll('g.folder-hull path.hull-shape')];
      if (!shapes.length) return null;
      // All the hulls share one parent transform, so one local coordinate
      // system serves every containment test here.
      const ctm = shapes[0].getScreenCTM();
      if (!ctm) return null;
      for (let i = shapes.length - 1; i >= 0; i--) {
        const found = this.regionSampleIn(shapes, shapes[i], ctm);
        if (found) return found;
      }
      return null;
    },

    /** One probe point per drawn region, innermost first.
     *
     *  Which folders a spec happens to claim is a property of the repo under
     *  test, so a check about the spec join has to sweep the regions rather
     *  than assert against whichever one came out tightest. */
    regionProbePoints() {
      const shapes = [...document.querySelectorAll('g.folder-hull path.hull-shape')];
      if (!shapes.length) return [];
      const ctm = shapes[0].getScreenCTM();
      if (!ctm) return [];
      const out = [];
      for (let i = shapes.length - 1; i >= 0; i--) {
        const found = this.regionSampleIn(shapes, shapes[i], ctm);
        if (found) out.push(found);
      }
      return out;
    },

    /** A client-space point over the canvas that is in no region at all —
     *  where the card has to be absent rather than showing the nearest
     *  guess. */
    pointOutsideRegions() {
      const svg = document.querySelector('.graph-container svg');
      if (!svg) return null;
      const r = svg.getBoundingClientRect();
      for (let gy = 1; gy < 12; gy++) {
        for (let gx = 1; gx < 12; gx++) {
          const x = r.left + (r.width * gx) / 12;
          const y = r.top + (r.height * gy) / 12;
          if (document.elementFromPoint(x, y) === svg) return { x, y };
        }
      }
      return null;
    },

    /** Move the pointer to a client point and report what the card says.
     *
     *  The wait is not padding: the card is Svelte state, so it renders on
     *  the tick after the event. Reading synchronously returns the *previous*
     *  hover — which reads as a pass anywhere the answer did not change, and
     *  as "no card" only on the first move.
     *
     *  Polled to stability rather than slept on a fixed delay. A single 60ms
     *  sleep passed most of the time and failed a few runs in ten on a loaded
     *  machine, which is the worst kind of check: it reports a bug that is
     *  really the harness reading too early. Two identical consecutive
     *  samples is the same "settled" test `waitSettled` uses on positions. */
    async hoverPoint(x, y) {
      const svg = document.querySelector('.graph-container svg');
      if (!svg) return null;
      svg.dispatchEvent(new PointerEvent('pointermove', { bubbles: true, clientX: x, clientY: y }));
      let prev = null;
      for (let i = 0; i < 12; i++) {
        await new Promise((r) => setTimeout(r, 40));
        const now = this.regionCard();
        const key = now === null ? 'none' : now.rows.map((r) => r.path).join('>') + '|' + (now.spec?.head ?? '');
        if (prev === key) return now;
        prev = key;
      }
      return this.regionCard();
    },

    /** The "where am I" card, or null when there isn't one. */
    regionCard() {
      const card = document.querySelector('[data-probe="region-card"]');
      if (!card) return null;
      const box = card.getBoundingClientRect();
      return {
        rows: [...card.querySelectorAll('[data-probe="region-row"]')].map((r) => ({
          path: r.getAttribute('data-region-path'),
          text: r.textContent.replace(/\s+/g, ' ').trim(),
          tightest: r.classList.contains('tightest'),
        })),
        path: card.querySelector('[data-probe="region-path"]')?.textContent.trim() ?? null,
        hint: card.querySelector('[data-probe="region-hint"]')?.textContent.trim() ?? null,
        // The spec claim (UI-097): who says something about this folder, and
        // whether the words are about the folder itself or an ancestor.
        spec: (() => {
          const s = card.querySelector('[data-probe="region-spec"]');
          if (!s) return null;
          return {
            id: s.getAttribute('data-spec-id'),
            head: s.querySelector('.region-spec-head')?.textContent.replace(/\s+/g, ' ').trim() ?? null,
            via: s.querySelector('[data-probe="region-spec-via"]')?.textContent.trim() ?? null,
            desc: s.querySelector('[data-probe="region-spec-desc"]')?.textContent.trim() ?? null,
          };
        })(),
        unclaimed: card.querySelector('[data-probe="region-spec-none"]') !== null,
        // What the region's relationships do (UI-071's second half). Read as
        // text on purpose: the claim is that the card makes a readable
        // statement about containment, not that a number reached the DOM.
        traffic: card.querySelector('[data-probe="region-traffic"]')?.textContent.replace(/\s+/g, ' ').trim() ?? null,
        counts: [...card.querySelectorAll('[data-probe="region-count"]')].map((c) => ({
          text: c.textContent.trim(),
          partial: c.classList.contains('partial'),
        })),
        box: { left: box.left, top: box.top, right: box.right, bottom: box.bottom },
        pointerEvents: getComputedStyle(card).pointerEvents,
      };
    },

    /** Polled for the same reason `hoverPoint` is: the disappearance is a
     *  render, and a fixed sleep turns a slow frame into a failed check. */
    async leaveCanvas() {
      const svg = document.querySelector('.graph-container svg');
      if (!svg) return null;
      svg.dispatchEvent(new PointerEvent('pointerleave', { bubbles: true }));
      for (let i = 0; i < 15; i++) {
        if (document.querySelector('[data-probe="region-card"]') === null) return true;
        await new Promise((r) => setTimeout(r, 40));
      }
      return false;
    },

    /** Click whatever is on top at a client point — the deselect regression
     *  (UI-071): over a region that is now hittable, this still has to be a
     *  click on nothing. */
    clickPoint(x, y) {
      const el = document.elementFromPoint(x, y);
      if (!el) return null;
      el.dispatchEvent(new MouseEvent('click', { bubbles: true, clientX: x, clientY: y }));
      return {
        on: el.getAttribute('class'),
        selected: document.querySelectorAll('g.node.selected').length,
      };
    },

    /** Double-click a region — the focus gesture (UI-089).
     *
     *  Dispatched on whatever is topmost, which is what a real pointer hits.
     *  Deliberately *not* aimed at the outline element: the outline is the
     *  backmost layer, so an edge crossing the region gets the event instead,
     *  and a probe that reached past it would be testing a gesture no reader
     *  can make. The class it landed on is returned so a failure says which. */
    dblclickPoint(x, y) {
      const el = document.elementFromPoint(x, y);
      if (!el) return null;
      el.dispatchEvent(new MouseEvent('dblclick', { bubbles: true, clientX: x, clientY: y }));
      return el.getAttribute('class');
    },

    /** The folder of every drawn node that has one — what a focused view is
     *  allowed to contain.
     *
     *  Ghosts are excluded. An external symbol has no file, so it belongs to
     *  no folder and cannot be "outside" the region: drilling into a small
     *  scope reopens it at entity level, which is exactly when ghosts appear
     *  in numbers. Counting them as strangers would fail the check for doing
     *  the right thing. */
    drawnFolders() {
      return [...document.querySelectorAll('g.node')]
        .filter((g) => g.style.display !== 'none' && !g.classList.contains('ghost'))
        .map((g) => g.__data__?.file_path ?? '')
        .filter((p) => p !== '')
        .map((p) => this.folderOfPath(p));
    },

    /** UI-092. The wayback's two controls, plus how deep the stack is.
     *
     *  Depth comes off `data-depth` because it is the one thing about this
     *  feature no pixel answers: "did that gesture record a frame" is
     *  invisible until you press the button, and a check that has to press
     *  the button to find out cannot then assert that nothing moved. */
    wayback() {
      const back = document.querySelector('[data-probe="wayback-back"]');
      const fwd = document.querySelector('[data-probe="wayback-forward"]');
      if (!back || !fwd) return null;
      return {
        depth: Number(back.closest('.wayback')?.getAttribute('data-depth') ?? -1),
        backEnabled: !back.disabled,
        forwardEnabled: !fwd.disabled,
        backTitle: back.getAttribute('title') ?? '',
        forwardTitle: fwd.getAttribute('title') ?? '',
      };
    },

    clickWayback(dir) {
      const b = document.querySelector(`[data-probe="wayback-${dir}"]`);
      if (!b || b.disabled) return false;
      b.click();
      return true;
    },

    /** UI-095. The address strip: what it says, and what of it is climbable.
     *
     *  `clickable` is read off the tag rather than a class, because the claim
     *  is that the crumb you are standing on is not a control at all — a
     *  `<span>` styled to look inert would still be clickable if someone gave
     *  it a handler. */
    crumbs() {
      const nav = document.querySelector('[data-probe="scope-crumbs"]');
      if (!nav) return null;
      return {
        parts: [...nav.querySelectorAll('.crumb')].map((c) => ({
          label: c.textContent.trim(),
          clickable: c.tagName === 'BUTTON',
          current: c.classList.contains('current'),
        })),
        elided: !!nav.querySelector('.elided'),
        filtered: !!nav.querySelector('.filtered'),
      };
    },

    clickCrumb(label) {
      const b = [...document.querySelectorAll('[data-probe="scope-crumbs"] button.crumb')]
        .find((c) => c.textContent.trim() === label);
      if (!b) return false;
      b.click();
      return true;
    },

    /** Double-click the first drawn collapsed node — the canonical drill,
     *  and the gesture every other one in UI-092 is measured against. */
    drillFirstCollapsed() {
      const g = [...document.querySelectorAll('g.node')].find(
        (n) => n.style.display !== 'none' && ['File', 'Module'].includes(n.__data__?.kind_raw),
      );
      if (!g) return null;
      g.dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
      return g.__data__.original_id;
    },

    /** The paths the scope tree says are in scope — what a restore has to
     *  put back. */
    scopeSelection() {
      return [...document.querySelectorAll('.scope-tree .tree-item')]
        .filter((r) => r.querySelector('.select-cb')?.checked)
        .map((r) => r.querySelector('.label')?.getAttribute('title') ?? '')
        .filter(Boolean)
        .sort();
    },

    /** Select the first drawn node, so the deselect check has something to
     *  lose. Returns how many nodes ended up selected. */
    selectFirstNode() {
      const g = [...document.querySelectorAll('g.node')].find((n) => n.style.display !== 'none');
      if (!g) return 0;
      g.dispatchEvent(new MouseEvent('click', { bubbles: true }));
      return document.querySelectorAll('g.node.selected').length;
    },

    setCohesion(level) {
      const b = document.querySelector(`[data-probe="cohesion-${level}"]`);
      if (!b) return false;
      b.click();
      return true;
    },

    cohesionState() {
      return [...document.querySelectorAll('[data-probe="cohesion"] .seg-btn')]
        .map((b) => ({ label: b.textContent.trim(), active: b.getAttribute('aria-pressed') === 'true' }));
    },

    /** Widen the diff to its top rung, and report whether a diff was active.
     *
     *  Originally a workaround: with a diff loaded, File and Module
     *  aggregation drew nothing at all, so a layout suite measured an empty
     *  canvas. UI-064 fixed that — collapsed nodes now resolve through a
     *  scope rollup instead of missing an entity-keyed lookup.
     *
     *  It stays because the layout suites want *every* node in the scope,
     *  not the changed ones. The ladder defaults to its narrowest rung
     *  (UI-088), so a suite that did not widen would measure a real but
     *  partial canvas — which is worse than an obviously empty one. */
    clearDiffFilters() {
      const rungs = [...document.querySelectorAll('[data-probe="diff-level"] button')];
      const top = document.querySelector('[data-probe="diff-level-neighbourhood"]');
      const cleared = top && !top.classList.contains('active') ? (top.click(), 1) : 0;
      return { diffActive: rungs.length > 0, cleared };
    },

    /** Move the diff ladder to one rung: 'edits' | 'rewiring' |
     *  'neighbourhood'. Returns the rung that ended up active. */
    setDiffLevel(level) {
      document.querySelector(`[data-probe="diff-level-${level}"]`)?.click();
      return document.querySelector('[data-probe="diff-level"] button.active')?.dataset.probe ?? null;
    },

    /** Enough of the diff bar to tell "no diff loaded" from "diff loaded and
     *  drawing nothing" — the two look identical on the canvas and only one
     *  of them is a bug. */
    diffState() {
      const rungs = [...document.querySelectorAll('[data-probe="diff-level"] button')];
      const nodes = [...document.querySelectorAll('g.node')];
      const active = rungs.find((b) => b.classList.contains('active'));
      const counts = document.querySelector('[data-probe="stats-counts"]')?.textContent ?? '';
      return {
        active: rungs.length > 0,
        level: active ? active.textContent.trim().toLowerCase() : null,
        dom: nodes.length,
        shown: nodes.filter((n) => getComputedStyle(n).display !== 'none').length,
        // The stats bar is the one place that reports *edges*, and edges are
        // what UI-088 is about — a rung that changed only the node count
        // would have missed the point entirely.
        links: Number((counts.match(/(\d+)\s+relationships/) ?? [0, 0])[1]),
        undrawable: document.querySelector('[data-probe="diff-undrawable"]')?.textContent.trim() ?? null,
        // UI-112. A rung above `edits` draws code the reader did not touch,
        // and the only place that distinction lands is the rendered opacity —
        // so it is read off the nodes rather than off the control.
        faded: nodes.filter((n) => {
          const st = getComputedStyle(n);
          const o = Number(st.opacity);
          return st.display !== 'none' && o > 0 && o < 1;
        }).length,
        contextPct: (() => {
          const el = document.querySelector('[data-probe="diff-context-opacity"]');
          return el ? Number(el.value) : null;
        })(),
      };
    },

    /** What the details pane is drawing for the selected entity's source
     *  (UI-085). `null` when there is no diff renderer on the page at all,
     *  which is the state that shipped for as long as the base-source lookup
     *  was keyed the wrong way. */
    sourceDiff() {
      const el = document.querySelector('[data-probe="source-diff"]');
      if (!el) return null;
      const counts = el.querySelector('.counts')?.textContent ?? '';
      const num = (re) => Number((counts.match(re) ?? [0, 0])[1]);
      const rows = [...el.querySelectorAll('.split-row')];
      return {
        mode: el.dataset.diffMode,
        added: num(/\+(\d+)/),
        removed: num(/[−-](\d+)/),
        unifiedLines: el.querySelectorAll('.diff-line').length,
        splitRows: rows.length,
        splitCells: rows.reduce((n, r) => n + r.querySelectorAll('.cell').length, 0),
        addedLines: el.querySelectorAll('.diff-line-added').length,
        removedLines: el.querySelectorAll('.diff-line-removed').length,
        gaps: [...el.querySelectorAll('[data-probe="diff-gap"]')].map((g) => g.textContent.trim()),
        fits: el.scrollWidth <= el.clientWidth + 1,
      };
    },

    /** Click one of the diff view's own controls by probe name. */
    clickProbe(name) {
      const b = document.querySelector(`[data-probe="${name}"]`);
      if (!b) return false;
      b.click();
      return true;
    },

    /** The relationships the selected entity gained and lost (UI-086). */
    relChanges() {
      return [...document.querySelectorAll('[data-probe="rel-change"]')].map((r) => ({
        status: r.classList.contains('rc-added') ? 'added' : 'removed',
        text: r.textContent.replace(/\s+/g, ' ').trim(),
        navigable: !r.disabled,
      }));
    },

    /** Type into the dataset search. The one route to a named entity that
     *  does not depend on where the canvas happened to draw it. */
    searchFor(name) {
      const input = el('search-input');
      if (!input) return false;
      const set = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
      set.call(input, name);
      input.dispatchEvent(new Event('input', { bubbles: true }));
      return true;
    },

    /** Pin a search result into the details pane, matched on name *and*
     *  file. Neither alone is enough: the search also matches file and
     *  folder names, so asking for a function in `diff.rs` by either half
     *  can pin the file entity instead and every later assertion then
     *  describes the wrong subject. */
    searchPick(name, file) {
      const items = [...document.querySelectorAll('[data-probe="search-results"] .search-result-item')];
      const hit = items.find((li) =>
        li.querySelector('.result-name')?.textContent.trim() === name
        && (li.getAttribute('title') ?? '').includes(file));
      const btn = hit?.querySelector('.row-btn');
      if (!btn) return false;
      btn.click();
      return true;
    },

    /** Which entity the details pane is currently pinned to. */
    pinnedEntity() {
      return document.querySelector('[data-probe="entity-name"]')?.textContent.trim() ?? null;
    },

    /** Persisted like the theme — raw, not JSON (settings.ts). Set this
     *  before a reload to control the strength the next load starts at. */
    storeCohesion(level) {
      try { localStorage.setItem('nao-folder-cohesion', level); } catch { /* ignore */ }
      return localStorage.getItem('nao-folder-cohesion') === level;
    },

    /** Set the grain before a load, so a measurement gets the UI-053 seed for
     *  that grain rather than inheriting the previous grain's settle — the
     *  same reason `storeCohesion` exists. */
    storeGrain(grain) {
      try { localStorage.setItem('nao-group-grain', grain); } catch { /* ignore */ }
      return localStorage.getItem('nao-group-grain') === grain;
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

/**
 * Clear the diff filters once they actually exist.
 *
 * The diff loads asynchronously after a scope pick, so a single immediate
 * call can run before the bar is in the DOM, silently do nothing, and show
 * up thousands of milliseconds later as an empty canvas that looks like a
 * layout failure. Polls until it clears something or until nodes are
 * visible (a clean tree loads no diff and needs no clearing). See UI-061.
 */
async function clearDiff(page) {
  for (let i = 0; i < 12; i++) {
    const r = await page.eval(() => window.__probe.clearDiffFilters());
    if (r.cleared > 0) return r;
    const n = await page.eval(() => window.__probe.nodePositions().length);
    if (n > 0) return { diffActive: r.diffActive, cleared: 0, nodes: n };
    await sleep(1000);
  }
  return { diffActive: false, cleared: 0 };
}

/**
 * Wait until the force simulation stops moving, and report how long it took.
 *
 * A fixed sleep cannot stand in for this. The settle takes as long as the
 * node count and the forces make it take, so "sleep 9s then compare two
 * runs" is a race: sample one run mid-settle and the other after it, and the
 * positions differ for reasons that have nothing to do with the thing under
 * test. Polls for two consecutive identical samples instead.
 */
async function waitSettled(page, maxMs = 30000) {
  let prev = null;
  const started = Date.now();
  while (Date.now() - started < maxMs) {
    const now = await page.eval(() => window.__probe.nodePositions());
    if (prev && now.length && now.length === prev.length) {
      const by = new Map(prev.map((n) => [n.id, n]));
      let worst = 0;
      for (const n of now) {
        const p = by.get(n.id);
        if (!p) { worst = Infinity; break; }
        worst = Math.max(worst, Math.hypot(n.x - p.x, n.y - p.y));
      }
      if (worst < 0.5) return { settledMs: Date.now() - started, nodes: now.length };
    }
    prev = now;
    await sleep(500);
  }
  return { settledMs: -1, nodes: prev?.length ?? 0 };
}

/**
 * The top-level scope row holding the most entities.
 *
 * Resolved once and reused, so every level in a run measures the same scope —
 * comparing two cohesion settings across two different scopes would be
 * measuring nothing.
 */
async function widestScope(page) {
  if (widestScope.chosen) return widestScope.chosen;
  const rows = await page.eval(() => window.__probe.scopeRowCounts());
  let best = null;
  for (const r of rows) {
    if (!best || r.entities > best.entities) best = r;
  }
  widestScope.chosen = best?.path ?? '.';
  return widestScope.chosen;
}
widestScope.chosen = null;

/**
 * Settle one cohesion level from a fresh page load, and report both grouping
 * numbers for it (UI-069).
 *
 * A fresh load per level, rather than clicking the control on a live graph.
 * Toggling restarts alpha at 0.3 from wherever the previous setting left the
 * nodes, so the measurement inherits that local minimum: measured that way,
 * one configuration scored anywhere in a ±0.08 band across runs and the
 * suite flapped between pass and fail on unchanged code. Reloading gives
 * every level the identical UI-053 seed to start from.
 *
 * The scope is the biggest top-level row rather than a name written here.
 * Kinship needs folders nested two deep *inside* the scope, and which row
 * offers that is a property of the payload, not of this repo: today the
 * largest is `.`, because one analyze run spells 9,445 of its entities
 * `./src/…` and the rest plainly (AN-015). Naming a folder here would tie
 * the suite to that bug and break the day it is fixed. Picking the largest
 * subtree keeps working either way, and a scope that genuinely has no depth
 * — `ui`, whose folders all sit directly under `ui/src` — is reported as
 * such by the first check rather than measured as a failure.
 *
 * Cached per level for the process, because each call costs a page load and
 * a full settle, and three checks want the same two numbers.
 */
async function settledGrouping(page, level) {
  const hit = settledGrouping.cache.get(level);
  if (hit) return hit;
  await page.eval((l) => window.__probe.storeCohesion(l), level);
  await page.goto(APP); await ready(page);
  const scope = await widestScope(page);
  // Idempotent, because the scope selection persists across reloads and the
  // checkbox *toggles*: clicking a row a previous measurement already
  // selected deselects it, and the canvas comes back holding one file. That
  // failure looks exactly like a layout bug and is not one.
  const scoped = await page.eval((p) => window.__probe.scopeRowState(p), scope);
  if (!scoped?.checked) await page.eval((p) => window.__probe.toggleScopePath(p), scope);
  // See `clearDiff` / UI-061 — a dirty working tree otherwise hides every
  // node at File aggregation and this suite measures an empty canvas.
  await clearDiff(page);
  await waitSettled(page);
  const out = {
    kinship: await page.eval(() => window.__probe.kinshipRatio()),
    cohesion: await page.eval(() => window.__probe.cohesionRatio()),
    // UI-102. Read from the same settle as the other two rather than from a
    // load of its own: the three numbers describe one layout, and taking them
    // from different settles would let a run-to-run wobble show up as a
    // trade-off between them that never happened.
    stray: await page.eval(() => window.__probe.folderMisplacement()),
  };
  settledGrouping.cache.set(level, out);
  return out;
}
settledGrouping.cache = new Map();

/**
 * Settle one *grain* from a fresh load and report what it drew (UI-103).
 *
 * A sibling of `settledGrouping` rather than a parameter on it, because the
 * two vary different things and share only the settle. Same fresh-load
 * discipline and for the same reason: switching grain on a live graph
 * restarts alpha from wherever the previous grain left the nodes, so the
 * measurement would inherit that local minimum instead of starting from the
 * UI-053 seed the grain actually gets in use.
 *
 * Cohesion is pinned at `low` — the shipped default, and the setting the
 * folder numbers on UI-102 were taken at, so the two tickets' figures are
 * comparable.
 */
async function settledGrain(page, grain) {
  const hit = settledGrain.cache.get(grain);
  if (hit) return hit;
  await page.eval((g) => window.__probe.storeGrain(g), grain);
  await page.eval(() => window.__probe.storeCohesion('low'));
  await page.goto(APP); await ready(page);
  const scope = await widestScope(page);
  const scoped = await page.eval((p) => window.__probe.scopeRowState(p), scope);
  if (!scoped?.checked) await page.eval((p) => window.__probe.toggleScopePath(p), scope);
  await clearDiff(page);
  // Entity level, explicitly, and this is the whole reason the suite cannot
  // reuse `settledGrouping`. `groupGrainFor` collapses the grain to `folder`
  // anywhere else — correctly, since at File level every node already is a
  // file — so a suite that let `autoLevel` pick would measure the folder
  // grain twice and report the two as identical, which is exactly what the
  // first version of this did.
  await page.eval(() => window.__probe.setLevel('Entity'));
  await waitSettled(page);
  const out = {
    hulls: await page.eval(() => window.__probe.hulls()),
    kinship: await page.eval(() => window.__probe.kinshipRatio()),
    cohesion: await page.eval(() => window.__probe.cohesionRatio()),
    stray: await page.eval(() => window.__probe.folderMisplacement()),
  };
  settledGrain.cache.set(grain, out);
  return out;
}
settledGrain.cache = new Map();

async function ready(page, ms = 6000) {
  await page.eval(installProbeLib);
  await sleep(ms);
  await page.eval(installProbeLib); // survive the app's own re-render
  // …and survive a navigation that commits *after* that install. `goto` is
  // `Page.navigate`, which returns before the new document exists, so both
  // installs above can land in the outgoing one — leaving `window.__probe`
  // undefined and every check in the suite failing on a TypeError rather than
  // on anything it was written to measure. Seen once in ten runs on a loaded
  // machine, which is exactly often enough to be believed as a real failure.
  for (let i = 0; i < 20; i++) {
    const there = await page.eval(() => typeof window.__probe);
    if (there === 'object') break;
    await page.eval(installProbeLib);
    await sleep(250);
  }
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
        criterion: 'Canvas stats and mode bar never overlap',
        async run(page) {
          // `canvas-stats` and `mode-bar` share one flex strip along the
          // canvas floor, so at every width one has to give way rather than
          // be drawn over. (This read `stats-bar` until UI-109 — that name
          // followed StatsBar up into the toolbar, where it cannot collide
          // with anything down here and the check proved nothing.)
          const results = {};
          for (const w of [1600, 1440, 1280, 1152, 1024]) {
            await page.viewport(w, 800); await sleep(900);
            results[w] = await page.eval(() => window.__probe.overlap('canvas-stats', 'mode-bar'));
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
          const overlap = await page.eval(() => window.__probe.overlap('canvas-stats', 'mode-bar'));
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

  'ui-052': {
    ticket: 'UI-052', title: 'Folder cohesion force',
    async setup(page) {
      await page.viewport(1600, 1000);
      await page.goto(APP); await ready(page);
      await page.eval((n) => window.__probe.pickScope(n), 'ui');
      // See `clearDiff` / UI-061 — a dirty working tree otherwise hides every
      // node at File aggregation and this suite measures an empty canvas.
      await clearDiff(page);
      await waitSettled(page);
    },
    checks: [
      {
        id: 'canvas-has-nodes',
        criterion: 'The scope actually drew something to measure',
        async run(page) {
          const r = await page.eval(() => window.__probe.cohesionRatio());
          return ok(!!r?.ratio, r, r?.ratio ? '' : 'empty canvas — every later number here is meaningless');
        },
      },
      {
        id: 'control-present',
        criterion: 'A four-step cohesion control is in the sidebar, one step active',
        async run(page) {
          const segs = await page.eval(() => window.__probe.cohesionState());
          const active = segs.filter((s) => s.active);
          return ok(segs.length === 4 && active.length === 1, segs,
            segs.length ? '' : 'no cohesion control — is it inside the Graph Visualization block?');
        },
      },
      {
        id: 'strength-tightens-folders',
        criterion: 'Same-folder nodes sit closer at High than at Off',
        async run(page) {
          // Measured, not inferred from the control's state: the whole claim
          // of UI-052 is that the layout changes, and a pressed button proves
          // nothing about where the nodes went.
          await page.eval(() => window.__probe.setCohesion('off'));
          await waitSettled(page);
          const off = await page.eval(() => window.__probe.cohesionRatio());
          await page.eval(() => window.__probe.setCohesion('high'));
          await waitSettled(page);
          const high = await page.eval(() => window.__probe.cohesionRatio());
          if (!off?.ratio || !high?.ratio) return ok(false, { off, high }, 'no same-folder pairs to measure');
          const pass = high.ratio < off.ratio * 0.9;
          return ok(pass, { offRatio: +off.ratio.toFixed(3), highRatio: +high.ratio.toFixed(3), nodes: high.nodes },
            pass ? '' : 'cohesion did not visibly tighten folders');
        },
      },
      {
        id: 'grouping-hides-nothing',
        criterion: 'Changing strength moves nodes without removing any',
        async run(page) {
          await page.eval(() => window.__probe.setCohesion('off'));
          await sleep(3000);
          const before = await page.eval(() => window.__probe.renderedNodes());
          await page.eval(() => window.__probe.setCohesion('high'));
          await sleep(3000);
          const after = await page.eval(() => window.__probe.renderedNodes());
          return ok(before === after, { before, after },
            before === after ? '' : 'the node count moved — this is a layout control, not a filter');
        },
      },
      {
        id: 'restores-default',
        criterion: 'The suite leaves the persisted strength back at its default',
        async run(page) {
          // Clicking a segment persists it (settings.ts), so a suite that
          // ends on High hands High to whatever runs next. Suites already
          // leak enough state into each other without adding to it.
          await page.eval(() => window.__probe.setCohesion('low'));
          await sleep(500);
          const segs = await page.eval(() => window.__probe.cohesionState());
          const active = segs.find((s) => s.active);
          return ok(active?.label === 'Low', segs);
        },
      },
    ],
  },

  'ui-069': {
    ticket: 'UI-069', title: 'Cohesion follows the folder tree, not one folder',
    async setup(page) {
      await page.viewport(1600, 1000);
      settledGrouping.cache.clear();
      widestScope.chosen = null;
      // A first load purely so `__probe` exists to write the persisted level
      // with. Every measurement below then starts from its own reload.
      await page.goto(APP); await ready(page);
    },
    checks: [
      {
        id: 'both-populations-drawn',
        criterion: 'The scope drew related and unrelated folder pairs to compare',
        async run(page) {
          const { kinship } = await settledGrouping(page, 'low');
          return ok(!!kinship?.ratio, kinship,
            kinship?.ratio ? '' : `kinPairs=${kinship?.kinPairs} farPairs=${kinship?.farPairs} — this scope has no tree depth, so every number below is meaningless`);
        },
      },
      {
        id: 'subtrees-stay-together',
        criterion: 'Folders inside one subtree stay closer than folders sharing none',
        async run(page) {
          // Deliberately *not* an off-versus-high comparison, though that is
          // the obvious shape and was the first build. Raising the strength
          // also contracts every leaf folder into a ball, and a ball's members
          // cannot approach each other past the collision radius while
          // unrelated pairs go on contracting — so that comparison conflates
          // UI-052's effect with UI-069's and cannot attribute what it sees.
          //
          // The attributable measurement is nested against flat at a fixed
          // strength, made by building with `MAX_TIERS` at 4 and at 1. On this
          // repo's widest scope (219 files, 12,564 kin pairs, 10,198 far
          // pairs):
          //
          //            flat    nested
          //   low      0.502   0.484
          //   high     0.464   0.376
          //
          // The ancestor tiers are worth 19% of the ratio at High, and the
          // effect grows with strength, which is what a real force does. That
          // comparison needs a build flag, so it cannot live in a suite; the
          // control version of it is in `graph-grouping.test.ts` ("kinship
          // comes from the tree, not from the arithmetic").
          //
          // What is left for the browser is the standing property: a subtree
          // is not scattered. The threshold sits well above the measured 0.484
          // so a payload with weaker folder-call correlation still passes, and
          // well below 1.0 so a change that starts flinging subtrees apart
          // fails loudly.
          const k = (await settledGrouping(page, 'low')).kinship;
          if (!k?.ratio) return ok(false, k, 'no pairs to measure');
          return ok(k.ratio < 0.75, { ratio: +k.ratio.toFixed(3), kinPairs: k.kinPairs, farPairs: k.farPairs },
            k.ratio < 0.75 ? '' : 'subtrees are no more clustered than unrelated folders');
        },
      },
      {
        id: 'leaves-still-tighten',
        criterion: 'Nesting did not cost UI-052 its own property',
        async run(page) {
          // The ancestor pull takes a share of a strength that used to go
          // entirely to the leaf. If that share loosens the leaf folders,
          // UI-069 bought kinship by spending the thing UI-052 built and the
          // hulls depend on.
          const off = (await settledGrouping(page, 'off')).cohesion;
          const high = (await settledGrouping(page, 'high')).cohesion;
          if (!off?.ratio || !high?.ratio) return ok(false, { off, high }, 'no same-folder pairs to measure');
          const pass = high.ratio < off.ratio * 0.9;
          return ok(pass, { offRatio: +off.ratio.toFixed(3), highRatio: +high.ratio.toFixed(3) },
            pass ? '' : 'folders stopped tightening — the ancestor tiers took the leaf pull');
        },
      },
      {
        id: 'grouping-hides-nothing',
        criterion: 'Strength moves nodes without removing any',
        async run(page) {
          const off = (await settledGrouping(page, 'off')).kinship;
          const high = (await settledGrouping(page, 'high')).kinship;
          return ok(off.rendered === high.rendered, { off: off.rendered, high: high.rendered },
            off.rendered === high.rendered ? '' : 'the node count moved — this is a layout control, not a filter');
        },
      },
      {
        id: 'restores-default',
        criterion: 'The suite leaves the persisted strength back at its default',
        async run(page) {
          // The measurements above write the level to localStorage, so a
          // suite that ends on High hands High to whatever runs next. These
          // suites already leak enough state into each other.
          await page.eval(() => window.__probe.storeCohesion('low'));
          await page.goto(APP); await ready(page);
          const segs = await page.eval(() => window.__probe.cohesionState());
          const active = segs.find((s) => s.active);
          return ok(active?.label === 'Low', segs);
        },
      },
    ],
  },

  'ui-103': {
    ticket: 'UI-103', title: 'A file can be a region',
    async setup(page) {
      await page.viewport(1600, 1000);
      settledGrain.cache.clear();

      widestScope.chosen = null;
      await page.eval(() => window.__probe?.storeGrain('folder'));
      await page.goto(APP); await ready(page);
    },
    checks: [
      {
        id: 'grain-control-present',
        criterion: 'A two-step grain control sits with cohesion, one step active',
        async run(page) {
          const segs = await page.eval(() => window.__probe.groupGrainState());
          const active = segs.filter((s) => s.active);
          return ok(segs.length === 2 && active.length === 1,
            segs, segs.length === 2 ? '' : 'no grain control — is it inside the Graph Visualization block?');
        },
      },
      {
        id: 'file-grain-draws-file-regions',
        criterion: 'Choosing Files outlines files, not the folders holding them',
        async run(page) {
          // The feature, stated as the thing a reader would look at. A region
          // path with a dot in its last segment is a file; the folder grain
          // can never produce one, so this cannot pass by accident.
          const m = await settledGrain(page, 'file');
          const files = m.hulls.filter((h) => h.d && h.path && /\.[a-z]+$/i.test(h.path));
          return ok(files.length > 0,
            { drawn: m.hulls.filter((h) => h.d).length, fileRegions: files.length, sample: files.slice(0, 4).map((h) => h.path) },
            files.length > 0 ? '' : 'no file earned an outline — MIN_HULL_MEMBERS or the foreign-share guard is rejecting all of them');
        },
      },
      {
        id: 'folder-grain-draws-no-file-region',
        criterion: 'The folder grain is not leaked into by the file grain',
        async run(page) {
          // The half of UI-103 that must not move, stated as the one thing
          // about it that is stable here.
          //
          // Deliberately NOT a misplacement threshold. The grain only means
          // anything at Entity level, and Entity level on this scope is far
          // over `RENDER_BUDGET`, so the canvas draws a ranked subset and
          // which folders keep the three drawn members an outline needs
          // varies run to run — measured that way the folder grain scored 17
          // regions once and 0 the next time, on unchanged code. UI-102's
          // suite already holds the misplacement threshold, at File level
          // where the whole scope is drawn. What is invariant regardless of
          // which nodes survive the ceiling is that no region here is a file.
          const m = await settledGrain(page, 'folder');
          const files = m.hulls.filter((h) => h.d && h.path && /\.[a-z]+$/i.test(h.path));
          return ok(files.length === 0,
            { drawn: m.hulls.filter((h) => h.d).length, fileRegions: files.map((h) => h.path) },
            files.length === 0 ? '' : 'a file region was drawn at folder grain');
        },
      },
      {
        id: 'the-grain-actually-changes-the-picture',
        criterion: 'The two grains do not settle the same canvas',
        async run(page) {
          // The check the first version of this suite needed and did not
          // have. Both grains reported identical numbers to three decimals
          // because nothing forced Entity level, so `groupGrainFor` collapsed
          // file to folder and the suite measured one thing twice while
          // reporting a pass. Comparing the two settles is what makes that
          // failure mode loud.
          //
          // No threshold on the direction or size of the difference: file
          // grain re-bases `tierWeightFor`'s depth-0 exemption from the
          // folder onto the file, so folder cohesion legitimately loosens and
          // there is no number it must hit. The numbers are reported so the
          // ticket can record them.
          const folder = await settledGrain(page, 'folder');
          const file = await settledGrain(page, 'file');
          const num = (m) => ({
            cohesion: m.cohesion?.ratio ?? null,
            misplaced: m.stray?.share ?? null,
            regions: m.hulls.filter((h) => h.d).length,
          });
          const a = num(folder);
          const b = num(file);
          const differs = a.cohesion !== b.cohesion || a.regions !== b.regions;
          return ok(differs, { folder: a, file: b },
            differs ? '' : 'both grains settled identically — is Entity level actually being set?');
        },
      },
    ],
  },

  'ui-102': {
    ticket: 'UI-102', title: 'A crossing edge no longer tows a node out of its folder',
    async setup(page) {
      await page.viewport(1600, 1000);
      settledGrouping.cache.clear();
      widestScope.chosen = null;
      await page.goto(APP); await ready(page);
    },
    checks: [
      {
        id: 'regions-to-be-misplaced-between',
        criterion: 'The scope drew at least two folders big enough to have outlines',
        async run(page) {
          // Below two regions nothing can be nearer to the wrong one, and the
          // share would read 0 for want of anywhere to go rather than because
          // the layout is right.
          const m = (await settledGrouping(page, 'low')).stray;
          return ok(m?.share !== null && m?.regions >= 2, m,
            m?.share !== null ? '' : `regions=${m?.regions} — nothing here can be measured`);
        },
      },
      {
        id: 'nodes-sit-in-their-own-region',
        criterion: 'Few nodes sit nearer another folder\'s centre than their own',
        async run(page) {
          // Measured at `low`, the shipped default, because that is the
          // setting the report came from and the one where the folder force
          // has least to spend on the argument.
          //
          // The threshold is an absolute standing property, not a
          // before/after: the fix has no toggle, so a comparison needs two
          // builds and cannot live in a suite. The controlled version is in
          // `graph-grouping.test.ts`, which settles the same forces over a
          // built world with and without the folder term. What the two builds
          // measured here is recorded on the ticket.
          const m = (await settledGrouping(page, 'low')).stray;
          if (m?.share === null) return ok(false, m, 'nothing to measure');
          return ok(m.share < 0.25,
            { share: +m.share.toFixed(3), misplaced: m.misplaced, measured: m.measured, regions: m.regions },
            m.share < 0.25 ? '' : 'a quarter of the drawn nodes sit closer to a folder they are not in');
        },
      },
      {
        id: 'a-crossing-edge-is-not-what-misplaces-a-node',
        criterion: 'Nodes with an edge out of their folder are misplaced no more often than nodes without one',
        async run(page) {
          // The report, as a comparison the layout either earns or does not.
          // A folder-blind link force makes exactly this split: the nodes it
          // can tow are the nodes with somewhere to be towed to, so their
          // share runs ahead of the rest. Reported with both populations, so
          // a pass on a scope where one of them is three nodes is visible as
          // such rather than as a result.
          const m = (await settledGrouping(page, 'low')).stray;
          if (m?.crossingShare === null || m?.internalShare === null) {
            return ok(false, m, 'one population is empty — this scope cannot answer the question');
          }
          const gap = m.crossingShare - m.internalShare;
          return ok(gap < 0.15, {
            crossing: +m.crossingShare.toFixed(3), internal: +m.internalShare.toFixed(3), gap: +gap.toFixed(3),
            crossingNodes: m.crossingNodes, internalNodes: m.internalNodes,
          }, gap < 0.15 ? '' : 'an edge out of the folder is still what decides whether a node stays in it');
        },
      },
      {
        id: 'folders-were-not-loosened-to-buy-it',
        criterion: 'Same-folder nodes are no less clustered than UI-052 left them',
        async run(page) {
          // The trade this change could silently make. Lengthening crossing
          // edges spreads the canvas, and misplacement improves for free if
          // every folder grew apart from every other. `cohesionRatio` is
          // scale-invariant, so holding it is what says the strays came home
          // rather than the canvas moving out from under the measurement.
          const c = (await settledGrouping(page, 'low')).cohesion;
          if (!c?.ratio) return ok(false, c, 'no same-folder pairs to measure');
          return ok(c.ratio < 0.75, { ratio: +c.ratio.toFixed(3), intraPairs: c.intraPairs },
            c.ratio < 0.75 ? '' : 'folders are no tighter than chance — the link change cost UI-052 its property');
        },
      },
      {
        id: 'subtrees-still-cohere',
        criterion: 'The tree signal UI-069 added survived the link change',
        async run(page) {
          // Weakening crossing edges weakens exactly the edges that used to
          // hold two sibling folders near each other. If kinship is what paid
          // for the misplacement number, this catches it — and it is the
          // reason the decay constant in `linkFolderWeights` is the same one
          // the cohesion force uses rather than something harsher.
          const k = (await settledGrouping(page, 'low')).kinship;
          if (!k?.ratio) return ok(false, k, 'this scope has no tree depth');
          return ok(k.ratio < 0.75, { ratio: +k.ratio.toFixed(3), kinPairs: k.kinPairs, farPairs: k.farPairs },
            k.ratio < 0.75 ? '' : 'subtrees scattered — the crossing edges that held them were weakened too far');
        },
      },
      {
        id: 'draws-what-it-drew',
        criterion: 'A folder-aware link force is a layout change, not a filter',
        async run(page) {
          const g = await settledGrouping(page, 'low');
          return ok(g.stray.rendered === g.cohesion.rendered && g.stray.rendered > 0,
            { stray: g.stray.rendered, cohesion: g.cohesion.rendered },
            g.stray.rendered > 0 ? '' : 'nothing rendered');
        },
      },
    ],
  },

  'ui-053': {
    ticket: 'UI-053', title: 'Folder-seeded initial positions',
    async setup(page) {
      await page.viewport(1600, 1000);
      // Cohesion off for the whole suite. With the force running, any
      // clustering could come from the seed or from the force, and this
      // suite is only about the seed.
      await page.goto(APP); await ready(page);
      await page.eval(() => window.__probe.storeCohesion('off'));
      await page.goto(APP); await ready(page);
      // Before the scope pick, deliberately. The simulation runs whether or
      // not the nodes are displayed, so clearing the diff *after* the graph
      // is built would hand us an already-settled layout and there would be
      // no first frames left to measure. See UI-061.
      await clearDiff(page);
    },
    checks: [
      {
        id: 'structure-before-settling',
        criterion: 'Folders are already grouped in the first frames, with the force off',
        async run(page) {
          await page.eval((n) => window.__probe.pickScope(n), 'ui');
          // Measure the moment nodes exist rather than at a fixed delay: the
          // publish takes as long as it takes, and a fixed sleep either
          // measures an empty canvas or a settled one depending on the day.
          let seen = 0;
          for (let i = 0; i < 60; i++) {
            seen = await page.eval(() => window.__probe.nodePositions().length);
            if (seen > 0) break;
            await sleep(200);
          }
          const early = await page.eval(() => window.__probe.cohesionRatio());
          if (!early?.ratio) return ok(false, early, 'no same-folder pairs to measure');
          const pass = early.ratio < 0.85;
          return ok(pass, { ratio: +early.ratio.toFixed(3), nodes: early.nodes },
            pass ? '' : 'the seed carried no folder structure — did nodes start on the index spiral?');
        },
      },
      {
        id: 'stable-across-reloads',
        criterion: 'The same scope settles to the same picture twice',
        async run(page) {
          // Note: this passes on pre-UI-053 builds too — identical data gives
          // an identical phyllotaxis spiral. It guards against the seed
          // introducing instability; the improvement it was written for
          // (invariance to node array order) is asserted in the unit suite.
          // Both samples are taken from a stopped simulation, not from a
          // fixed delay — see `waitSettled`.
          const settleA = await waitSettled(page);
          const first = await page.eval(() => window.__probe.nodePositions());
          await page.goto(APP); await ready(page);
          await clearDiff(page);
          await page.eval((n) => window.__probe.pickScope(n), 'ui');
          const settleB = await waitSettled(page);
          const second = await page.eval(() => window.__probe.nodePositions());
          if (settleA.settledMs < 0 || settleB.settledMs < 0) {
            return ok(false, { settleA, settleB }, 'the simulation never came to rest — cannot compare');
          }
          if (!first.length || !second.length) {
            return ok(false, { first: first.length, second: second.length }, 'a canvas was empty — nothing to compare');
          }

          const byId = new Map(second.map((n) => [n.id, n]));
          let compared = 0, moved = 0, worst = 0;
          for (const a of first) {
            const b = byId.get(a.id);
            if (!b) continue;
            compared++;
            const d = Math.hypot(a.x - b.x, a.y - b.y);
            if (d > worst) worst = d;
            if (d > 40) moved++;
          }
          const pass = compared > 0 && moved / compared < 0.1;
          return ok(pass, { compared, moved, worst: Math.round(worst), settleA, settleB },
            pass ? '' : 'the same scope drew a different picture on reload');
        },
      },
      {
        id: 'restores-default',
        criterion: 'The suite leaves the persisted strength back at its default',
        async run(page) {
          const restored = await page.eval(() => window.__probe.storeCohesion('low'));
          return ok(restored === true, restored);
        },
      },
    ],
  },

  'ui-070': {
    ticket: 'UI-070', title: 'Regions at more than one tier',
    async setup(page) {
      await page.viewport(1600, 1000);
      await page.goto(APP); await ready(page);
      // The widest scope, for the reason ui-069 gives: tiers need folders
      // nested inside the scope, and which top-level row offers that is a
      // property of the payload rather than a name worth hard-coding.
      const scope = await widestScope(page);
      const scoped = await page.eval((p) => window.__probe.scopeRowState(p), scope);
      if (!scoped?.checked) await page.eval((p) => window.__probe.toggleScopePath(p), scope);
      await clearDiff(page);
      // High cohesion for the same reason ui-055 uses it: zero outlines is a
      // legitimate outcome of a layout whose groups are not separable, so
      // asserting on the default setting would test the dataset.
      await page.eval(() => window.__probe.setCohesion('high'));
      await page.eval(() => window.__probe.setHulls(true));
      await waitSettled(page);
    },
    checks: [
      {
        id: 'depth-control-present',
        criterion: 'A tier control sits with the outline toggle, one step active',
        async run(page) {
          const segs = await page.eval(() => window.__probe.hullDepthState());
          const active = segs.filter((s) => s.active);
          return ok(segs.length >= 2 && active.length === 1, segs,
            segs.length ? '' : 'no tier control — is it inside the Graph Visualization block?');
        },
      },
      {
        id: 'one-tier-draws-only-real-folders',
        criterion: 'At one tier every region is a folder that holds a drawn node',
        async run(page) {
          // What "one tier" claims, stated exactly. Not "no region contains
          // another": a folder holding both files and a drawn subfolder is a
          // parent at any setting, because it genuinely is one — `ui/src` has
          // loose files *and* `ui/src/stores` under it. The tier count is how
          // far *above* a node's own folder an outline may sit, so at one
          // tier no region may be a pure ancestor.
          await page.eval(() => window.__probe.setHullDepth(1));
          await sleep(1500);
          const [hulls, nodes] = await Promise.all([
            page.eval(() => window.__probe.hulls()),
            page.eval(() => window.__probe.nodePositions()),
          ]);
          const held = new Set(nodes.map((n) => n.folder));
          const drawn = hulls.filter((h) => h.d && h.path !== null);
          const invented = drawn.filter((h) => !held.has(h.path)).map((h) => h.path);
          return ok(invented.length === 0, { drawn: drawn.length, invented },
            invented.length === 0 ? '' : `${invented.join(', ')} hold no drawn file of their own`);
        },
      },
      {
        id: 'a-tier-adds-regions',
        criterion: 'Raising the tier count outlines the subtrees around the folders',
        async run(page) {
          await page.eval(() => window.__probe.setHullDepth(1));
          await sleep(1500);
          const one = await page.eval(() => window.__probe.hulls());
          await page.eval(() => window.__probe.setHullDepth(2));
          await sleep(1500);
          const two = await page.eval(() => window.__probe.hulls());
          const parents = two.filter((h) => h.d && h.parent);
          // Every region drawn at one tier must still be drawn at two: a tier
          // is an addition, never a replacement.
          const kept = one.filter((h) => h.d).every((h) => two.some((t) => t.path === h.path && t.d));
          const pass = parents.length > 0 && kept;
          return ok(pass, { atOne: one.filter((h) => h.d).length, atTwo: two.filter((h) => h.d).length, parents: parents.map((p) => p.path) },
            parents.length === 0 ? 'no subtree got an outline — is the foreign-share test still rejecting ancestors?' : (kept ? '' : 'a leaf region disappeared when the tier was added'));
        },
      },
      {
        id: 'a-parent-contains-its-children',
        criterion: 'A parent region is mostly its own subtree, not a lasso',
        async run(page) {
          // The UI-055 guard, retested one tier up. A descendant is not
          // foreign — that is the whole change — so a parent that fails this
          // is enclosing other subtrees rather than its own.
          const purity = await page.eval(() => window.__probe.hullPurity());
          const parents = purity.filter((p) => p.share !== null && p.path && p.path.indexOf('/') === -1);
          const worst = parents.reduce((w, p) => (w === null || p.share > w.share ? p : w), null);
          const pass = parents.length === 0 || worst.share <= 0.35;
          return ok(pass, { parents: parents.map((p) => ({ path: p.path, own: p.own, foreign: p.foreign, share: +p.share.toFixed(2) })) },
            pass ? '' : `${worst.path} is ${Math.round(worst.share * 100)}% other subtrees`);
        },
      },
      {
        id: 'region-names-stay-readable',
        criterion: 'No two region names are drawn on top of each other',
        async run(page) {
          // The whole point of a region is that it is *named*; two names
          // stacked on one another name nothing. Nesting is what makes this
          // structural rather than unlucky — a parent's outline touches its
          // child's along the top, so their names are anchored within a few
          // pixels of each other by construction, not by accident.
          //
          // Run at both grains (UI-103). File grain multiplies the label
          // count — every drawn file with `MIN_HULL_MEMBERS` entities earns a
          // name — so `separateLabels` is under far more pressure there, and
          // measuring only the folder case would leave the harder half
          // untested. Folder grain is restored at the end: the suites share a
          // page, and a grain left set would silently re-run every check
          // after this one against a different picture.
          await page.eval(() => window.__probe.setHullDepth(3));
          await sleep(1500);
          const byGrain = {};
          for (const grain of ['folder', 'file']) {
            await page.eval((g) => window.__probe.setGroupGrain(g), grain);
            await sleep(1500);
            byGrain[grain] = await page.eval(() => window.__probe.hullLabelBoxes());
          }
          await page.eval(() => window.__probe.setGroupGrain('folder'));
          // Longer than the per-grain waits above because restoring the grain
          // re-runs the force's `initialize` and restarts at alpha 0.5 — a
          // full re-settle, not an overlay redraw — and the next check in
          // this suite reads the hulls this leaves behind.
          await sleep(3000);
          const collisions = [...byGrain.folder.collisions, ...byGrain.file.collisions];
          const counts = { folderNames: byGrain.folder.labels.length, fileNames: byGrain.file.labels.length };
          return ok(collisions.length === 0, { ...counts, collisions },
            collisions.length === 0 ? ''
              : `${collisions.length} pair(s) overlap, worst ${collisions.reduce((w, c) => Math.max(w, c.overlapX), 0)}px across`);
        },
      },
      {
        id: 'parents-read-as-headings',
        criterion: 'A parent is outline-only, and its name is the larger one',
        async run(page) {
          const hulls = await page.eval(() => window.__probe.hulls());
          const parents = hulls.filter((h) => h.d && h.parent);
          const leaves = hulls.filter((h) => h.d && !h.parent);
          if (!parents.length || !leaves.length) return ok(false, { parents: parents.length, leaves: leaves.length }, 'no nesting drawn to compare');
          // No wash on a parent: three nested fills stack into a tint over
          // the innermost nodes and start shifting what their metric colours
          // appear to say.
          const unfilled = parents.every((p) => p.fillOpacity === 0);
          const bigger = parseFloat(parents[0].fontSize) > parseFloat(leaves[0].fontSize);
          return ok(unfilled && bigger,
            { parentFill: parents[0].fillOpacity, leafFill: leaves[0].fillOpacity, parentFont: parents[0].fontSize, leafFont: leaves[0].fontSize },
            unfilled ? (bigger ? '' : 'a parent name is not set apart from its children') : 'a parent region is filled, and the washes stack');
        },
      },
      {
        id: 'painted-outside-in',
        criterion: 'A parent is painted before the regions inside it',
        async run(page) {
          const hulls = await page.eval(() => window.__probe.hulls());
          const drawn = hulls.filter((h) => h.d);
          let violation = null;
          drawn.forEach((h, i) => {
            if (!h.parent || !h.path) return;
            const child = drawn.findIndex((c, j) => j < i && c.path && c.path.startsWith(`${h.path}/`));
            if (child !== -1) violation = { parent: h.path, buriedBy: drawn[child].path };
          });
          return ok(violation === null, { drawn: drawn.length, violation },
            violation ? `${violation.parent} was painted over ${violation.buriedBy}` : '');
        },
      },
      {
        id: 'tiers-hide-nothing',
        criterion: 'Adding a tier draws no fewer nodes',
        async run(page) {
          await page.eval(() => window.__probe.setHullDepth(1));
          await sleep(1200);
          const before = await page.eval(() => window.__probe.renderedNodes());
          await page.eval(() => window.__probe.setHullDepth(3));
          await sleep(1200);
          const after = await page.eval(() => window.__probe.renderedNodes());
          return ok(before === after, { before, after },
            before === after ? '' : 'the node count moved — regions are an overlay, not a filter');
        },
      },
      {
        id: 'restores-default',
        criterion: 'The suite leaves the persisted tier count back at its default',
        async run(page) {
          await page.eval(() => window.__probe.setHullDepth(2));
          await sleep(500);
          const segs = await page.eval(() => window.__probe.hullDepthState());
          return ok(segs.find((s) => s.active)?.depth === 2, segs);
        },
      },
    ],
  },

  'ui-071': {
    ticket: 'UI-071/UI-089', title: 'Which region am I in, and focusing it',
    async setup(page) {
      await page.viewport(1600, 1000);
      await page.goto(APP); await ready(page);
      // Scope, then *confirm* it took. A toggle sent while the index is still
      // arriving finds no row and silently does nothing, and every check then
      // fails with "no region to stand in" — which reads as a broken feature
      // and is an empty canvas. Retried a few times rather than assumed.
      const scope = await widestScope(page);
      for (let i = 0; i < 6; i++) {
        const state = await page.eval((p) => window.__probe.scopeRowState(p), scope);
        if (!state?.checked) await page.eval((p) => window.__probe.toggleScopePath(p), scope);
        await sleep(1500);
        if (await page.eval(() => window.__probe.renderedNodes()) > 0) break;
      }
      await clearDiff(page);
      // High cohesion and two tiers for the same reason ui-070 uses them: a
      // nested trail is the thing under test, and zero separable regions is a
      // legitimate outcome of the default setting.
      await page.eval(() => window.__probe.setCohesion('high'));
      await page.eval(() => window.__probe.setHulls(true));
      await page.eval(() => window.__probe.setHullDepth(2));
      await waitSettled(page);
    },
    checks: [
      {
        id: 'card-names-the-region',
        criterion: 'Standing in a region raises a card naming it',
        async run(page) {
          const p = await page.eval(() => window.__probe.regionProbePoint());
          if (!p) return ok(false, null, 'no region to stand in — did any outline get drawn?');
          const card = await page.eval(([x, y]) => window.__probe.hoverPoint(x, y), [p.x, p.y]);
          const named = card?.rows?.some((r) => r.path === p.tightest);
          return ok(!!named, { point: p, card },
            card ? (named ? '' : 'the card names a different region than the one under the pointer') : 'no card appeared inside a region');
        },
      },
      {
        id: 'card-lists-every-enclosing-region',
        criterion: 'The card lists the whole enclosing stack, widest first',
        async run(page) {
          // The plural is the point: with tiers the pointer is inside several
          // regions at once, and one name hides the more useful half of the
          // answer.
          const p = await page.eval(() => window.__probe.regionProbePoint());
          if (!p) return ok(false, null, 'no region to stand in');
          const card = await page.eval(([x, y]) => window.__probe.hoverPoint(x, y), [p.x, p.y]);
          const rows = card?.rows?.map((r) => r.path) ?? [];
          const same = rows.length === p.paths.length && rows.every((r, i) => r === p.paths[i]);
          return ok(same, { shape: p.paths, card: rows },
            same ? '' : 'the card and the shapes under the pointer disagree about where you are');
        },
      },
      {
        id: 'card-says-what-the-relationships-do',
        criterion: 'The card says how much of the region\'s coupling stays inside it',
        async run(page) {
          // The question the outline exists to raise and never answered: is
          // this a subsystem, or a directory someone filed things in?
          const p = await page.eval(() => window.__probe.regionProbePoint());
          if (!p) return ok(false, null, 'no region to stand in');
          const card = await page.eval(([x, y]) => window.__probe.hoverPoint(x, y), [p.x, p.y]);
          const line = card?.traffic ?? '';
          const label = (p.tightest || '').split('/').pop();
          const speaks = /relationships/.test(line) && (!label || line.includes(label));
          return ok(speaks, { traffic: line, tightest: p.tightest },
            speaks ? '' : 'the card still says nothing about what the region is coupled to');
        },
      },
      {
        id: 'the-sentence-does-not-impersonate-cohesion',
        criterion: 'It states counts, not a percentage the Quality panel would contradict',
        async run(page) {
          // Two numbers under one name is worse than one: the Quality panel
          // measures every dependency edge in the repo, this measures what is
          // drawn, and they legitimately disagree.
          const p = await page.eval(() => window.__probe.regionProbePoint());
          if (!p) return ok(false, null, 'no region to stand in');
          const card = await page.eval(([x, y]) => window.__probe.hoverPoint(x, y), [p.x, p.y]);
          const line = card?.traffic ?? '';
          const clean = line !== '' && !/%/.test(line) && !/cohesion/i.test(line);
          return ok(clean, line,
            clean ? '' : 'the card is quoting a ratio that the Quality panel measures differently');
        },
      },
      {
        id: 'tightest-region-is-the-focus-target',
        criterion: 'The last row is the tightest region, and the hint names it',
        async run(page) {
          const p = await page.eval(() => window.__probe.regionProbePoint());
          if (!p) return ok(false, null, 'no region to stand in');
          const card = await page.eval(([x, y]) => window.__probe.hoverPoint(x, y), [p.x, p.y]);
          const rows = card?.rows ?? [];
          const last = rows[rows.length - 1];
          const marked = rows.filter((r) => r.tightest);
          const pass = !!last && last.path === p.tightest && marked.length === 1
            && marked[0].path === p.tightest && !!card.path && !!card.hint;
          return ok(pass, { card, tightest: p.tightest },
            pass ? '' : 'the row a double-click would act on is not the one the card emphasises');
        },
      },
      {
        id: 'card-is-inert',
        criterion: 'The card cannot intercept the gestures it describes',
        async run(page) {
          // It tracks the cursor, so anything it caught would be aimed at the
          // region or the node underneath it — including the double-click it
          // is advertising.
          const p = await page.eval(() => window.__probe.regionProbePoint());
          if (!p) return ok(false, null, 'no region to stand in');
          const card = await page.eval(([x, y]) => window.__probe.hoverPoint(x, y), [p.x, p.y]);
          return ok(card?.pointerEvents === 'none', { pointerEvents: card?.pointerEvents });
        },
      },
      {
        id: 'card-stays-inside-the-canvas',
        criterion: 'Near an edge the card flips rather than being clipped',
        async run(page) {
          const p = await page.eval(() => window.__probe.regionProbePoint());
          if (!p) return ok(false, null, 'no region to stand in');
          // Two moves: the flip is computed from the box measured on the
          // previous frame, so a card that has never been shown has no size
          // to reason about yet.
          await page.eval(([x, y]) => window.__probe.hoverPoint(x, y), [p.x, p.y]);
          const card = await page.eval(([x, y]) => window.__probe.hoverPoint(x, y), [p.x, p.y]);
          const view = await page.eval(() => {
            const r = document.querySelector('.graph-container').getBoundingClientRect();
            return { left: r.left, top: r.top, right: r.right, bottom: r.bottom };
          });
          const b = card?.box;
          const inside = !!b && b.left >= view.left - 1 && b.right <= view.right + 1
            && b.top >= view.top - 1 && b.bottom <= view.bottom + 1;
          return ok(inside, { box: b, view },
            inside ? '' : 'the card hangs off the canvas');
        },
      },
      {
        id: 'no-region-no-card',
        criterion: 'Off every region the card goes away rather than guessing',
        async run(page) {
          // Two different nothings, told apart: a canvas with no regions on
          // it, and a canvas so covered in them that no sample lands outside.
          // The old note claimed the second whichever it was, which sent the
          // last debugging session looking for a hull bug that was an empty
          // canvas.
          const drawn = await page.eval(() => document.querySelectorAll('g.folder-hull').length);
          const out = await page.eval(() => window.__probe.pointOutsideRegions());
          if (!out) return ok(false, { regions: drawn }, drawn === 0
            ? 'no regions on the canvas at all — the scope or the outline toggle did not take'
            : 'every sampled point was inside a region');
          const card = await page.eval(([x, y]) => window.__probe.hoverPoint(x, y), [out.x, out.y]);
          return ok(card === null, { point: out, card },
            card === null ? '' : 'the card named a region the pointer is not in');
        },
      },
      {
        id: 'leaving-the-canvas-clears-the-card',
        criterion: 'The card does not outlive the pointer',
        async run(page) {
          const p = await page.eval(() => window.__probe.regionProbePoint());
          if (!p) return ok(false, null, 'no region to stand in');
          await page.eval(([x, y]) => window.__probe.hoverPoint(x, y), [p.x, p.y]);
          const gone = await page.eval(() => window.__probe.leaveCanvas());
          return ok(gone === true, { gone });
        },
      },
      {
        id: 'a-claimed-folder-carries-its-spec-entity',
        criterion: 'A folder a `cr:` claims shows the entity and its description',
        async run(page) {
          // Swept rather than asserted on one region: which folders a spec
          // claims is a property of the repo under test. Every region is
          // required to say *something* — an entity or "no spec entity claims
          // this folder" — and at least one must find a claim, or the join is
          // not running at all.
          const points = await page.eval(() => window.__probe.regionProbePoints());
          if (!points.length) return ok(false, null, 'no regions drawn to hover');
          const cards = [];
          for (const p of points) {
            cards.push(await page.eval(([x, y]) => window.__probe.hoverPoint(x, y), [p.x, p.y]));
          }
          const said = cards.filter((c) => c && (c.spec || c.unclaimed));
          const claimed = cards.filter((c) => c?.spec?.head);
          const pass = said.length === cards.length && claimed.length > 0;
          return ok(pass, {
            regions: cards.length,
            claimed: claimed.map((c) => ({ path: c.path, head: c.spec.head, via: c.spec.via, desc: !!c.spec.desc })),
            silent: cards.length - said.length,
          }, pass
            ? ''
            : (claimed.length === 0
              ? 'no region found a claiming entity — is the spec graph loaded, and does this repo`s spec name folders?'
              : 'a region said nothing at all about the spec, neither a claim nor its absence'));
        },
      },
      {
        id: 'an-inherited-claim-says-whose-words-they-are',
        criterion: 'A claim from a folder above is labelled with the path it describes',
        async run(page) {
          // `cr: "ui/"` genuinely covers `ui/src/stores`, but its words are
          // about `ui`. Passing them off as the subfolder's own description
          // would put words in the author's mouth.
          const points = await page.eval(() => window.__probe.regionProbePoints());
          const seen = [];
          for (const p of points) {
            const card = await page.eval(([x, y]) => window.__probe.hoverPoint(x, y), [p.x, p.y]);
            if (card?.spec) seen.push({ path: card.path, via: card.spec.via });
          }
          if (!seen.length) return ok(false, null, 'no claimed region to inspect');
          // Every claim is either exact — no `via` line — or labelled with a
          // path that is a proper ancestor of the folder being described.
          const wrong = seen.filter((s) => s.via && !s.path.startsWith(`${s.via.replace(/^claims /, '')}/`));
          return ok(wrong.length === 0, { seen, wrong },
            wrong.length === 0 ? '' : 'an inherited claim names a path that does not contain the folder');
        },
      },
      {
        id: 'a-paragraph-does-not-become-a-wall',
        criterion: 'A spec description leaves the card a card',
        async run(page) {
          // `d:` is one string by grammar and a paragraph in practice — this
          // repo's longest runs past 300 words. The clamp is the reason the
          // join could ship on a hover surface at all.
          const points = await page.eval(() => window.__probe.regionProbePoints());
          let worst = null;
          for (const p of points) {
            const card = await page.eval(([x, y]) => window.__probe.hoverPoint(x, y), [p.x, p.y]);
            if (!card) continue;
            const h = card.box.bottom - card.box.top;
            if (worst && h <= worst.h) continue;
            // Captured here, while this card is the one on screen. Measuring
            // after the loop reports whichever card was hovered last, which
            // is how the first version of this diagnostic sent the reader
            // after an innocent 86px card.
            const parts = await page.eval(() => {
              const c = document.querySelector('[data-probe="region-card"]');
              return c === null ? null : [...c.children].map((e) => ({
                part: e.getAttribute('data-probe') ?? (e.getAttribute('class') ?? '').split(' ')[0],
                h: Math.round(e.getBoundingClientRect().height),
              }));
            });
            worst = { h, path: card.path, desc: card.spec?.desc?.length ?? 0, parts };
          }
          if (!worst) return ok(false, null, 'no card measured');
          // 215, raised from 200 when the card gained the traffic sentence
          // (UI-071's second half). Deliberate, not a concession: the budget
          // is protecting against a card that reads as a panel, and every
          // cheaper saving was worse — eliding the trail would reverse
          // UI-089's decision that the card names the *whole* enclosing
          // stack, and the description is already CSS line-clamped to two
          // lines and cannot give anything back. The card also went from
          // 260px to 300px wide, which paid for most of the new line.
          const CEILING = 215;
          return ok(worst.h <= CEILING, worst,
            worst.h <= CEILING ? '' : `the card grew to ${Math.round(worst.h)}px — something on it is not paying its way`);
        },
      },
      {
        id: 'a-region-click-still-deselects',
        criterion: 'Clicking a region with no node under the pointer deselects',
        async run(page) {
          // The regression the whole feature was skipped for the first time
          // round: deselect used to fire only when the click landed on the
          // <svg> itself, so a hittable region would have swallowed it
          // everywhere a hull reaches.
          // Select first and let the canvas settle: selecting opens the
          // details column, which narrows the canvas and restarts the layout,
          // so a point picked beforehand aims at where the region used to be.
          const selected = await page.eval(() => window.__probe.selectFirstNode());
          await waitSettled(page);
          const p = await page.eval(() => window.__probe.regionProbePoint());
          if (!p) return ok(false, { selected }, 'no region to click');
          const after = await page.eval(([x, y]) => window.__probe.clickPoint(x, y), [p.x, p.y]);
          const pass = selected > 0 && after?.on?.includes('hull-shape') && after.selected === 0;
          return ok(pass, { selected, after },
            pass ? '' : 'a click on a region no longer counts as a click on nothing');
        },
      },
      {
        id: 'double-click-focuses-the-region',
        criterion: 'Double-clicking a region narrows the view to it',
        async run(page) {
          const p = await page.eval(() => window.__probe.regionProbePoint());
          if (!p) return ok(false, null, 'no region to focus');
          const before = await page.eval(() => window.__probe.drawnFolders());
          const on = await page.eval(([x, y]) => window.__probe.dblclickPoint(x, y), [p.x, p.y]);
          await waitSettled(page);
          // Poll for the narrowed view rather than sampling once. Focusing is
          // a scope change, a re-fetch and a re-level, and `waitSettled` only
          // proves the *positions* stopped moving — which they also do while
          // the old picture is still on screen waiting for the new one. Asserts
          // on the transition, not on a moment.
          const outsideOf = (fs) => fs.filter((f) => f !== p.tightest && !f.startsWith(`${p.tightest}/`));
          let after = await page.eval(() => window.__probe.drawnFolders());
          for (let i = 0; i < 10 && (after.length === 0 || outsideOf(after).length > 0); i++) {
            await sleep(600);
            after = await page.eval(() => window.__probe.drawnFolders());
          }
          const outside = outsideOf(after);
          // Not a node count: focusing re-enables auto-level, so a small
          // region legitimately opens at entity level and draws *more* nodes
          // than the file-level view it came from. Narrower means narrower in
          // what the view is *about*, which is the folder test below.
          const widerBefore = before.some((f) => f !== p.tightest && !f.startsWith(`${p.tightest}/`));
          const pass = after.length > 0 && outside.length === 0 && widerBefore;
          return ok(pass, { on, focused: p.tightest, before: before.length, after: after.length, outside: [...new Set(outside)].slice(0, 8) },
            pass ? '' : (widerBefore ? 'the view still holds folders from outside the region that was focused' : 'the view was already only that region — nothing was narrowed'));
        },
      },
    ],
  },

  'ui-055': {
    ticket: 'UI-055', title: 'Named hulls behind folder groups',
    async setup(page) {
      await page.viewport(1600, 1000);
      await page.goto(APP); await ready(page);
      await page.eval((n) => window.__probe.pickScope(n), 'ui');
      await clearDiff(page);
      // High cohesion for the checks that need a region to exist. Zero
      // outlines is a *legitimate* outcome of this design — a group that
      // isn't spatially separable earns none — so asserting "at least one"
      // at whatever the default happens to yield tests the dataset, not the
      // feature. High is where separability is the expected outcome.
      await page.eval(() => window.__probe.setCohesion('high'));
      await waitSettled(page);
    },
    checks: [
      {
        id: 'regions-are-outlined',
        criterion: 'Separable folder groups get an outline',
        async run(page) {
          await page.eval(() => window.__probe.setHulls(true));
          await sleep(1200);
          const hulls = await page.eval(() => window.__probe.hulls());
          const drawn = hulls.filter((h) => h.d);
          // Not a fixed count. How many regions exist is a property of the
          // layout, and the whole design is that a group which is not
          // spatially separable gets no outline rather than a bad one.
          return ok(drawn.length >= 1, { hulls: hulls.length, drawn: drawn.length },
            drawn.length >= 1 ? '' : 'no outlines at all — is the hull layer being cleared?');
        },
      },
      {
        id: 'no-outline-is-a-lasso',
        criterion: 'Every outline is mostly its own folder, not a lasso round the middle',
        async run(page) {
          const purity = await page.eval(() => window.__probe.hullPurity());
          const bad = purity.filter((p) => p.share !== null && p.share > 0.5);
          return ok(purity.length > 0 && bad.length === 0, purity,
            bad.length === 0 ? '' : `${bad.map((b) => b.label).join(', ')} enclose mostly other folders`);
        },
      },
      {
        id: 'regions-earn-their-outline',
        criterion: 'Raising cohesion does not reduce how many regions are drawn',
        async run(page) {
          // The self-regulating claim: separability is what earns an
          // outline, so tightening the layout may add regions and must
          // never remove them.
          await page.eval(() => window.__probe.setCohesion('off'));
          await waitSettled(page);
          const off = (await page.eval(() => window.__probe.hulls())).length;
          await page.eval(() => window.__probe.setCohesion('high'));
          await waitSettled(page);
          const high = (await page.eval(() => window.__probe.hulls())).length;
          await page.eval(() => window.__probe.setCohesion('high'));
          await waitSettled(page);
          return ok(high >= off, { off, high },
            high >= off ? '' : 'tightening the layout removed regions');
        },
      },
      {
        id: 'regions-are-named',
        criterion: 'Every outline carries the folder name',
        async run(page) {
          const hulls = await page.eval(() => window.__probe.hulls());
          const named = hulls.filter((h) => h.label && h.label.length > 0);
          return ok(hulls.length > 0 && named.length === hulls.length,
            hulls.map((h) => h.label),
            named.length === hulls.length ? '' : 'an outline has no label — the name is the point of the ticket');
        },
      },
      {
        id: 'label-is-not-an-entity-name',
        criterion: 'A region name cannot be mistaken for an entity name',
        async run(page) {
          const [hulls, nameSize] = await Promise.all([
            page.eval(() => window.__probe.hulls()),
            page.eval(() => window.__probe.nameLabelFontSize()),
          ]);
          const hullSize = hulls[0]?.fontSize;
          const upper = hulls.every((h) => h.label === h.label.toUpperCase());
          const pass = !!hullSize && hullSize !== nameSize && upper;
          return ok(pass, { hullSize, nameSize, upper },
            pass ? '' : 'hull labels read like node labels');
        },
      },
      {
        id: 'painted-behind-everything',
        criterion: 'Outlines sit behind links and nodes',
        async run(page) {
          const o = await page.eval(() => window.__probe.hullPaintOrder());
          const pass = o && o.hulls >= 0 && o.hulls < o.links && o.hulls < o.nodes;
          return ok(pass, o, pass ? '' : 'the hull layer is not the backmost group');
        },
      },
      {
        id: 'only-the-region-itself-is-hittable',
        criterion: 'The outline takes events; the empty space around it does not',
        async run(page) {
          // This check used to read "outlines are not hit-testable", which was
          // the *mechanism* UI-055 used to protect click-to-deselect: the
          // deselect test fired only when the event target was the <svg>
          // itself, and a hull covers most of the canvas. UI-071 made the
          // region hittable and widened that test to count a hull as
          // background, so the mechanism moved and the claim did not.
          //
          // What is still ui-055's to assert is the layering: the group stays
          // inert and only the two drawn parts opt in, so the gap between one
          // region and the next is empty canvas. That deselect still happens
          // is asserted behaviourally in `ui-071`, which is where the gesture
          // now lives — it is deliberately not repeated here, because
          // selecting a node refocuses the whole graph and would leave the
          // rest of this suite measuring a different picture.
          const hulls = await page.eval(() => window.__probe.hulls());
          const pass = hulls.length > 0
            && hulls.every((h) => h.groupPointerEvents === 'none' && h.pointerEvents === 'fill');
          return ok(pass, hulls.map((h) => ({ path: h.path, group: h.groupPointerEvents, shape: h.pointerEvents })),
            pass ? '' : 'the region layer no longer opts in part by part');
        },
      },
      {
        id: 'fill-does-not-claim-the-colour-channel',
        criterion: 'The wash is faint enough not to read as node colour',
        async run(page) {
          const hulls = await page.eval(() => window.__probe.hulls());
          const worst = Math.max(...hulls.map((h) => h.fillOpacity ?? 0));
          return ok(worst > 0 && worst <= 0.12, { worst },
            worst <= 0.12 ? '' : 'hull fill is strong enough to shift how a node reads');
        },
      },
      {
        id: 'restores-default-cohesion',
        criterion: 'The suite leaves the persisted strength back at its default',
        async run(page) {
          await page.eval(() => window.__probe.setCohesion('low'));
          await sleep(500);
          const segs = await page.eval(() => window.__probe.cohesionState());
          return ok(segs.find((x) => x.active)?.label === 'Low', segs);
        },
      },
      {
        id: 'toggle-removes-them',
        criterion: 'Turning the control off leaves no outline behind',
        async run(page) {
          await page.eval(() => window.__probe.setHulls(false));
          await sleep(1000);
          const off = await page.eval(() => window.__probe.hulls());
          await page.eval(() => window.__probe.setHulls(true));
          await sleep(1200);
          const on = await page.eval(() => window.__probe.hulls());
          const pass = off.length === 0 && on.length > 0;
          return ok(pass, { off: off.length, on: on.length },
            pass ? '' : 'the toggle did not fully clear or restore the layer');
        },
      },
    ],
  },

  'ui-064': {
    ticket: 'UI-064', title: 'A loaded diff must not blank the collapsed canvas',
    // Needs a diff. `nao watch` picks up an existing diff.json at boot; if
    // there isn't one, compute HEAD → working first:
    //   curl -XPOST localhost:3000/api/diff -H 'content-type: application/json' \
    //        -d '{"from_ref":"HEAD","to_ref":"WORKING"}'
    // Deliberately does NOT call `clearDiff` — the filters being on is the
    // whole subject.
    async setup(page) {
      await page.viewport(1600, 1000);
      await page.goto(APP); await ready(page);
      await page.eval((n) => window.__probe.pickScope(n), 'ui');
      await sleep(14000);
    },
    checks: [
      {
        id: 'diff-is-loaded',
        criterion: 'A diff is active, so the rest of this suite means something',
        async run(page) {
          const s = await page.eval(() => window.__probe.diffState());
          return ok(s.active, s, s.active ? '' : 'no diff bar — compute one (see the suite header) or these checks prove nothing');
        },
      },
      {
        id: 'collapsed-canvas-draws',
        criterion: 'A scope that auto-collapses to File level draws nodes at the narrowest rung',
        async run(page) {
          // The bug, exactly: `Shown: 0` on a scope that was just reported as
          // in-scope and collapsed, with every sidebar filter permissive.
          await page.eval(() => window.__probe.setDiffLevel('edits'));
          await sleep(3000);
          const s = await page.eval(() => window.__probe.diffState());
          return ok(s.shown > 0, s,
            s.shown > 0 ? '' : 'the narrowest rung emptied the collapsed canvas — the entity-keyed lookup is back');
        },
      },
      {
        id: 'rungs-narrow-rather-than-erase',
        criterion: 'edits ⊆ rewiring ⊆ neighbourhood, with each step a real count',
        async run(page) {
          // A filter that hides everything and a filter that hides nothing
          // are both easy to ship by accident. What says the rollup is being
          // read is the ladder between them.
          const at = async (level) => {
            await page.eval((l) => window.__probe.setDiffLevel(l), level);
            await sleep(2500);
            return (await page.eval(() => window.__probe.diffState())).shown;
          };
          const edits = await at('edits');
          const rewiring = await at('rewiring');
          const neighbourhood = await at('neighbourhood');
          const pass = edits > 0 && edits <= rewiring && rewiring <= neighbourhood
            && rewiring < neighbourhood;
          return ok(pass, { edits, rewiring, neighbourhood },
            pass ? '' : 'expected 0 < edits ≤ rewiring < neighbourhood — a flat ladder means the rungs are not consulting the rollup');
        },
      },
    ],
  },

  'ui-088': {
    ticket: 'UI-088', title: 'The diff ladder draws what changed, edges included',
    // Needs a diff loaded. Against your own engine, not the one you are using:
    //   nao watch <repo> --port 3010 --allow-origin http://localhost:5210
    //   curl -XPOST localhost:3010/api/diff -H 'content-type: application/json' \
    //        -d '{"from_ref":"HEAD~1","to_ref":"WORKING"}'
    async setup(page) {
      await page.viewport(1600, 1000);
      await page.goto(APP); await ready(page);
      await page.eval((n) => window.__probe.pickScope(n), 'ui');
      await sleep(14000);
    },
    checks: [
      {
        id: 'diff-is-loaded',
        criterion: 'A diff is active, so the rest of this suite means something',
        async run(page) {
          const s = await page.eval(() => window.__probe.diffState());
          return ok(s.active, s, s.active ? '' : 'no diff ladder on screen — compute a diff (see the suite header)');
        },
      },
      {
        id: 'the-badge-covers-no-other-control',
        criterion: 'The diff badge, at its widest, hides neither the mode bar nor the overview',
        async run(page) {
          // A loaded diff is when the bottom-left group is longest: seed
          // split, four rungs, both opacity sliders and the undrawn count. It
          // used to run straight under the refresh button and the endpoint
          // chip, which stayed clickable-looking and were not.
          //
          // Measured at `rewiring`, which is the widest the group ever gets:
          // the Context slider (UI-112) is absent at `edits`, and the undrawn
          // count is absent at `neighbourhood`. Only this rung shows both.
          // Measured on the badge, not on the strip group around it: a flex
          // item that has been shrunk still reports the smaller box while its
          // contents spill past it, which is precisely how this went wrong.
          await page.eval((l) => window.__probe.setDiffLevel(l), 'rewiring');
          await sleep(1500);
          const results = {};
          for (const w of [1600, 1440, 1280, 1152]) {
            await page.viewport(w, 900); await sleep(900);
            results[w] = {
              modeBar: await page.eval(() => window.__probe.overlap('diff-badge', 'mode-bar')),
              overview: await page.eval(() => window.__probe.overlap('diff-badge', 'overview-panel')),
            };
          }
          await page.viewport(1600, 1000); await sleep(900);
          const bad = Object.entries(results).filter(([, v]) => v.modeBar || v.overview);
          return ok(bad.length === 0, results,
            bad.length ? `the badge sits on another control at ${bad.map((b) => b[0]).join(', ')}px` : '');
        },
      },
      {
        id: 'edits-draws-no-untouched-wiring',
        criterion: 'The narrowest rung draws strictly fewer edges than the widest',
        async run(page) {
          // The finding UI-088 exists for: three quarters of the edges the
          // old view drew were untouched wiring that merely happened to run
          // between two changed entities. If these two counts are equal, the
          // rung is not filtering edges at all and the ticket is unbuilt.
          const at = async (level) => {
            await page.eval((l) => window.__probe.setDiffLevel(l), level);
            await sleep(2500);
            return await page.eval(() => window.__probe.diffState());
          };
          const edits = await at('edits');
          const neighbourhood = await at('neighbourhood');
          const pass = edits.links < neighbourhood.links;
          return ok(pass, { edits: edits.links, neighbourhood: neighbourhood.links },
            pass ? '' : 'the narrow rung drew as many edges as the wide one — edges are not being filtered');
        },
      },
      {
        id: 'the-neighbourhood-is-not-drawn-like-the-edits',
        criterion: 'What a rung recruits is drawn quieter than what was edited (UI-112)',
        async run(page) {
          // The complaint this exists for: widening to Neighbourhood multiplies
          // the node count, and every added node arrives drawn exactly like the
          // handful that changed — so the picture gains context and loses its
          // subject. The Context slider is a third opacity tier, and it must be
          // absent at `edits`, which recruits nothing to weight.
          await page.eval((l) => window.__probe.setDiffLevel(l), 'edits');
          await sleep(2500);
          const edits = await page.eval(() => window.__probe.diffState());
          await page.eval((l) => window.__probe.setDiffLevel(l), 'neighbourhood');
          await sleep(2500);
          const wide = await page.eval(() => window.__probe.diffState());
          const pass = edits.contextPct === null && wide.contextPct !== null && wide.faded > 0;
          return ok(pass, { edits, neighbourhood: wide },
            pass ? '' : 'the recruited neighbourhood is drawn at the same strength as the edits');
        },
      },
      {
        id: 'rewiring-reaches-unedited-far-ends',
        criterion: 'Rewiring draws nodes that Edits does not — the far ends of changed edges',
        async run(page) {
          const at = async (level) => {
            await page.eval((l) => window.__probe.setDiffLevel(l), level);
            await sleep(2500);
            return await page.eval(() => window.__probe.diffState());
          };
          const edits = await at('edits');
          const rewiring = await at('rewiring');
          // Equal is a legitimate outcome on a diff where every changed edge
          // happens to land between two edited entities, so this reports
          // rather than fails on equality — but it must never shrink.
          const pass = rewiring.shown >= edits.shown;
          return ok(pass, { editsNodes: edits.shown, rewiringNodes: rewiring.shown },
            pass ? '' : 'widening the rung removed nodes — the ladder is not ordered');
        },
      },
      {
        id: 'undrawable-edges-are-counted-not-dropped',
        criterion: 'Reported edge changes the canvas cannot draw are stated on screen',
        async run(page) {
          // A removed edge has no line in the head graph, by construction.
          // Saying nothing about it is the failure mode: the canvas would
          // read as complete coverage of the diff when it is not.
          await page.eval(() => window.__probe.setDiffLevel('edits'));
          await sleep(2000);
          const s = await page.eval(() => window.__probe.diffState());
          const summary = await page.eval(() => ({
            edges: document.querySelector('[data-probe="diff-edge-counts"]')?.textContent.trim() ?? null,
          }));
          // If the diff reported removals, the note must be there. If it
          // reported none, its absence is correct.
          const removals = Number((summary.edges?.match(/[−-]\s*(\d+)/) ?? [0, 0])[1]);
          const pass = removals === 0 ? s.undrawable === null : s.undrawable !== null;
          return ok(pass, { ...summary, removals, undrawable: s.undrawable },
            pass ? '' : `${removals} removed edges reported but nothing on screen says they cannot be drawn`);
        },
      },
    ],
  },

  'ui-054': {
    ticket: 'UI-054', title: 'Hover highlights the folder group',
    async setup(page) {
      await page.viewport(1600, 1000);
      await page.goto(APP); await ready(page);
      await page.eval((n) => window.__probe.pickScope(n), 'ui');
      await clearDiff(page);
      await waitSettled(page);
    },
    checks: [
      {
        id: 'both-questions-offered',
        criterion: 'Hover can answer either question, with one selected',
        async run(page) {
          const st = await page.eval(() => window.__probe.hoverModeState());
          const present = st.filter((m) => m.present);
          const active = st.filter((m) => m.active);
          return ok(present.length === 2 && active.length === 1, st,
            present.length === 2 ? '' : 'the hover-mode control is missing');
        },
      },
      {
        id: 'links-is-the-default',
        criterion: 'Connections stays the default — this is an addition, not a replacement',
        async run(page) {
          const st = await page.eval(() => window.__probe.hoverModeState());
          const active = st.find((m) => m.active);
          return ok(active?.mode === 'connections', st);
        },
      },
      {
        id: 'group-mode-lights-one-folder',
        criterion: 'In Folder mode, everything lit shares the hovered node\'s folder',
        async run(page) {
          await page.eval(() => window.__probe.setHoverMode('group'));
          await sleep(400);
          const r = await page.eval(() => window.__probe.hoverNode('displayPlan.ts'));
          if (!r) return ok(false, r, 'displayPlan.ts not drawn — cannot hover it');
          // The lit set must be exactly one folder, and it must be the
          // hovered node's own.
          const pass = r.litFolders.length === 1 && r.litFolders[0] === r.hoveredFolder && r.lit < r.drawn;
          return ok(pass, r, pass ? '' : 'the lit set is not one folder');
        },
      },
      {
        id: 'links-mode-answers-differently',
        criterion: 'Connections mode lights a different set from Folder mode',
        async run(page) {
          await page.eval(() => window.__probe.unhoverAll());
          await page.eval(() => window.__probe.setHoverMode('connections'));
          await sleep(400);
          const r = await page.eval(() => window.__probe.hoverNode('displayPlan.ts'));
          if (!r) return ok(false, r, 'displayPlan.ts not drawn');
          // Reachability crosses folders; membership does not. If this lit
          // exactly one folder, the two modes are not actually distinct.
          return ok(r.litFolders.length > 1, r,
            r.litFolders.length > 1 ? '' : 'connections mode lit a single folder — are the modes wired to the same path?');
        },
      },
      {
        id: 'hover-does-not-outlive-the-cursor',
        criterion: 'Leaving the node restores every dimmed element',
        async run(page) {
          await page.eval(() => window.__probe.setHoverMode('group'));
          await page.eval(() => window.__probe.hoverNode('displayPlan.ts'));
          await sleep(200);
          const stillDimmed = await page.eval(() => window.__probe.unhoverAll());
          return ok(stillDimmed === 0, { stillDimmed },
            stillDimmed === 0 ? '' : 'a hover-scoped dim survived mouseout');
        },
      },
      {
        id: 'depth-says-it-is-unavailable',
        criterion: 'Highlight depth greys out in Folder mode instead of silently doing nothing',
        async run(page) {
          await page.eval(() => window.__probe.setHoverMode('group'));
          await sleep(300);
          const inGroup = await page.eval(() => window.__probe.depthEnabled());
          await page.eval(() => window.__probe.setHoverMode('connections'));
          await sleep(300);
          const inLinks = await page.eval(() => window.__probe.depthEnabled());
          const pass = inGroup.enabled === 0 && inLinks.enabled === inLinks.count && inLinks.count > 0;
          return ok(pass, { inGroup, inLinks },
            pass ? '' : 'depth availability does not track the hover mode');
        },
      },
    ],
  },

  'ui-056': {
    ticket: 'UI-056', title: 'Demote hub nodes',
    async setup(page) {
      await page.viewport(1600, 1000);
      await page.goto(APP); await ready(page);
      await page.eval((n) => window.__probe.pickScope(n), 'ui');
      await clearDiff(page);
      await waitSettled(page);
    },
    checks: [
      {
        id: 'off-by-default',
        criterion: 'Nothing is hidden until the reader asks for it',
        async run(page) {
          const list = await page.eval(() => window.__probe.demotedList());
          return ok(list === null, { list },
            list === null ? '' : 'edges were being hidden on first load');
        },
      },
      {
        id: 'demoting-removes-edges-not-nodes',
        criterion: 'Turning it on draws fewer edges and the same nodes',
        async run(page) {
          const before = await page.eval(() => window.__probe.drawnCounts());
          await page.eval(() => window.__probe.setDemote(true));
          await sleep(2500);
          const after = await page.eval(() => window.__probe.drawnCounts());
          const pass = after.links < before.links && after.nodes === before.nodes;
          return ok(pass, { before, after },
            pass ? '' : 'either no edges went away, or nodes did');
        },
      },
      {
        id: 'says-what-it-took',
        criterion: 'The demoted nodes are named while the control is active',
        async run(page) {
          const list = await page.eval(() => window.__probe.demotedList());
          const pass = !!list && /Demoted:/.test(list);
          return ok(pass, { list },
            pass ? '' : 'edges vanished with nothing on screen saying which');
        },
      },
      {
        id: 'a-demoted-node-is-still-a-node',
        criterion: 'A demoted node stays drawn and stays selectable',
        async run(page) {
          const list = await page.eval(() => window.__probe.demotedList());
          const listed = (list ?? '').replace(/^Demoted:\s*/, '').split(',')[0]?.trim();
          if (!listed) return ok(false, { list }, 'no demoted name to test');
          // The panel qualifies colliding names (`types/graph.ts`); the canvas
          // label is the bare file name, so search on the last segment.
          const first = listed.slice(listed.lastIndexOf('/') + 1);
          const r = await page.eval((n) => window.__probe.nodeIsSelectable(n), first);
          // The selection re-runs the display plan, so the class lands a tick
          // later; read it back rather than trusting the click's own return.
          await sleep(1500);
          const cls = await page.eval((n) => window.__probe.nodeIsSelected(n), first);
          // Clear it again. A selection narrows the view to a BFS around the
          // node, and leaving one set would hand the next check a seven-node
          // graph — which is exactly what it did on the first run.
          await page.eval(() => window.__probe.clearSelection());
          await waitSettled(page);
          return ok(r.present && cls, { first, present: r.present, selected: cls },
            r.present ? '' : 'the demoted node left the canvas');
        },
      },
      {
        id: 'count-changes-what-is-demoted',
        criterion: 'Raising N demotes more and removes more edges',
        async run(page) {
          await page.eval(() => window.__probe.setHubCount(3));
          await sleep(2000);
          const few = await page.eval(() => window.__probe.drawnCounts());
          await page.eval(() => window.__probe.setHubCount(10));
          await sleep(2000);
          const many = await page.eval(() => window.__probe.drawnCounts());
          return ok(many.links < few.links, { few, many },
            many.links < few.links ? '' : 'N had no effect on the picture');
        },
      },
      {
        id: 'turning-it-off-restores-everything',
        criterion: 'The control is fully reversible',
        async run(page) {
          const on = await page.eval(() => window.__probe.drawnCounts());
          await page.eval(() => window.__probe.setDemote(false));
          await sleep(2500);
          const off = await page.eval(() => window.__probe.drawnCounts());
          const pass = off.links > on.links && off.nodes === on.nodes;
          return ok(pass, { on, off }, pass ? '' : 'the graph did not come back');
        },
      },
    ],
  },

  'ui-057': {
    ticket: 'UI-057', title: 'Expand one scope without expanding all of them',
    async setup(page) {
      await page.viewport(1600, 1000);
      await page.goto(APP); await ready(page);
      // `src` is big enough that auto-level lands on Module, which is where
      // opening one scope in place has something to prove.
      await page.eval((n) => window.__probe.pickScope(n), 'src');
      await clearDiff(page);
      await waitSettled(page);
    },
    checks: [
      {
        id: 'gesture-is-discoverable',
        criterion: 'The toolbar says how to open a scope',
        async run(page) {
          const st = await page.eval(() => window.__probe.expansionState());
          return ok(!!st.gesture && /double-click/i.test(st.gesture), st,
            st.gesture ? '' : 'nothing on screen tells the reader the gesture exists');
        },
      },
      {
        id: 'one-scope-opens-the-rest-stay-shut',
        criterion: 'Opening one scope draws its contents beside collapsed ones',
        async run(page) {
          // Whichever level auto-level picked, the claim is the same: after
          // opening one scope the canvas carries *two* granularities at once.
          // Targeting the level that is actually on screen keeps this honest
          // on any repo — `src` here collapses to File, not Module.
          const before = await page.eval(() => window.__probe.drawnNodeKinds());
          const rollup = (before.Module ?? 0) > 0 ? 'Module' : 'File';
          const target = await page.eval((k) => window.__probe.firstNodeOfKind(k), rollup);
          if (!target) return ok(false, { before, rollup }, `no ${rollup} node to open`);
          await page.eval((n) => window.__probe.expandNode(n), target);
          await waitSettled(page);
          const after = await page.eval(() => window.__probe.drawnNodeKinds());
          const finer = Object.keys(after).filter((k) => k !== rollup);
          const pass = (after[rollup] ?? 0) > 0 && finer.length > 0;
          return ok(pass, { rollup, target, before, after },
            pass ? '' : 'the view is still uniform — expansion did not take');
        },
      },
      {
        id: 'expansion-is-counted-and-reversible',
        criterion: 'The toolbar counts open scopes and closes them again',
        async run(page) {
          const open = await page.eval(() => window.__probe.expansionState());
          const opened = await page.eval(() => window.__probe.drawnNodeKinds());
          await page.eval(() => window.__probe.collapseAll());
          await waitSettled(page);
          const shut = await page.eval(() => window.__probe.drawnNodeKinds());
          // Back to one granularity, and the toolbar had counted the one
          // scope that was open.
          const pass = open.count === 1 && Object.keys(shut).length === 1;
          return ok(pass, { open, opened, shut },
            pass ? '' : 'collapsing did not restore the uniform view');
        },
      },
      {
        id: 'opening-does-not-refetch',
        criterion: 'Opening a scope redraws; it does not reload the dataset',
        async run(page) {
          // The scope line reports what is *loaded*. Expansion is a drawing
          // decision (ADR 0010) and must not move it.
          const before = await page.eval(() => window.__probe.scopeEntities());
          const target = await page.eval(() => window.__probe.firstNodeOfKind('Module'));
          await page.eval((n) => window.__probe.expandNode(n), target);
          await waitSettled(page);
          const after = await page.eval(() => window.__probe.scopeEntities());
          await page.eval(() => window.__probe.collapseAll());
          await waitSettled(page);
          return ok(before === after, { before, after },
            before === after ? '' : 'expanding changed what is loaded');
        },
      },
    ],
  },

  'ui-058': {
    ticket: 'UI-058', title: 'Edges into a collapsed group merge into one',
    async setup(page) {
      await page.viewport(1600, 1000);
      await page.goto(APP); await ready(page);
      await page.eval((n) => window.__probe.pickScope(n), 'ui');
      await clearDiff(page);
      await waitSettled(page);
    },
    checks: [
      {
        id: 'collapsed-neighbours-speak-dependency',
        criterion: 'With everything collapsed, every edge is a dependency',
        async run(page) {
          const kinds = await page.eval(() => window.__probe.drawnLinkKinds());
          const total = Object.values(kinds).reduce((a, b) => a + b, 0);
          const pass = total > 0 && (kinds.DependsOn ?? 0) === total;
          return ok(pass, kinds,
            pass ? '' : '"file A calls file B" is nonsense — these should read as dependencies');
        },
      },
      {
        id: 'opened-regions-keep-their-real-kinds',
        criterion: 'Inside an opened file, edges keep the kind they really are',
        async run(page) {
          const kinds0 = await page.eval(() => window.__probe.drawnNodeKinds());
          const rollup = (kinds0.Module ?? 0) > 0 ? 'Module' : 'File';
          const target = await page.eval((k) => window.__probe.firstNodeOfKind(k), rollup);
          if (!target) return ok(false, { kinds0 }, `no ${rollup} node to open`);
          await page.eval((n) => window.__probe.expandNode(n), target);
          await waitSettled(page);
          const kinds = await page.eval(() => window.__probe.drawnLinkKinds());
          const specific = Object.entries(kinds).filter(([k]) => k !== 'DependsOn');
          await page.eval(() => window.__probe.collapseAll());
          await waitSettled(page);
          return ok(specific.length > 0, { target, kinds },
            specific.length > 0 ? '' : 'every edge is still DependsOn — real kinds were flattened');
        },
      },
      {
        id: 'a-merged-edge-carries-its-count',
        criterion: 'A merged edge says how many relationships it stands for',
        async run(page) {
          const titles = await page.eval(() => [...document.querySelectorAll('line.link title')]
            .map((t) => t.textContent).filter((t) => /underlying relationship/.test(t)).slice(0, 3));
          return ok(titles.length > 0, titles,
            titles.length > 0 ? '' : 'no merged edge reports its breakdown on hover');
        },
      },
    ],
  },

  'ui-083': {
    ticket: 'UI-083', title: 'An overview panel that says where the viewport is',
    /**
     * Every claim here is about the *box*, because a minimap that draws a
     * pretty cloud and puts the box in the wrong place is worse than no
     * minimap — it is a confident wrong answer to the one question the panel
     * exists to answer, and nothing on screen contradicts it.
     *
     * The geometry itself is unit-tested (`npm run test:overview`). What
     * needs a browser is the wiring: that the box responds to a real zoom,
     * that a real click steers the real canvas, and that the panel is over
     * the canvas rather than beside it.
     */
    async setup(page) {
      await page.viewport(1600, 1000);
      await page.goto(APP); await ready(page);
      await page.eval((n) => window.__probe.pickScope(n), 'ui');
      await waitSettled(page);
    },
    checks: [
      {
        id: 'panel-overlays-the-canvas',
        criterion: 'The overview sits inside the canvas, taking no column from it',
        async run(page) {
          const o = await page.eval(() => window.__probe.overview());
          return ok(!!o && o.open && o.insideCanvas === true, o,
            o ? '' : 'no [data-probe="overview-panel"] on the page');
        },
      },
      {
        id: 'clears-the-bottom-bar',
        criterion: 'The panel does not cover the live-status and endpoint chips',
        async run(page) {
          // Both want the bottom-right corner. A status indicator hidden
          // under an opaque panel is worse than one that is absent — the page
          // still looks like it is reporting.
          const over = await page.eval(() => window.__probe.overlap('overview-panel', 'mode-bar'));
          return ok(over === null, { overlap: over },
            over === null ? '' : 'the overview is sitting on the mode bar');
        },
      },
      {
        id: 'draws-the-graph-it-summarises',
        criterion: 'One dot per drawn node, and the same count as the canvas',
        async run(page) {
          const o = await page.eval(() => window.__probe.overview());
          const drawn = await page.eval(() => window.__probe.renderedNodes());
          // The box is a `rect`, not a `circle`, so the dot count is exact
          // rather than off by one.
          return ok(o?.dots === drawn && drawn > 0, { dots: o?.dots, drawn },
            o?.dots === drawn ? '' : 'the panel and the canvas disagree about what is drawn');
        },
      },
      {
        id: 'box-starts-inside-the-panel',
        criterion: 'The viewport box is drawn within the panel at rest',
        async run(page) {
          const o = await page.eval(() => window.__probe.overview());
          const b = o?.box;
          const pass = !!b && b.x >= -0.5 && b.y >= -0.5 &&
            b.x + b.w <= o.map.w + 0.5 && b.y + b.h <= o.map.h + 0.5;
          return ok(pass, { box: b, map: o?.map },
            pass ? '' : 'the box is outside the panel — the reader is told nothing');
        },
      },
      {
        id: 'zooming-in-shrinks-the-box',
        criterion: 'Zooming the canvas shrinks the box, and it stays inside',
        async run(page) {
          const before = await page.eval(() => window.__probe.overview());
          for (let i = 0; i < 3; i++) {
            await page.eval(() => window.__probe.zoomCanvas('in'));
            await sleep(500);
          }
          const after = await page.eval(() => window.__probe.overview());
          const k = await page.eval(() => window.__probe.canvasTransform());
          const inside = !!after?.box && after.box.x >= -0.5 && after.box.y >= -0.5 &&
            after.box.x + after.box.w <= after.map.w + 0.5 &&
            after.box.y + after.box.h <= after.map.h + 0.5;
          // Shrunk, not merely changed: the direction is the whole claim.
          const shrank = !!before?.box && !!after?.box && after.box.share < before.box.share;
          return ok(shrank && inside, { before: before?.box, after: after?.box, k },
            shrank ? (inside ? '' : 'the box left the panel once zoomed')
              : 'the box did not shrink — it is not tracking the zoom');
        },
      },
      {
        id: 'a-click-steers-the-canvas',
        criterion: 'Clicking a corner of the panel pans the canvas that way',
        async run(page) {
          // Still zoomed in from the previous check, which is the state this
          // matters in — at 1x the whole graph is on screen and a pan has
          // nothing to reveal.
          const before = await page.eval(() => window.__probe.canvasTransform());
          await page.eval(() => window.__probe.clickOverview(0.15, 0.15));
          await sleep(400);
          const after = await page.eval(() => window.__probe.canvasTransform());
          // Centring on a point up and to the LEFT moves the world right and
          // down on screen, so both translations must increase. Asserting the
          // sign is what separates a working inverse from a sign-flipped one,
          // which moves just as convincingly in the wrong direction.
          const pass = after.x > before.x + 1 && after.y > before.y + 1 && after.k === before.k;
          return ok(pass, { before, after },
            pass ? '' : 'the click did not pan the canvas toward the point pressed');
        },
      },
      {
        id: 'the-corner-folds-away',
        criterion: 'The panel can be collapsed and the choice sticks',
        async run(page) {
          await page.eval(() => window.__probe.clickText('Overview'));
          await sleep(300);
          const closed = await page.eval(() => window.__probe.overview());
          await page.goto(APP); await ready(page);
          await page.eval((n) => window.__probe.pickScope(n), 'ui');
          await waitSettled(page);
          const reloaded = await page.eval(() => window.__probe.overview());
          // Put it back, so a later suite in the same run does not inherit a
          // hidden panel.
          await page.eval(() => window.__probe.clickText('Overview'));
          await sleep(300);
          const reopened = await page.eval(() => window.__probe.overview());
          const pass = closed?.open === false && reloaded?.open === false && reopened?.open === true;
          return ok(pass, { closed: closed?.open, reloaded: reloaded?.open, reopened: reopened?.open },
            pass ? '' : 'the collapse state did not survive a reload');
        },
      },
    ],
  },

  'ui-084': {
    ticket: 'UI-084', title: 'Mark several files, then drill into all of them',
    /**
     * The claim is a *transition*: a file-level view goes to an entity-level
     * one covering exactly the files that were picked. Every check below
     * either drives that transition or asserts something it must not have
     * disturbed — the pure part (which path a node stands for, what survives
     * a level change) is unit-tested in `npm run test:marks`, and what needs
     * a browser is the wiring between the gesture, the ring and the scope.
     */
    async setup(page) {
      await page.viewport(1600, 1000);
      await page.goto(APP); await ready(page);
      await page.eval((n) => window.__probe.pickScope(n), 'ui');
      await clearDiff(page);
      await waitSettled(page);
      // File level is where the feature is for: the reader can see which
      // files depend on which, and cannot see inside any of them.
      await page.eval(() => window.__probe.setLevel('File'));
      await waitSettled(page);
    },
    checks: [
      {
        id: 'mark-rings-without-selecting',
        criterion: '⌘-click rings a node and leaves the subject alone',
        async run(page) {
          const paths = await page.eval((n) => window.__probe.markNodes(n), 2);
          await sleep(400);
          const st = await page.eval(() => window.__probe.markState());
          // `selected` is the load-bearing half. Marking that re-pointed the
          // Details and Description panes would make building a set destroy
          // the reading you built it from.
          const pass = st.rings === 2 && st.marked === 2 && st.selected === 0;
          return ok(pass, { paths, ...st },
            pass ? '' : 'the ring and the subject did not come out independent');
        },
      },
      {
        id: 'toolbar-counts-the-set',
        criterion: 'The toolbar offers the drill and says how big the set is',
        async run(page) {
          const st = await page.eval(() => window.__probe.markState());
          const pass = /Drill into 2 marked/.test(st.button ?? '');
          return ok(pass, st,
            pass ? '' : 'the set has no control — a mark you cannot spend is invisible state');
        },
      },
      {
        id: 'drill-lands-at-entity-level',
        criterion: 'Spending the set re-opens the same files as entities',
        async run(page) {
          const before = await page.eval(() => window.__probe.drawnNodeKinds());
          await page.eval(() => window.__probe.drillMarks());
          await waitSettled(page);
          const st = await page.eval(() => window.__probe.markState());
          const after = await page.eval(() => window.__probe.drawnNodeKinds());
          // Nothing in the feature names a level: two files fall under the
          // render budget, so auto-level picks Entity on its own. A view
          // still made of File circles means the drill did not narrow.
          const pass = st.level === 'Entity' && !after.File;
          return ok(pass, { before, after, level: st.level },
            pass ? '' : 'the drill did not reach the entities');
        },
      },
      {
        id: 'the-set-is-spent',
        criterion: 'The marks and their control are gone once drilled',
        async run(page) {
          // Every node now drawn sits under a marked path, so a set that
          // survived the drill would ring the whole canvas.
          const st = await page.eval(() => window.__probe.markState());
          const pass = st.rings === 0 && st.marked === 0 && st.button === null;
          return ok(pass, st, pass ? '' : 'the spent set is still ringing nodes');
        },
      },
    ],
  },

  'ui-085': {
    ticket: 'UI-085', title: 'The details pane diffs the code it is showing',
    /**
     * Needs a diff. Compute one first:
     *   curl -XPOST localhost:3000/api/diff -H 'content-type: application/json' \
     *        -d '{"from_ref":"HEAD~1","to_ref":"WORKING"}'
     *
     * The line arithmetic — pairing, folding, counting — is unit-tested
     * (`npm run test:linediff`). What needs a browser is everything between
     * the engine and the pixels: that a base source is actually *found* for
     * the pinned entity (it wasn't, for as long as the lookup keys
     * disagreed), that both sides get drawn, and that the two controls do
     * what they say.
     */
    async setup(page) {
      await page.viewport(1600, 1000);
      await page.goto(APP); await ready(page);
      const subject = await changedSubject();
      if (!subject) throw new Error('no modified entity with source on both sides — compute a diff (see the suite header)');
      await ensureScope(page, topScopeOf(subject.file_path));
      await selectByName(page, subject.name, subject.file_path);
    },
    checks: [
      {
        id: 'a-changed-entity-is-drawn-as-a-diff',
        criterion: 'The pinned entity shows a diff, not a printout of its current source',
        async run(page) {
          const d = await page.eval(() => window.__probe.sourceDiff());
          const pinned = await page.eval(() => window.__probe.pinnedEntity());
          const pass = !!d && d.added + d.removed > 0;
          return ok(pass, { pinned, ...(d ?? {}) },
            d ? (pass ? '' : 'a diff frame with nothing marked in it')
              : 'no diff renderer in the pane — the base source was not found');
        },
      },
      {
        id: 'every-counted-line-is-marked',
        criterion: 'Unified view marks exactly the lines it counted, on both sides',
        async run(page) {
          // Not "there is at least one of each": which entity the engine
          // reports as the biggest edit is a property of the working tree,
          // and a pure insertion is a perfectly good diff. What must hold on
          // any fixture is that the header and the body agree.
          await page.eval(() => window.__probe.clickProbe('diff-mode-unified'));
          await sleep(400);
          const d = await page.eval(() => window.__probe.sourceDiff());
          const pass = d?.mode === 'unified' && d.addedLines === d.added && d.removedLines === d.removed;
          return ok(pass, d, pass ? '' : 'the +/- header and the marked lines disagree');
        },
      },
      {
        id: 'split-puts-before-beside-after',
        criterion: 'Split view draws two cells per row and nothing spills sideways',
        async run(page) {
          await page.eval(() => window.__probe.clickProbe('diff-mode-split'));
          await sleep(400);
          const d = await page.eval(() => window.__probe.sourceDiff());
          const paired = !!d && d.splitRows > 0 && d.splitCells === d.splitRows * 2;
          return ok(paired && d.mode === 'split', d,
            paired ? '' : 'the two columns are not row-aligned');
        },
      },
      {
        id: 'the-choice-survives-a-reload',
        criterion: 'The view mode is remembered',
        async run(page) {
          await page.goto(APP); await ready(page);
          // The scope survives the reload and the row *toggles*, so this
          // re-selects only if something cleared it.
          const subject = await changedSubject();
          await ensureScope(page, topScopeOf(subject.file_path));
          await selectByName(page, subject.name, subject.file_path);
          const d = await page.eval(() => window.__probe.sourceDiff());
          // Put it back for the checks below, which read the unified rows.
          await page.eval(() => window.__probe.clickProbe('diff-mode-unified'));
          await sleep(300);
          return ok(d?.mode === 'split', d, d?.mode === 'split' ? '' : 'the pane forgot the split view');
        },
      },
      {
        id: 'unchanged-stretches-fold-away',
        criterion: 'Far-from-the-change lines collapse, and the toggle brings them back',
        async run(page) {
          const folded = await page.eval(() => window.__probe.sourceDiff());
          await page.eval(() => window.__probe.clickProbe('diff-context-toggle'));
          await sleep(400);
          const whole = await page.eval(() => window.__probe.sourceDiff());
          await page.eval(() => window.__probe.clickProbe('diff-context-toggle'));
          await sleep(400);
          // A short entity has nothing to fold, and that is not a failure —
          // what must hold is that "whole entity" never shows *less*.
          const pass = !!whole && whole.unifiedLines >= (folded?.unifiedLines ?? 0)
            && whole.gaps.length === 0;
          return ok(pass, { folded: { lines: folded?.unifiedLines, gaps: folded?.gaps }, whole: { lines: whole?.unifiedLines, gaps: whole?.gaps } },
            pass ? '' : 'the context toggle did not restore the hidden lines');
        },
      },
      {
        id: 'a-changed-file-diffs-itself',
        criterion: 'A file node shows the file\'s own diff, and what changed inside it',
        async run(page) {
          // The state a reader is actually in: Current Changes on anything
          // bigger than a small scope opens at File level, so the node under
          // the cursor is a file — which has no row in the diff at all, since
          // `compute_diff` walks entities and a file is not one. Before
          // UI-097 this pane printed the file's source with nothing marked.
          const file = await changedFile();
          if (!file) return ok(false, null, 'no changed file with text on both sides — fixture, not the code');
          await page.eval((l) => window.__probe.setLevel(l), 'File');
          await sleep(3000);
          const clicked = await page.eval((n) => window.__probe.selectNode(n), file.name);
          await sleep(2500);
          const d = await page.eval(() => window.__probe.sourceDiff());
          const tally = await page.eval(() => {
            const el = document.querySelector('[data-probe="scope-tally"]');
            return el ? el.textContent.replace(/\s+/g, ' ').trim() : null;
          });
          const pinned = await page.eval(() => window.__probe.pinnedEntity());
          // Put the level back: it persists, and a later suite that searches
          // for an entity would find it "collapsed into its file" and select
          // the file instead — measuring the wrong subject entirely.
          await page.eval((l) => window.__probe.setLevel(l), 'Entity');
          await sleep(2000);
          const pass = clicked && !!d && d.added + d.removed > 0 && !!tally;
          return ok(pass, { file: file.name, pinned, tally, diff: d && { added: d.added, removed: d.removed, gaps: d.gaps.length } },
            pass ? '' : 'the file node still says nothing about what changed in it');
        },
      },
    ],
  },

  'ui-086': {
    ticket: 'UI-086', title: 'Relationships that appeared or disappeared',
    /**
     * Same fixture as ui-085 — a computed diff. The engine's own arithmetic
     * is unit-tested in `cargo test --lib diff::`; this asserts the pane
     * reports exactly what the engine found, including the edges that are in
     * neither the head graph nor the canvas and so have nowhere else to
     * appear.
     */
    async setup(page) {
      await page.viewport(1600, 1000);
      await page.goto(APP); await ready(page);
      const subject = await rewiredSubject();
      if (!subject) throw new Error('no entity with edge changes in the engine diff');
      await ensureScope(page, topScopeOf(subject.file_path));
      // These rows belong to an entity, and the level persists across runs:
      // at File level the search hit is "collapsed into its file" and
      // selecting it pins the file, whose deltas are a different question.
      await page.eval((l) => window.__probe.setLevel(l), 'Entity');
      await sleep(2500);
      await selectByName(page, subject.name, subject.file_path);
    },
    checks: [
      {
        id: 'the-engine-counts-rewiring',
        criterion: 'The diff reports edges appearing or disappearing at all',
        async run() {
          const d = await engineDiff();
          const n = (d.summary.relationships_added ?? 0) + (d.summary.relationships_removed ?? 0);
          return ok(n > 0, d.summary, n > 0 ? '' : 'no edge deltas in this diff — the checks below prove nothing');
        },
      },
      {
        id: 'every-changed-edge-is-listed',
        criterion: 'The pane lists exactly the deltas the engine gave for this entity',
        async run(page) {
          const subject = await rewiredSubject();
          const rows = await page.eval(() => window.__probe.relChanges());
          const want = subject.rel_deltas.length;
          const added = rows.filter((r) => r.status === 'added').length;
          const wantAdded = subject.rel_deltas.filter((r) => r.status === 'added').length;
          const pass = rows.length === want && added === wantAdded;
          return ok(pass, { entity: subject.name, rows: rows.length, want, added, wantAdded, sample: rows.slice(0, 3) },
            pass ? '' : 'the pane and the engine disagree about what moved');
        },
      },
      {
        id: 'a-changed-edge-names-its-far-end',
        criterion: 'Each row says which entity is at the other end',
        async run(page) {
          const subject = await rewiredSubject();
          const rows = await page.eval(() => window.__probe.relChanges());
          const missing = subject.rel_deltas.filter(
            (d) => !rows.some((r) => r.text.includes(d.other_name)),
          );
          return ok(missing.length === 0, { missing: missing.map((m) => m.other_name).slice(0, 5) },
            missing.length ? 'a delta was counted but its far end is not named' : '');
        },
      },
      {
        id: 'a-listed-edge-navigates',
        criterion: 'A row whose far end is on the canvas selects it',
        async run(page) {
          const before = await page.eval(() => window.__probe.pinnedEntity());
          const moved = await page.eval(() => {
            const r = [...document.querySelectorAll('[data-probe="rel-change"]')].find((x) => !x.disabled);
            if (!r) return false;
            r.click();
            return true;
          });
          await sleep(1200);
          const after = await page.eval(() => window.__probe.pinnedEntity());
          // No navigable row is a legitimate state — every far end may be
          // off-canvas — but then this claim is untested, and saying so is
          // better than a green tick.
          return ok(moved && after !== before, { moved, before, after },
            moved ? (after === before ? 'the row did not move the selection' : '')
              : 'no navigable row in this diff to click');
        },
      },
    ],
  },

  'ui-092': {
    ticket: 'UI-092', title: 'A way back from every drill',
    /**
     * Every check here is about a *sequence*, because that is where a history
     * goes wrong: one step looks right in isolation and the third one lands
     * somewhere nobody was. So the suite drills, adjusts, steps back, steps
     * forward and drills again, asserting the stack depth at each point —
     * depth being the thing that says whether a gesture was remembered, which
     * is the claim the pixels cannot make.
     */
    async setup(page) {
      await page.viewport(1600, 1000);
      await page.goto(APP); await ready(page);
      await page.eval((n) => window.__probe.pickScope(n), 'ui');
      await clearDiff(page);
      await waitSettled(page);
    },
    checks: [
      {
        id: 'the-controls-are-there-and-empty',
        criterion: 'Back and forward exist, disabled, before anything has been navigated',
        async run(page) {
          const w = await page.eval(() => window.__probe.wayback());
          if (!w) return ok(false, null, 'no wayback controls in the toolbar bar');
          // Ticking a folder in the scope tree is an adjustment, not a
          // navigation — and the first-run auto-scope replaced an empty
          // canvas, which is not a picture worth returning to.
          const pass = w.depth === 0 && !w.backEnabled && !w.forwardEnabled;
          return ok(pass, w, pass ? '' : 'the stack is not empty before the first navigation');
        },
      },
      {
        id: 'a-drill-is-remembered',
        criterion: 'Drilling into a collapsed node records the picture it left',
        async run(page) {
          const before = await page.eval(() => window.__probe.scopeSelection());
          const target = await page.eval(() => window.__probe.drillFirstCollapsed());
          if (!target) return ok(false, { before }, 'nothing collapsed on the canvas to drill into');
          await waitSettled(page);
          const w = await page.eval(() => window.__probe.wayback());
          const after = await page.eval(() => window.__probe.scopeSelection());
          const narrowed = JSON.stringify(after) !== JSON.stringify(before);
          const pass = narrowed && w.depth === 1 && w.backEnabled && !w.forwardEnabled;
          return ok(pass, { target, before, after, w },
            pass ? '' : 'the drill either did not narrow the scope or was not recorded');
        },
      },
      {
        id: 'the-button-names-where-it-leads',
        criterion: 'Back says which picture it goes to, not just "Back"',
        async run(page) {
          const w = await page.eval(() => window.__probe.wayback());
          const pass = /^Back to .+/.test(w.backTitle) && !/^Back to\s*$/.test(w.backTitle);
          return ok(pass, w.backTitle, pass ? '' : 'the control names a direction and no destination');
        },
      },
      {
        id: 'an-adjustment-is-not-a-navigation',
        criterion: 'Toggling a filter adds no frame to the stack',
        async run(page) {
          const before = await page.eval(() => window.__probe.wayback());
          // Node labels: a display toggle, on the far side of UI-082's line —
          // it changes how the picture looks, not which entities reach it.
          const clicked = await page.eval(() => window.__probe.clickText('Node Labels'));
          await sleep(400);
          const after = await page.eval(() => window.__probe.wayback());
          const pass = clicked && after.depth === before.depth;
          return ok(pass, { clicked, before: before.depth, after: after.depth },
            pass ? '' : 'an adjustment grew the history — back would now undo a checkbox');
        },
      },
      {
        id: 'back-returns-the-previous-picture',
        criterion: 'Back restores the scope the drill left',
        async run(page) {
          const drilled = await page.eval(() => window.__probe.scopeSelection());
          const moved = await page.eval(() => window.__probe.clickWayback('back'));
          await waitSettled(page);
          const now = await page.eval(() => window.__probe.scopeSelection());
          const w = await page.eval(() => window.__probe.wayback());
          const pass = moved && JSON.stringify(now) !== JSON.stringify(drilled)
            && w.depth === 0 && !w.backEnabled && w.forwardEnabled;
          return ok(pass, { drilled, now, w },
            pass ? '' : 'back did not restore the previous scope, or recorded itself');
        },
      },
      {
        id: 'forward-goes-back-in',
        criterion: 'Forward returns the picture back was pressed from',
        async run(page) {
          const before = await page.eval(() => window.__probe.scopeSelection());
          const moved = await page.eval(() => window.__probe.clickWayback('forward'));
          await waitSettled(page);
          const now = await page.eval(() => window.__probe.scopeSelection());
          const w = await page.eval(() => window.__probe.wayback());
          const pass = moved && JSON.stringify(now) !== JSON.stringify(before)
            && w.depth === 1 && !w.forwardEnabled;
          return ok(pass, { before, now, w },
            pass ? '' : 'forward did not return to the drilled picture');
        },
      },
      {
        id: 'a-new-navigation-drops-the-forward-stack',
        criterion: 'Stepping back and then drilling elsewhere makes forward unreachable',
        async run(page) {
          await page.eval(() => window.__probe.clickWayback('back'));
          await waitSettled(page);
          const mid = await page.eval(() => window.__probe.wayback());
          const target = await page.eval(() => window.__probe.drillFirstCollapsed());
          await waitSettled(page);
          const w = await page.eval(() => window.__probe.wayback());
          const pass = mid.forwardEnabled && !w.forwardEnabled && w.backEnabled;
          return ok(pass, { mid, after: w, target },
            pass ? '' : 'forward survived a new navigation and now points somewhere nobody was');
        },
      },
      {
        id: 'the-keys-work-from-anywhere',
        criterion: '[ and ] step the history whichever pane has focus',
        async run(page) {
          const before = await page.eval(() => window.__probe.wayback());
          if (!before.backEnabled) return ok(false, before, 'nothing to step back to');
          await page.key('0');   // focus the sidebar — the pane furthest from the canvas
          await page.key('[');
          await waitSettled(page);
          const after = await page.eval(() => window.__probe.wayback());
          const pass = after.depth === before.depth - 1 && after.forwardEnabled;
          return ok(pass, { before, after },
            pass ? '' : '`[` did nothing from the sidebar — the binding is not global');
        },
      },
    ],
  },

  'ui-095': {
    ticket: 'UI-095', title: 'The canvas says where you are',
    /**
     * The claims worth measuring are the ones where a strip could *lie*: an
     * address shown for a scope that has none, a crumb that looks clickable
     * and is where you already are, and a climb that silently keeps the
     * aggregation level the deep scope picked.
     */
    async setup(page) {
      await page.viewport(1600, 1000);
      await page.goto(APP); await ready(page);
      await page.eval((n) => window.__probe.pickScope(n), 'ui');
      await clearDiff(page);
      await waitSettled(page);
    },
    checks: [
      {
        id: 'the-address-is-rooted-and-deepest-last',
        criterion: 'A single-path scope reads as its ancestors, rooted at repo',
        async run(page) {
          const c = await page.eval(() => window.__probe.crumbs());
          if (!c) return ok(false, null, 'no address strip in the toolbar bar');
          const labels = c.parts.map((p) => p.label);
          const pass = labels[0] === 'repo' && labels[labels.length - 1] === 'ui';
          return ok(pass, c, pass ? '' : 'the strip does not name where the reader is');
        },
      },
      {
        id: 'where-you-are-is-not-a-link',
        criterion: 'The deepest crumb is not clickable — there is nowhere for it to go',
        async run(page) {
          const c = await page.eval(() => window.__probe.crumbs());
          const last = c.parts[c.parts.length - 1];
          const others = c.parts.slice(0, -1);
          const pass = last.current && !last.clickable && others.every((p) => p.clickable);
          return ok(pass, c.parts,
            pass ? '' : 'either the current crumb is a control or an ancestor is not');
        },
      },
      {
        id: 'a-crumb-climbs-and-is-undoable',
        criterion: 'Clicking an ancestor widens the scope and Back returns',
        /**
         * Drills first, so the climb under test is to a *middle* crumb — the
         * real case, and the one `repo` cannot stand in for. Climbing to the
         * root would also reload the whole repo, which is the heaviest thing
         * this suite could ask for and made it flake under load.
         *
         * It ends where it started, at `ui`, because the next check needs a
         * scope with two folders in it to mark from.
         */
        async run(page) {
          // Read the address, not the scope tree: a single-file scope renders
          // no checked row, so `scopeSelection` is `[]` both before and after
          // and would pass a Back that did nothing.
          const where = async () => {
            const c = await page.eval(() => window.__probe.crumbs());
            return c?.parts.map((p) => p.label).join('/') ?? null;
          };
          const target = await page.eval(() => window.__probe.drillFirstCollapsed());
          if (!target) return ok(false, null, 'nothing collapsed to drill into');
          await waitSettled(page);
          const drilled = await where();
          const depth0 = (await page.eval(() => window.__probe.wayback())).depth;

          const clicked = await page.eval(() => window.__probe.clickCrumb('ui'));
          await waitSettled(page);
          const climbed = await where();
          const depth1 = (await page.eval(() => window.__probe.wayback())).depth;

          await page.eval(() => window.__probe.clickWayback('back'));
          await waitSettled(page);
          const back = await where();
          // And back to where the check started, for whoever runs next.
          await page.eval(() => window.__probe.clickWayback('back'));
          await waitSettled(page);

          const pass = clicked && depth1 === depth0 + 1
            && climbed === 'repo/ui' && drilled !== climbed && back === drilled;
          return ok(pass, { target, drilled, climbed, back, depth0, depth1 },
            pass ? '' : 'the climb either did not move, was not recorded, or did not come back');
        },
      },
      {
        id: 'a-scope-with-no-address-is-not-given-one',
        criterion: 'A multi-path scope says how many paths, and offers no climb',
        async run(page) {
          // Mark two files in different folders and drill: the result has no
          // single folder, which is exactly the case a naive strip invents an
          // answer for.
          const marked = await page.eval(() => {
            const nodes = [...document.querySelectorAll('g.node')]
              .filter((g) => g.style.display !== 'none' && g.__data__?.file_path);
            const seen = new Set(); const picked = [];
            for (const g of nodes) {
              const dir = g.__data__.file_path.split('/').slice(0, -1).join('/');
              if (seen.has(dir)) continue;
              seen.add(dir); picked.push(g);
              if (picked.length === 2) break;
            }
            for (const g of picked) {
              g.dispatchEvent(new MouseEvent('click', { bubbles: true, metaKey: true }));
            }
            return picked.length;
          });
          if (marked < 2) return ok(false, { marked }, 'could not mark two files in different folders');
          await page.eval(() => window.__probe.clickText('Drill into'));
          await waitSettled(page);
          const c = await page.eval(() => window.__probe.crumbs());
          const pass = c && c.parts.length === 1 && !c.parts[0].clickable
            && /\d+ paths/.test(c.parts[0].label);
          return ok(pass, c, pass ? '' : 'the strip invented a folder for a scope that has none');
        },
      },
      {
        id: 'the-strip-does-not-crowd-the-counts',
        criterion: 'At 1280 the address never overlaps the counts',
        async run(page) {
          await page.viewport(1280, 800);
          await sleep(600);
          const hit = await page.eval(() => window.__probe.overlap('scope-crumbs', 'stats-bar'));
          const crumbBox = await page.eval(() => window.__probe.box('scope-crumbs'));
          await page.viewport(1600, 1000);
          await sleep(400);
          return ok(hit === null, { overlap: hit, crumbBox },
            hit === null ? '' : 'the address strip pushed into the counts');
        },
      },
    ],
  },
};

// ── Diff fixture helpers (ui-085 / ui-086) ───────────────────────────────
//
// Read from the engine rather than written down here: which entity changed
// is a property of the working tree the probe happens to run against, and a
// name hard-coded in this file would go stale on the next commit.

let engineDiffCache = null;
async function engineDiff() {
  if (!engineDiffCache) {
    const resp = await fetch(`${ENGINE}/api/diff`);
    if (!resp.ok) throw new Error(`no diff on ${ENGINE} — compute one first`);
    engineDiffCache = await resp.json();
  }
  return engineDiffCache;
}

/** The detail sidecars, head and base. Fetched because "modified" in the
 *  diff does not imply "has source on both sides" — a Svelte component
 *  entity, for instance, carries none — and a subject with nothing to show
 *  would fail these checks for a reason that is not the one under test. */
let sidecarCache = null;
async function sidecars() {
  if (!sidecarCache) {
    const [head, base] = await Promise.all([
      fetch(`${ENGINE}/api/details`).then((r) => r.json()),
      fetch(`${ENGINE}/api/details/base`).then((r) => r.json()),
    ]);
    sidecarCache = { head, base };
  }
  return sidecarCache;
}

/** A modified entity whose own source moved and whose text exists on both
 *  sides, preferring the biggest edit — a one-line change would leave the
 *  folding checks with nothing to fold. */
async function changedSubject() {
  const d = await engineDiff();
  const { head, base } = await sidecars();
  const loc = (e) => Math.abs(e.metric_deltas?.find((m) => m.name === 'loc')?.delta ?? 0);
  return d.entities
    .filter((e) => e.status === 'modified' && e.source_changed && e.base_entity_id)
    .filter((e) => head[e.entity_id]?.source_code && base[e.base_entity_id]?.source_code)
    .sort((a, b) => loc(b) - loc(a))[0] ?? null;
}

/** A file the diff changed, with its text on both sides — the fixture for
 *  the file-level checks. */
async function changedFile() {
  const subject = await changedSubject();
  if (!subject) return null;
  const { head, base } = await sidecars();
  const path = subject.file_path;
  if (!head[path]?.source_code || !base[path]?.source_code) return null;
  return { path, name: path.split('/').pop() };
}

/** The top-level scope holding a path — what the scope tree offers, rather
 *  than a directory written down in this file. `src` was hard-coded here and
 *  the suite went red the day the working tree's edits landed under `ui/`. */
function topScopeOf(filePath) {
  const i = filePath.indexOf('/');
  return i < 0 ? filePath : filePath.slice(0, i);
}

/** The entity the diff has the most to say about, edge-wise. */
async function rewiredSubject() {
  const d = await engineDiff();
  return d.entities
    .filter((e) => (e.rel_deltas?.length ?? 0) > 0)
    .sort((a, b) => b.rel_deltas.length - a.rel_deltas.length)[0] ?? null;
}

/**
 * Get the app to a state where `path` is the analysis scope.
 *
 * Two pieces of leaked state to undo first, both persisted and both of which
 * turn every later assertion into a measurement of an empty app:
 *
 *  - The sidebar tab. On `quality`, `.sidebar-top` holds the Quality panel and
 *    the scope tree is not in the DOM at all, so `pickScope` silently finds
 *    nothing. Any earlier suite that opened Quality leaves it that way.
 *  - The scope itself. The row *toggles* and survives a reload, so an
 *    unguarded pick clears a scope a previous run selected.
 */
async function ensureScope(page, path) {
  await page.eval(() => window.__probe.pickTab('Filters'));
  await sleep(500);
  const st = await page.eval((p) => window.__probe.scopeRowState(p), path);
  if (!st?.checked) await page.eval((n) => window.__probe.pickScope(n), path);
  await sleep(6000);
  return st;
}

/** Search for an entity and pin it, disambiguating by file path. */
async function selectByName(page, name, file) {
  await page.eval((n) => window.__probe.searchFor(n), name);
  await sleep(1500);
  const picked = await page.eval(([n, f]) => window.__probe.searchPick(n, f), [name, file]);
  // The detail sidecar (source, fields) loads after the selection lands.
  await sleep(2000);
  return picked;
}

// ── CLI ──────────────────────────────────────────────────────────────────

const argv = process.argv.slice(2);
const keepOpen = argv.includes('--keep-open');
const verbose = argv.includes('--verbose');
const wanted = argv.includes('--all')
  ? Object.keys(SUITES)
  : argv.filter((a) => !a.startsWith('--')).map((a) => a.toLowerCase());

if (!wanted.length) {
  console.log(`ux-probe — layout assertions for the nao web UI

usage: node ui/scripts/ux-probe.mjs <suite...> | --all [--keep-open] [--verbose]

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
      // A passing check's measurement is worth reading too — it is what you
      // quote when writing the ticket up, and re-deriving it later means
      // re-running the whole suite with the assertion inverted.
      if (!res.pass || verbose) {
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
