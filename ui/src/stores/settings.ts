import { writable } from 'svelte/store';
import { COHESION_LEVELS, GROUP_GRAINS, type CohesionLevel, type GroupGrain } from '../utils/forceCohesion';
import { HUB_COUNTS, DEFAULT_HUB_COUNT } from '../viewmodels/hubs';
import {
  SIZE_CURVE_IDS, SIZE_BOOST_DEFAULT, SIZE_BINS_OFF, clampBoost, clampBins,
  type SizeCurveId,
} from '../viewmodels/sizeCurve';

export type ThemeId = 'llvator' | 'obsidian' | 'nord' | 'light' | 'midnight';

export interface ThemeDefinition {
  id: ThemeId;
  name: string;
  description: string;
  colors: {
    bgBody: string;
    bgSurface: string;
    bgSurfaceAlt: string;
    bgHover: string;
    bgDeep: string;
    border: string;
    borderSubtle: string;
    accent: string;
    /** Text drawn *on* an accent fill — active toolbar buttons, primary
     *  submit buttons. Separate from `text` because a light accent needs
     *  dark ink and a dark accent needs light: the old `color: white`
     *  literal reads at 1.99:1 over Nord's #88c0d0. */
    accentFg: string;
    text: string;
    textSecondary: string;
    textMuted: string;
    textDim: string;
    textDisabled: string;
    dangerFg: string;
    tierOkFg: string;
    tierWarnFg: string;
    tierBadFg: string;
  };
}

// Order is the order the theme picker renders, and the first entry is the
// product default (see `loadTheme`). `llvator` leads because the hosted
// visualizer, the VS Code webview and llvator.com should all open looking
// like one product.
export const THEMES: ThemeDefinition[] = [
  {
    id: 'llvator',
    name: 'LLvator',
    description: 'Amber on near-black — matches llvator.com',
    // Palette lifted from the site's design tokens so the hosted visualizer
    // and the pages around it read as one product. Unlike the four other
    // themes (see UI-024), every informational token here was chosen to clear
    // WCAG AA 4.5:1 against both --bg-surface and --bg-body; the measured
    // ratio is noted per token so a later tweak can't quietly drop below.
    //
    // `ux-probe.mjs ui-023` scores it at 5 failing pairs against 45–58 for
    // the others: 3 are --text-disabled (exempt, see below) and 2 are
    // --text-dim over ScopeTree's hardcoded rgba(33,150,243,.25) selection
    // fill, which is a component literal rather than a palette value (UI-025).
    colors: {
      bgBody: '#0a0c0f',
      bgSurface: '#12161c',
      bgSurfaceAlt: '#0e1116',
      // Deliberately close to bgSurface: hover has to read as a state change
      // without lifting the background far enough to sink --text-dim.
      bgHover: '#19212a',
      bgDeep: '#07090c',
      border: '#232a34',
      borderSubtle: '#1a2029',
      accent: '#ffc107', // 11.1:1 on bgSurface
      accentFg: '#241a00', // 10.5:1 on the accent fill
      text: '#e8ecf1', // 15.3:1
      textSecondary: '#aeb8c4', // 9.0:1
      textMuted: '#a3adba', // 8.0:1
      // The token UI-024 found failing in every theme. Set high enough to
      // clear 4.5:1 not just on the flat surfaces but on the tinted metric
      // tiles too (4.8:1 over the amber tint, 5.3:1 over the green), which
      // is where the other four palettes lose it.
      textDim: '#8e99a7', // 6.3:1
      // Below the bar on purpose, and the only one: WCAG exempts inactive
      // controls, and disabled text must read as unavailable. Never used for
      // information the reader needs.
      textDisabled: '#5a636e', // 3.0:1
      dangerFg: '#ff8a80', // 8.0:1
      tierOkFg: '#A5D6A7',
      tierWarnFg: '#FFCC80',
      tierBadFg: '#EF9A9A',
    },
  },
  {
    id: 'obsidian',
    name: 'Obsidian',
    description: 'Pure dark with violet accents',
    colors: {
      bgBody: '#141414',
      bgSurface: '#1e1e1e',
      bgSurfaceAlt: '#181818',
      bgHover: '#2a2a2a',
      bgDeep: '#0e0e0e',
      border: '#333333',
      borderSubtle: '#444444',
      accent: '#7c4dff',
      accentFg: '#ffffff',
      text: '#e0e0e0',
      textSecondary: '#c0c0c0',
      textMuted: '#999999',
      textDim: '#777777',
      textDisabled: '#555555',
      dangerFg: '#ff8a80',
      tierOkFg: '#A5D6A7',
      tierWarnFg: '#FFCC80',
      tierBadFg: '#EF9A9A',
    },
  },
  {
    id: 'nord',
    name: 'Nord',
    description: 'Arctic blue-grey palette',
    colors: {
      bgBody: '#2e3440',
      bgSurface: '#3b4252',
      bgSurfaceAlt: '#353b48',
      bgHover: '#434c5e',
      bgDeep: '#272c36',
      border: '#4c566a',
      borderSubtle: '#5c6477',
      accent: '#88c0d0',
      // Nord's accent is a pale arctic blue; white on it measures 2.0:1.
      // Dark ink takes the same fill to 7.9:1 (UI-024 territory, but this
      // token is new so there is no previous value to preserve).
      accentFg: '#1b232e',
      text: '#eceff4',
      textSecondary: '#d8dee9',
      textMuted: '#a0aab5',
      textDim: '#7b8794',
      textDisabled: '#616e7c',
      dangerFg: '#ffa8a8',
      tierOkFg: '#B5DDB7',
      tierWarnFg: '#FFD9A0',
      tierBadFg: '#F5AFAF',
    },
  },
  {
    id: 'light',
    name: 'Light',
    description: 'Clean light theme',
    colors: {
      bgBody: '#f5f6fa',
      bgSurface: '#ffffff',
      bgSurfaceAlt: '#f0f1f5',
      bgHover: '#e2e5ed',
      bgDeep: '#e8eaf0',
      border: '#d0d5dd',
      borderSubtle: '#c0c5cc',
      accent: '#d63d57',
      accentFg: '#ffffff',
      text: '#1e2129',
      textSecondary: '#333a45',
      textMuted: '#5c6370',
      textDim: '#7d8590',
      textDisabled: '#a0a5ae',
      dangerFg: '#b3261e',
      tierOkFg: '#1B5E20',
      tierWarnFg: '#8A4B00',
      tierBadFg: '#B3261E',
    },
  },
  {
    id: 'midnight',
    name: 'Midnight',
    description: 'Deep blue-purple dark theme',
    colors: {
      bgBody: '#1a1a2e',
      bgSurface: '#16213e',
      bgSurfaceAlt: '#161625',
      bgHover: '#0f3460',
      bgDeep: '#0d1117',
      border: '#0f3460',
      borderSubtle: '#30363d',
      accent: '#e94560',
      accentFg: '#ffffff',
      text: '#eeeeee',
      textSecondary: '#c9d1d9',
      textMuted: '#aaaaaa',
      textDim: '#888888',
      textDisabled: '#666666',
      dangerFg: '#ff8a8a',
      tierOkFg: '#A5D6A7',
      tierWarnFg: '#FFCC80',
      tierBadFg: '#EF9A9A',
    },
  },
];

// ── Auto-fit view ───────────────────────────────────────────────────────
const AUTO_FIT_KEY = 'nao-auto-fit';

function loadAutoFit(): boolean {
  try {
    const v = localStorage.getItem(AUTO_FIT_KEY);
    // Absent means "never chosen" and gets the default; "false" is an
    // explicit opt-out and must survive. Conflating the two is exactly what
    // would defeat the default below.
    if (v !== null) return v === 'true';
  } catch { /* SSR / blocked storage */ }
  // Default ON. Off-by-default meant the first graph a user ever saw was
  // clipped on every edge, with the remedy buried among sixteen identical
  // toolbar buttons (UI-022).
  return true;
}

/** When enabled, the viewport automatically calls fitView() after every
 *  graph/tree layout transition (view mode switch, selection change, etc.). */
export const autoFitView = writable<boolean>(loadAutoFit());

autoFitView.subscribe((v) => {
  try { localStorage.setItem(AUTO_FIT_KEY, String(v)); } catch { /* ignore */ }
});

// ── Node encoding channels (UI-014) ─────────────────────────────────────
//
// Which metric drives node size, and what node fill means. Persisted like
// every other view setting so a chosen encoding survives a reload.
//
// The defaults are the product decision the ticket was gated on: size = LOC
// (the one magnitude metric that rolls up meaningfully at entity, file AND
// module level), fill = severity (the composite score the Quality panel
// already ranks by). `colour: kind` reproduces the pre-UI-014 palette.
const SIZE_CHANNEL_KEY = 'nao-size-channel';
const COLOR_CHANNEL_KEY = 'nao-color-channel';

function loadChannel<T extends string>(key: string, valid: readonly T[], fallback: T): T {
  try {
    const v = localStorage.getItem(key);
    if (v && (valid as readonly string[]).includes(v)) return v as T;
  } catch { /* SSR / blocked storage */ }
  return fallback;
}

const SIZE_CHANNEL_IDS = ['loc', 'churn', 'degree', 'coupling', 'methodCount', 'wmc', 'cyclomatic', 'pagerank', 'kind'] as const;
const COLOR_CHANNEL_IDS = ['severity', 'kind'] as const;

export type SizeChannelId = typeof SIZE_CHANNEL_IDS[number];
export type ColorChannelId = typeof COLOR_CHANNEL_IDS[number];

export const sizeChannel = writable<SizeChannelId>(
  loadChannel(SIZE_CHANNEL_KEY, SIZE_CHANNEL_IDS, 'loc'),
);
export const colorChannel = writable<ColorChannelId>(
  loadChannel(COLOR_CHANNEL_KEY, COLOR_CHANNEL_IDS, 'severity'),
);

sizeChannel.subscribe((v) => {
  try { localStorage.setItem(SIZE_CHANNEL_KEY, v); } catch { /* ignore */ }
});
colorChannel.subscribe((v) => {
  try { localStorage.setItem(COLOR_CHANNEL_KEY, v); } catch { /* ignore */ }
});

// ── Node size shape and scale (UI-106) ──────────────────────────────────
//
// Two more knobs on the same channel: `sizeCurve` decides how the value range
// is distributed over the radius range, `sizeBoost` decides how wide that
// radius range is. Both were constants — √ over 7–34px — and both were the
// reason a skewed repo drew nine hundred near-identical circles.
//
// Defaults reproduce the pre-UI-106 picture exactly (`area`, ×1): the old
// behaviour was a reasonable default and a bad ceiling, so it stays the
// starting point and stops being the only point.
const SIZE_CURVE_KEY = 'nao-size-curve';
const SIZE_BOOST_KEY = 'nao-size-boost';

export const sizeCurve = writable<SizeCurveId>(
  loadChannel(SIZE_CURVE_KEY, SIZE_CURVE_IDS, 'area'),
);

function loadBoost(): number {
  try {
    const v = localStorage.getItem(SIZE_BOOST_KEY);
    // `clampBoost` also absorbs the NaN a hand-edited or half-written value
    // would produce, so a corrupt entry falls back to ×1 rather than sizing
    // every node NaN and drawing an empty canvas.
    if (v !== null) return clampBoost(Number(v));
  } catch { /* SSR / blocked storage */ }
  return SIZE_BOOST_DEFAULT;
}

export const sizeBoost = writable<number>(loadBoost());

sizeCurve.subscribe((v) => {
  try { localStorage.setItem(SIZE_CURVE_KEY, v); } catch { /* ignore */ }
});
sizeBoost.subscribe((v) => {
  try { localStorage.setItem(SIZE_BOOST_KEY, String(v)); } catch { /* ignore */ }
});

// ── Size groups (UI-110) ────────────────────────────────────────────────
//
// How many size classes the canvas draws, or `SIZE_BINS_OFF` for the
// continuous scale the curve and the scale both assume.
//
// Continuous is the default and stays the default: it is what every earlier
// measurement and screenshot is of, it discards nothing, and grouping is a
// deliberate trade — within-group differences for an answerable "which group
// is this in". A reader who wants classes asks for them.
const SIZE_BINS_KEY = 'nao-size-bins';

function loadBins(): number {
  try {
    const v = localStorage.getItem(SIZE_BINS_KEY);
    // Same shape as `loadBoost`: `clampBins` absorbs NaN and out-of-range,
    // and lands on continuous — the mode that cannot mislead.
    if (v !== null) return clampBins(Number(v));
  } catch { /* SSR / blocked storage */ }
  return SIZE_BINS_OFF;
}

export const sizeBins = writable<number>(loadBins());

sizeBins.subscribe((v) => {
  try { localStorage.setItem(SIZE_BINS_KEY, String(v)); } catch { /* ignore */ }
});

// ── Folder cohesion (UI-052) ────────────────────────────────────────────
//
// How hard the folder tree pulls against the call graph in the force layout.
// A view setting like the two above, and persisted for the same reason: a
// reader who has tuned the picture should not have to re-tune it on reload.
//
// Default is `low`, not `off`. Off-by-default would mean the first graph
// anyone ever sees is the hairball this force exists to break up, with the
// remedy behind a control they have no reason to look for — the same mistake
// UI-022 fixed for auto-fit. Low is enough to make groups visible without
// overriding what the relationships say.
const COHESION_KEY = 'nao-folder-cohesion';

export const folderCohesion = writable<CohesionLevel>(
  loadChannel(COHESION_KEY, COHESION_LEVELS, 'low'),
);

folderCohesion.subscribe((v) => {
  try { localStorage.setItem(COHESION_KEY, v); } catch { /* ignore */ }
});

// ── Folder hulls (UI-055) ───────────────────────────────────────────────
//
// Outline and name each folder region on the canvas. On by default: naming
// the regions is the whole reason the grouping work is visible at all, and a
// reader who has to discover a toggle to find out what they are looking at
// has not been told.
const HULLS_KEY = 'nao-folder-hulls';

function loadHulls(): boolean {
  try {
    const v = localStorage.getItem(HULLS_KEY);
    if (v !== null) return v === 'true';
  } catch { /* SSR / blocked storage */ }
  return true;
}

export const showFolderHulls = writable<boolean>(loadHulls());

showFolderHulls.subscribe((v) => {
  try { localStorage.setItem(HULLS_KEY, String(v)); } catch { /* ignore */ }
});

// ── Group grain (UI-103) ────────────────────────────────────────────────
//
// Which level of the declared tree is the innermost group: the folder, or
// the file inside it. Not a filter and not an aggregation level — the same
// nodes are drawn either way, grouped differently.
//
// Default `folder`, unlike cohesion's non-off default. The argument there
// was that the first graph anyone sees should not be the hairball the force
// exists to break up; here the folder grain *is* the established reading of
// every screenshot and every measurement, and the file grain answers a
// narrower question a reader has to be looking for.
//
// Deliberately not part of `ViewState`: a saved view holds what decides
// which entities reach the canvas, and cohesion and hull depth are both
// absent for the same reason. See `f.window_mirror` on why layout stays
// local.
const GROUP_GRAIN_KEY = 'nao-group-grain';

export const groupGrain = writable<GroupGrain>(
  loadChannel(GROUP_GRAIN_KEY, GROUP_GRAINS, 'folder'),
);

groupGrain.subscribe((v) => {
  try { localStorage.setItem(GROUP_GRAIN_KEY, v); } catch { /* ignore */ }
});

// ── Region tiers (UI-070) ───────────────────────────────────────────────
//
// How many levels of the folder tree get an outline. 1 is the UI-055
// picture — a region per leaf folder and nothing above it — and each step
// adds the tier above.
//
// Default 2, because one tier cannot say the thing the reader is missing:
// three sibling regions with no shape around them are three neighbourhoods
// with no district, and the canvas ends up a weaker account of the repo than
// the file tree in the sidebar. It is capped at 3 rather than left open:
// past that the outlines are nested closer than the eye separates them, and
// every tier costs a polygon per group per redraw.
const HULL_DEPTH_KEY = 'nao-hull-depth';

export const HULL_DEPTHS: readonly number[] = [1, 2, 3];

/**
 * What each tier setting is called, which depends on the grain (UI-103).
 *
 * `tiers` counts upward from the *innermost* region, so the same number means
 * a different thing once the file is the innermost one. Labelling depth 1
 * "Folders" at file grain would name the tier above the one it draws — and
 * that mislabelling is the reason grain is a second control rather than a
 * fourth value in `HULL_DEPTHS`.
 */
export function hullDepthLabels(grain: GroupGrain): Record<number, string> {
  return grain === 'file'
    ? { 1: 'Files', 2: '+ Folder', 3: '+ Parent' }
    : { 1: 'Folders', 2: '+ Parent', 3: '+ Grandparent' };
}

function loadHullDepth(): number {
  try {
    const v = Number(localStorage.getItem(HULL_DEPTH_KEY));
    if (HULL_DEPTHS.includes(v)) return v;
  } catch { /* SSR / blocked storage */ }
  return 2;
}

export const hullDepth = writable<number>(loadHullDepth());

hullDepth.subscribe((v) => {
  try { localStorage.setItem(HULL_DEPTH_KEY, String(v)); } catch { /* ignore */ }
});

// ── Hub demotion (UI-056) ───────────────────────────────────────────────
//
// Off by default. Unlike cohesion and hulls, which change how the same graph
// is arranged and annotated, this one *removes edges from the picture* —
// and a tool that quietly hides relationships on first run has misled the
// reader before they have had a chance to ask for it.
const DEMOTE_KEY = 'nao-demote-hubs';
const HUB_COUNT_KEY = 'nao-hub-count';

function loadDemote(): boolean {
  try { return localStorage.getItem(DEMOTE_KEY) === 'true'; } catch { return false; }
}

function loadHubCount(): number {
  try {
    const v = Number(localStorage.getItem(HUB_COUNT_KEY));
    if (HUB_COUNTS.includes(v)) return v;
  } catch { /* SSR / blocked storage */ }
  return DEFAULT_HUB_COUNT;
}

export const demoteHubs = writable<boolean>(loadDemote());
export const hubCount = writable<number>(loadHubCount());

demoteHubs.subscribe((v) => {
  try { localStorage.setItem(DEMOTE_KEY, String(v)); } catch { /* ignore */ }
});
hubCount.subscribe((v) => {
  try { localStorage.setItem(HUB_COUNT_KEY, String(v)); } catch { /* ignore */ }
});

// ── Theme ───────────────────────────────────────────────────────────────
const STORAGE_KEY = 'nao-theme';

function loadTheme(): ThemeId {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored && THEMES.some((t) => t.id === stored)) return stored as ThemeId;
  } catch { /* SSR / blocked storage */ }
  // A stored id always wins — this is the first-run default only.
  return 'llvator';
}

export const activeTheme = writable<ThemeId>(loadTheme());

// Persist to localStorage on change.
activeTheme.subscribe((id) => {
  try { localStorage.setItem(STORAGE_KEY, id); } catch { /* ignore */ }
});

/** Apply the theme's CSS custom properties to the document root. */
export function applyTheme(id: ThemeId): void {
  const theme = THEMES.find((t) => t.id === id);
  if (!theme) return;
  const root = document.documentElement;
  root.setAttribute('data-theme', id);
  const c = theme.colors;
  root.style.setProperty('--bg-body', c.bgBody);
  root.style.setProperty('--bg-surface', c.bgSurface);
  root.style.setProperty('--bg-surface-alt', c.bgSurfaceAlt);
  root.style.setProperty('--bg-hover', c.bgHover);
  root.style.setProperty('--bg-deep', c.bgDeep);
  root.style.setProperty('--border', c.border);
  root.style.setProperty('--border-subtle', c.borderSubtle);
  root.style.setProperty('--accent', c.accent);
  root.style.setProperty('--accent-fg', c.accentFg);
  root.style.setProperty('--text', c.text);
  root.style.setProperty('--text-secondary', c.textSecondary);
  root.style.setProperty('--text-muted', c.textMuted);
  root.style.setProperty('--text-dim', c.textDim);
  root.style.setProperty('--text-disabled', c.textDisabled);
  root.style.setProperty('--danger-fg', c.dangerFg);
  root.style.setProperty('--tier-ok-fg', c.tierOkFg);
  root.style.setProperty('--tier-warn-fg', c.tierWarnFg);
  root.style.setProperty('--tier-bad-fg', c.tierBadFg);
}
