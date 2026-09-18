/**
 * Two windows of the app, kept on the same reading (UI-095).
 *
 * The rules for *what* to apply live in `viewmodels/mirror.ts`; this module is
 * the four things that need the stores and the browser: a channel to talk on,
 * reading the picture out, writing one back, and knowing whether anyone is
 * listening.
 *
 * Shaped after `viewHistory.ts`, and for the same reason — UI-082 already
 * built the codec, so a second consumer of it is a channel plus a decision,
 * not a new notion of what a picture is.
 *
 * **`BroadcastChannel`, so: same browser, same profile, same machine.** That
 * is the whole delivery mechanism, and it is enough for what this is for —
 * two monitors on one desk. Two machines, or Chrome talking to Firefox, would
 * need a relay through the watch server, which is a different feature and not
 * this one. The peer count exists so that limit is visible rather than
 * discovered: a mirror with nobody on the other end says so.
 *
 * **Not persisted beyond the switch.** Whether the mode is on survives a
 * reload, because opening the second window *is* the gesture and it should
 * come up already mirroring. Nothing else here does: peers are whoever is
 * currently listening, and a sequence number that outlived its window would
 * make every message from the new one look stale.
 */

import { get, writable } from 'svelte/store';
import type { Readable } from 'svelte/store';
import { currentState, restoreState } from './savedViews';
import { withoutNavigation } from './scope';
import { graphData, selectedNode, hoveredNode, hoverLocked, viewMode } from './graph';
import { specGraph, specPath, specHoverId, specPinnedHighlight } from './crossFilter';
import { changedFiles, selectedChangedFile } from './changedFiles';
import type { D3Node } from '../types/graph';
import { serveRepo } from '../vscodeAdapter';
import { persistedFlag } from './panes';
import {
  channelName, emptyAttention, hoverToShow, isFresh, normalizeMessage, whatToApply,
  type Attention, type MirrorMessage,
} from '../viewmodels/mirror';

/** Whether this window is publishing and listening. */
export const mirrorOn = persistedFlag('mezz-mirror-on', false);

/** How many other windows are on the channel. Zero with the mode on is the
 *  failure worth seeing — a mirror nobody is on the other end of. */
const peers = writable(0);
export const mirrorPeers: Readable<number> = peers;

/**
 * This window's name on the channel, fixed for its lifetime.
 *
 * Only ever compared for equality, so its only requirement is not colliding
 * with the window next to it — `randomUUID` where it exists, and a random
 * suffix where it does not (older WebViews, and `crypto.randomUUID` is
 * secure-context-only, which `http://localhost` satisfies but a LAN address
 * over plain HTTP does not).
 */
const ORIGIN = globalThis.crypto?.randomUUID?.() ?? `w-${Math.random().toString(36).slice(2)}`;

/** Bumped per message, so a payload that arrives behind a fresher one from the
 *  same window can be dropped without inspecting it. */
let seq = 0;

/** The last sequence number accepted from each window. */
let seen: Record<string, number> = {};

/**
 * Set while a remote update is being written to the stores.
 *
 * Without it the write would be heard by our own publisher and sent straight
 * back. The peer would work out that it changed nothing — `whatToApply`
 * returns `none` for a state it already has — so this is not what makes the
 * loop terminate. What it prevents is the round of traffic and, more to the
 * point, a slow `restoreState` on one side racing its own echo.
 */
let applying = false;

let channel: BroadcastChannel | null = null;
let unsubscribe: (() => void) | null = null;
let pending: ReturnType<typeof setTimeout> | null = null;

/** Long enough to coalesce a drag across a filter list into one message, short
 *  enough that a click feels immediate on the other screen. */
const DEBOUNCE_MS = 80;

/** The peer's last word on where it is standing, kept because the local
 *  pointer leaving is a reason to re-read it without a message arriving. */
let peerAttention: Attention = emptyAttention();

/** The remote hover this window last took on. See `hoverToShow` for why
 *  knowing this is what makes "local wins" decidable at all. */
let adoptedHover: string | null = null;

/** The remote hover a local selection refused, retired as soon as the peer's
 *  pointer moves off it. See `hoverToShow`. */
let declinedHover: string | null = null;

/**
 * The remote *spec* hover this window last took on.
 *
 * The second pointer needs its own memory for the reason the first one does:
 * `specHoverId` cannot say whether the entity it names got there from this
 * window's pointer or the peer's, and "local wins" is undecidable without
 * that. Kept apart from `adoptedHover` because the two pointers are genuinely
 * independent — a reader can be sweeping the spec pane here while the peer
 * sweeps the code canvas there, and one memory would have each cancel the
 * other.
 */
let adoptedSpecHover: string | null = null;

/**
 * A file the peer has open that this window could not name yet.
 *
 * The Changes list is fetched, and the message that carries a *reading* also
 * carries the comparison it belongs to — so a peer's open file routinely
 * arrives a beat before the list that could resolve it. Remembered rather
 * than dropped: `settleFile` is retried when the list lands, and nothing else
 * announces that moment.
 */
let pendingFile: string | null = null;

/**
 * The entity an id names, in either pane.
 *
 * Two universes, because the split view draws two subgraphs of one analysis and
 * only one of them is in `graphData`. The spec pane is cut from the *unscoped*
 * graph on purpose (`crossFilter.ts`: it is the control surface for the code
 * pane's scope, so narrowing the scope must not erase the map you narrow with),
 * and `graphData` is the scoped graph *collapsed to the level now being drawn*.
 * So a spec entity resolves there only when its `.elv` happens to be in scope
 * and the level happens to be `entity` — and when it did not, the peer window
 * answered a mirrored spec selection by clearing its own, which is the
 * "Details went blank on the other screen" half of this bug.
 *
 * `specGraph` rather than `visibleSpecGraph`: the "follow the analysis scope"
 * toggle is per-window layout, like the pane widths, and a window with it on
 * should still be able to name what its peer selected.
 */
function nodeById(id: string | null): D3Node | null {
  if (!id) return null;
  return get(graphData).nodes.find((n) => n.id === id)
    ?? get(specGraph).nodes.find((n) => n.id === id)
    ?? null;
}

/** What this window is looking at, right now. */
function snapshot(): MirrorMessage {
  return {
    origin: ORIGIN,
    seq,
    state: get(currentState),
    attention: {
      selected: get(selectedNode)?.id ?? null,
      hovered: get(hoveredNode)?.id ?? null,
      file: get(selectedChangedFile)?.path ?? null,
      hoverLocked: get(hoverLocked),
      mode: get(viewMode),
      specPath: [...get(specPath)],
      specHover: get(specHoverId),
      specPinned: [...get(specPinnedHighlight)],
    },
  };
}

function publish(): void {
  if (!channel || applying) return;
  seq += 1;
  channel.postMessage({ kind: 'state', ...snapshot() });
}

function schedulePublish(): void {
  if (!channel || applying) return;
  if (pending) clearTimeout(pending);
  pending = setTimeout(() => { pending = null; publish(); }, DEBOUNCE_MS);
}

/**
 * Move this window onto what the message describes.
 *
 * `withoutNavigation` is load-bearing, exactly as it is in `viewHistory.step`:
 * `restoreState` writes the same stores every gesture writes, and without it
 * every change made in the *other* window would land in this one's back stack.
 * A history that fills up with somewhere you never went is worse than no
 * history, and the two windows would each be recording the other's steps.
 */
async function apply(msg: MirrorMessage): Promise<void> {
  applying = true;
  try {
    const action = whatToApply(msg, snapshot());
    if (action === 'none') return;
    // A difference the reader made *within* the same reading deliberately
    // skips the republish: the picture is already the one on screen, and
    // `restoreState` would refetch the graph to arrive back where it started.
    if (action === 'all') await withoutNavigation(() => restoreState(msg.state));
    peerAttention = msg.attention;
    viewMode.set(msg.attention.mode);
    specPath.set([...msg.attention.specPath]);
    // A plain assignment, like the selection and unlike either hover: pinning
    // is a click, and a click is the newest gesture whichever window made it.
    // No "local wins" rule to apply, so no memory of what was adopted.
    specPinnedHighlight.set(new Set(msg.attention.specPinned));
    selectedNode.set(nodeById(msg.attention.selected));
    // After the selection, never before it: selecting a node clears whatever
    // file the Changes tab had open unless it is that file's own node
    // (`stores/changedFiles`), so the other order would have this window
    // undo the message it is in the middle of applying.
    settleFile(msg.attention.file);
    hoverLocked.set(msg.attention.hoverLocked);
    settleHover();
    settleSpecHover();
  } finally {
    applying = false;
  }
}

/**
 * Open, here, the file the peer opened there (UI-142).
 *
 * Unlike the hover there is no "local wins" rule to apply: opening a row is a
 * click, and a click is the newest gesture whichever window made it — the same
 * way `selected` already travels. So this is a plain assignment, and the only
 * thing it has to be careful about is naming the row.
 *
 * It names it out of *this* window's list rather than trusting the path, which
 * is what keeps a mirrored file honest: `FileDiff` fetches both sides against
 * the comparison this window has loaded, and the comparison is not part of
 * `ViewState`. A path this window's own list does not hold is one it cannot
 * render, so it waits for a list that does instead of opening the wrong pair.
 */
function settleFile(want: string | null): void {
  pendingFile = null;
  if (!want) {
    selectedChangedFile.set(null);
    return;
  }
  if (get(selectedChangedFile)?.path === want) return;
  const row = get(changedFiles)?.find((f) => f.path === want) ?? null;
  if (row) selectedChangedFile.set(row);
  else pendingFile = want;
}

/**
 * Put this window's hover where the two pointers agree it belongs.
 *
 * Called both when the peer speaks and when this window's own hover clears,
 * because "the local pointer left, so the peer's hover applies again" is a
 * transition no message announces.
 */
function settleHover(): void {
  const mine = get(hoveredNode)?.id ?? null;
  const want = hoverToShow(mine, peerAttention.hovered, adoptedHover, declinedHover);
  // The peer moving on retires the refusal. What a local click turns down is
  // the one hover that was standing when it happened, not the peer's pointer
  // as such — a block that outlived the entity would be the mirror quietly
  // switching itself off.
  if (peerAttention.hovered !== declinedHover) declinedHover = null;
  if (want === mine) return;
  hoveredNode.set(nodeById(want));
  adoptedHover = want;
}

/**
 * The same reconcile for the spec pane's pointer.
 *
 * Separate from `settleHover` rather than folded into it, because the two
 * pointers move independently and the stores they write are different: one
 * lands in `hoveredNode` and feeds the Details and Description panes, the
 * other lands in `specHoverId` and lights rings on the code canvas. Sharing
 * one function would mean sharing one `adopted` memory, and a reader sweeping
 * the spec pane here while the peer sweeps the canvas there would have each
 * pointer cancelling the other's.
 *
 * An **id and not a node**, unlike `settleHover`: the spec hover is spent by
 * `specHighlightPaths`, which resolves it against the spec graph's `claims`
 * map itself. Resolving to a node here would put it through `nodeById`, whose
 * `graphData` leg is the *scoped, collapsed* code graph — so the peer's
 * Feature would fail to resolve in exactly the window that has narrowed its
 * scope, which is the window most likely to want the ring.
 */
function settleSpecHover(): void {
  const mine = get(specHoverId);
  const want = hoverToShow(mine, peerAttention.specHover, adoptedSpecHover);
  if (want === mine) return;
  specHoverId.set(want);
  adoptedSpecHover = want;
}

/**
 * Give back a ring this window borrowed, when there is nobody left to give it
 * back to.
 *
 * A hover is the one piece of attention with no end-of-life message: a pointer
 * stops being anywhere when its window closes, and `bye` is all that says so.
 * Left alone, the last spec entity the peer pointed at goes on ringing this
 * window's canvas indefinitely, and there is nothing on screen that explains
 * it — the pane it came from is on a monitor that is now showing something
 * else. The reader's only way out would be to find the entity in their own
 * spec pane and point at it, which requires knowing what happened.
 *
 * Only the borrowed one, and only the *spec* hover. A borrowed code hover has
 * the same staleness, but it is still named in the Details and Description
 * panes, so a reader can see what it is and click past it; changing that is a
 * decision about `settleHover` and not part of this.
 */
function dropBorrowedSpecHover(): void {
  if (adoptedSpecHover === null) return;
  peerAttention = { ...peerAttention, specHover: null };
  if (get(specHoverId) === adoptedSpecHover) {
    applying = true;
    try { specHoverId.set(null); } finally { applying = false; }
  }
  adoptedSpecHover = null;
}

/**
 * Hand the panes back to the selection the reader just made.
 *
 * The Description pane prefers hover over selection, which is right for one
 * window — the pointer is the newer gesture — and wrong for a window that is
 * only showing a hover because it borrowed one. In the setup this feature is
 * for, the pane window has no pointer on the graph at all, so without this a
 * click there moved Details and left Description on the peer's node forever.
 */
function declineRemoteHover(): void {
  declinedHover = peerAttention.hovered;
  applying = true;
  try { settleHover(); } finally { applying = false; }
}

function onMessage(ev: MessageEvent): void {
  const raw = ev.data as Record<string, unknown> | null;
  const kind = raw && typeof raw === 'object' ? raw.kind : null;

  // Roll call. A window that has just joined asks, and everyone already on the
  // channel answers — including with their state, so the newcomer lands on the
  // reading the others are on rather than dragging them onto its own.
  if (kind === 'hello') {
    if (raw?.origin === ORIGIN) return;
    peers.update((n) => n + 1);
    channel?.postMessage({ kind: 'here', ...snapshot() });
    return;
  }
  if (kind === 'here') {
    if (raw?.origin === ORIGIN) return;
    peers.update((n) => n + 1);
    // Falls through: a window that has just joined adopts the reading the
    // others are already on, which is the direction that does not surprise
    // anyone. `hello` returns above precisely so the newcomer does not drag
    // established windows onto its own.
  }

  // A window closing says so, because a peer count that only ever went up
  // would claim a mirror after the other screen had gone.
  if (kind === 'bye') {
    if (raw?.origin !== ORIGIN) {
      peers.update((n) => Math.max(0, n - 1));
      dropBorrowedSpecHover();
    }
    return;
  }

  const msg = normalizeMessage(raw);
  if (!msg || msg.origin === ORIGIN) return;
  if (!isFresh(msg, seen)) return;
  seen[msg.origin] = msg.seq;
  void apply(msg);
}

function connect(): void {
  if (channel || typeof BroadcastChannel === 'undefined') return;
  channel = new BroadcastChannel(channelName(serveRepo() ?? ''));
  channel.onmessage = onMessage;
  seen = {};
  peers.set(0);
  channel.postMessage({ kind: 'hello', ...snapshot() });

  // One subscriber over the codec's own store list, so a store added to
  // `ViewState` is published without this file being touched. The rest are
  // separate because `ViewState` deliberately excludes where the reader is
  // standing — that is `Attention`, and it has no derived store of its own
  // because two of the three would loop through `settleHover`.
  const unsubState = currentState.subscribe(() => schedulePublish());
  // A local selection is the newer gesture, so it takes the panes back off a
  // hover this window borrowed. Skipped while applying — a selection arriving
  // from the peer is not this reader clicking — and on a *cleared* selection,
  // which is a request to follow a pointer again, and the nearest pointer this
  // window has is the peer's.
  const unsubSel = selectedNode.subscribe((node) => {
    if (!applying && node) declineRemoteHover();
    schedulePublish();
  });
  // Opening a row in the Changes tab is the second thing the Details pane can
  // be showing, and the one `selected` cannot describe. A *local* open also
  // retires anything the peer left waiting on a list: the reader has since
  // asked for something else, and a request that outlived the gesture would
  // reopen it the next time the comparison refreshed.
  const unsubFile = selectedChangedFile.subscribe(() => {
    if (!applying) pendingFile = null;
    schedulePublish();
  });

  // The list arrives after the comparison it belongs to, so a peer's open file
  // is routinely named a beat late. This is the only announcement of that
  // moment; nothing republishes on our behalf.
  const unsubFiles = changedFiles.subscribe(() => {
    if (!pendingFile || applying) return;
    applying = true;
    try { settleFile(pendingFile); } finally { applying = false; }
  });

  const unsubLock = hoverLocked.subscribe(() => schedulePublish());
  const unsubMode = viewMode.subscribe(() => schedulePublish());
  const unsubPath = specPath.subscribe(() => schedulePublish());
  const unsubPinned = specPinnedHighlight.subscribe(() => schedulePublish());

  // The spec pane's pointer, treated exactly like the canvas's directly below
  // — `applying` separates a remote write from a local move, and the reconcile
  // covers the transition no message announces, which is this window's own
  // pointer leaving the pane and the peer's applying again.
  const unsubSpecHover = specHoverId.subscribe(() => {
    if (applying) return;
    applying = true;
    try { settleSpecHover(); } finally { applying = false; }
    schedulePublish();
  });

  // Hover is the one store this window writes to itself while applying a
  // message, so a remote write must not be mistaken for a local pointer move.
  // `applying` is what tells them apart; the reconcile handles the case the
  // peer cannot announce, which is this window's own pointer leaving.
  const unsubHover = hoveredNode.subscribe(() => {
    if (applying) return;
    applying = true;
    try { settleHover(); } finally { applying = false; }
    schedulePublish();
  });

  unsubscribe = () => {
    unsubState(); unsubSel(); unsubFile(); unsubFiles();
    unsubLock(); unsubMode(); unsubPath(); unsubHover();
    unsubPinned(); unsubSpecHover();
  };
}

function disconnect(): void {
  if (pending) { clearTimeout(pending); pending = null; }
  unsubscribe?.();
  unsubscribe = null;
  channel?.postMessage({ kind: 'bye', origin: ORIGIN });
  channel?.close();
  channel = null;
  peers.set(0);
  // The hover this window is showing is its own from here on, and a peer's
  // last position must not be re-adopted the next time the mode goes on.
  peerAttention = emptyAttention();
  adoptedHover = null;
  declinedHover = null;
  // Both pointers. A *borrowed* spec hover has to go with the channel, or
  // turning the mode off leaves the peer's pointer lighting this window's
  // canvas with nothing on screen to clear it — the pane that would explain
  // the rings is on the other monitor. Only the borrowed one: this window's
  // own pointer may be resting on its own spec pane, and clearing that would
  // make the switch reach across and undo a local gesture.
  if (adoptedSpecHover !== null && get(specHoverId) === adoptedSpecHover) {
    specHoverId.set(null);
  }
  adoptedSpecHover = null;
  // A file the peer never got to name must not open itself the next time a
  // comparison loads here, hours after that window closed.
  pendingFile = null;
}

// The switch is the whole public surface: everything else follows from it.
mirrorOn.subscribe((on) => (on ? connect() : disconnect()));

// Closing the tab is the commonest way a peer leaves, and it never reaches
// the switch. `pagehide` rather than `unload`, which fires unreliably on
// mobile and is ignored by the back-forward cache.
if (typeof window !== 'undefined') {
  window.addEventListener('pagehide', () => channel?.postMessage({ kind: 'bye', origin: ORIGIN }));
}

/**
 * Open a second window onto the same app.
 *
 * `location.href` rather than a bare origin so the new window inherits the
 * endpoint and repo in the query string, and `mirrorOn` being persisted means
 * it comes up already listening. Turning the mode on here as well, because
 * "open a second window" from a window that is not mirroring would produce two
 * unrelated sessions — which is the thing being fixed.
 */
export function openSecondWindow(): void {
  mirrorOn.set(true);
  window.open(window.location.href, '_blank');
}
