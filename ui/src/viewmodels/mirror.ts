/**
 * What two windows of the app owe each other when they are kept in sync
 * (UI-095).
 *
 * The reason to want this is real estate: two monitors can hold panes one
 * window never can. So the thing that syncs is the *reading* — the scope, the
 * level, the filters — plus where the reader is standing in it, and the thing
 * that does not is the *layout*. Both windows show the same graph; one can
 * give the canvas the whole screen while the other gives Details 1200px.
 * Mirroring the pane widths too would make the second window a duplicate
 * rather than an extension, which is the opposite of the point.
 *
 * A reading already has a name and a codec: `ViewState`, from UI-082. This
 * module does not define a second one. What it adds is `Attention`, which
 * `ViewState` deliberately excludes — a saved view records what reaches the
 * canvas, not where the reader is standing, and for a mirror the second one is
 * most of the value.
 *
 * Pure and store-free, like `viewHistory.ts`: the rules about what to apply
 * are the part worth testing, and testing them should not need two browser
 * windows (`npm run test:mirror`).
 */

import { sameState, normalizeState, type ViewState } from './savedViews.ts';
import type { ViewMode } from '../types/graph.ts';

/**
 * Where the reader is standing, as opposed to what reaches the canvas.
 *
 * `ViewState` answers the second question and deliberately not the first. This
 * is the first: the entity under the pointer, the one clicked, whether the
 * preview is frozen, and which way the canvas is drawn. All of it is cheap to
 * apply — no refetch — which is why it travels beside the reading rather than
 * inside it.
 *
 * `hovered` is here because of what reads it. The Description pane prefers
 * hover over selection, and Details falls back to it, so a window whose panes
 * are the reason the second monitor exists shows nothing at all while the
 * pointer moves on the other screen. UI-095 first filed hover with the layout,
 * next to pane widths and zoom; that was wrong for the setup the feature is
 * for — canvas on one screen, panes on the other.
 *
 * `specPath` is here and *not* in `ViewState` for the reason that file's header
 * gives: a saved view records what reaches the canvas, and the spec pane's
 * drill path decides what the **spec pane** draws, not which code entities
 * survive the filter. The thing that does reach the canvas is the spec
 * *selection*, and that is already `ViewState.spec`. Filing the path here also
 * makes drilling an `attention` change rather than an `all` one, so opening a
 * Category on one screen does not cost a graph refetch on the other.
 */
export interface Attention {
  /** Entity id, or `null` for nothing selected. */
  selected: string | null;
  /** Entity id under the pointer, or `null`. */
  hovered: string | null;
  /**
   * The Changes-tab row the reader opened, as a repo-relative path (UI-142).
   *
   * Here because the Details pane has *two* contents, and `selected` only
   * names one of them: a file opened from the Changes tab wins the pane over
   * the selected entity (UI-134), and it is the whole file's source rather
   * than an entity's. Mirroring the selection alone left the second window
   * showing the file's node — name, metrics, relationships — beside a first
   * window showing the file.
   *
   * A path and not the row itself, for the same reason `selected` is an id:
   * what a window may open is what its *own* comparison lists. The loaded
   * comparison is deliberately not part of `ViewState`, so two windows are
   * not guaranteed to be on the same one, and shipping the peer's row would
   * render it against refs this window never compared.
   */
  file: string | null;
  /** A preview the reader froze (`L`), which outlives the pointer leaving. */
  hoverLocked: boolean;
  mode: ViewMode;
  /** The spec pane's open path, root first. Ordered, so it is compared as a
   *  sequence rather than as a set — `[a, b]` and `[b, a]` are two different
   *  descents. */
  specPath: string[];
  /**
   * The spec entity under the pointer in the spec pane, or `null`.
   *
   * The second pointer this app has, and the one the two-window setup exists
   * to carry. `hovered` above is a pointer on the *code* canvas; this is a
   * pointer on the spec pane, and what it moves is a ring on the code canvas
   * — so in the arrangement this whole feature is for, canvas on one screen
   * and panes on the other, it is the only one of the two that can ever fire.
   * Left out, the highlight was a feature that worked in every window except
   * the one shaped like its use case.
   *
   * Attention and not `ViewState` by that file's own test: it hides nothing,
   * costs no refetch, and a saved view has no business recording where a
   * pointer was resting.
   */
  specHover: string | null;
  /**
   * Spec entities pinned to keep their code lit, unordered.
   *
   * Travels beside the hover rather than inside it because it answers to a
   * different gesture — a pin is a click, and clicks are the newest word
   * whichever window made them, the way `selected` already is. Compared as a
   * **set**, unlike `specPath` directly above: two windows holding the same
   * two pins in the other order are looking at the same picture, and treating
   * that as a difference would put the two into a publish-apply loop over
   * nothing.
   */
  specPinned: string[];
}

/**
 * One window telling the others where it is.
 *
 * `origin` is the sending window's id, which is what makes a message
 * identifiable as one's own echo. `seq` is a per-window counter, used only to
 * drop messages that arrive out of order — `BroadcastChannel` does not
 * reorder, but a debounced publisher plus an async apply can still deliver a
 * stale payload after a fresh one.
 */
export interface MirrorMessage {
  origin: string;
  seq: number;
  state: ViewState;
  attention: Attention;
}

const MODES: ViewMode[] = ['graph', 'tree'];

/** Standing nowhere in particular — the shape a message that predates this
 *  field, or cannot be read, falls back to. */
export function emptyAttention(): Attention {
  return {
    selected: null, hovered: null, file: null, hoverLocked: false, mode: 'graph',
    specPath: [], specHover: null, specPinned: [],
  };
}

export function sameAttention(a: Attention, b: Attention): boolean {
  return a.selected === b.selected
    && a.hovered === b.hovered
    && a.file === b.file
    && a.hoverLocked === b.hoverLocked
    && a.mode === b.mode
    && a.specHover === b.specHover
    && samePath(a.specPath, b.specPath)
    && sameSet(a.specPinned, b.specPinned);
}

function samePath(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((step, i) => step === b[i]);
}

/** Membership only. See `Attention.specPinned` for why the pins are compared
 *  this way and the path directly above them is not. */
function sameSet(a: readonly string[], b: readonly string[]): boolean {
  if (a.length !== b.length) return false;
  const held = new Set(a);
  return b.every((id) => held.has(id));
}

/**
 * What an arriving message asks this window to do.
 *
 * - `none` — it is our own echo, or it describes the picture already on screen.
 * - `attention` — same reading, the reader moved within it.
 * - `all` — the reading itself moved.
 *
 * The middle case is why this is a three-way answer rather than a boolean.
 * Pointing at nodes is the commonest gesture there is, and `restoreState` is
 * not cheap: it re-publishes the scope and waits on a graph fetch. A window
 * that ran the full restore every time its peer's pointer crossed a node would
 * spend the whole session refetching a graph it already had.
 */
export type MirrorAction = 'none' | 'attention' | 'all';

export function whatToApply(incoming: MirrorMessage, mine: MirrorMessage): MirrorAction {
  if (incoming.origin === mine.origin) return 'none';
  if (!sameState(incoming.state, mine.state)) return 'all';
  return sameAttention(incoming.attention, mine.attention) ? 'none' : 'attention';
}

/**
 * Which hover this window should show, given that both windows have a pointer.
 *
 * Local wins. Two readers sweeping two canvases would otherwise overwrite each
 * other every few pixels and neither pane would settle on anything — and the
 * one whose pointer is moving is the one who asked a question.
 *
 * `adopted` is the remote hover this window last took on, and it is what makes
 * the rule decidable: `hoveredNode` alone cannot say whether the entity under
 * it got there from this window's pointer or the other's. Once the local
 * pointer leaves, the peer's hover is picked back up rather than the pane
 * falling to the selection — otherwise a mirror stops mirroring the moment you
 * touch it, and stays stopped.
 *
 * `declined` is the third case, and it is the one the two-window setup made
 * visible: a borrowed hover has to lose to a **local click**. In the setup this
 * feature is for — canvas on one screen, panes on the other — the pane window
 * has no pointer of its own on the graph, so it is always showing the peer's
 * hover; selecting something in its own Spec pane then moved Details and left
 * Description narrating the peer's pointer. Nothing about `local`, `remote` or
 * `adopted` distinguishes that from an ordinary remote hover, so the caller
 * names the hover the click refused.
 *
 * The refusal is on that one entity and not on mirroring: the moment the peer's
 * pointer moves somewhere else it is a new gesture and gets adopted again,
 * which is what keeps this from becoming "a mirror that stops mirroring the
 * moment you touch it".
 *
 * **Both pointers use this**, the one on the code canvas and the one on the
 * spec pane (`Attention.specHover`) — "local wins, and a borrowed hover is
 * given back when the local pointer leaves" is the same rule for either.
 * The spec pointer passes no `declined`, and that is not an omission: the
 * refusal exists because a borrowed *code* hover competes with a local click
 * for the Details and Description panes, and the spec hover competes with
 * nothing — it lights rings on the canvas and nothing else reads it.
 */
export function hoverToShow(
  local: string | null,
  remote: string | null,
  adopted: string | null,
  declined: string | null = null,
): string | null {
  const ownedLocally = local !== null && local !== adopted;
  if (ownedLocally) return local;
  return remote === declined ? null : remote;
}

/**
 * Is this message newer than the last one we accepted from that window?
 *
 * Kept separate from `whatToApply` because the two questions fail differently:
 * an out-of-order message is dropped without looking at its contents, while a
 * current message may still turn out to ask for nothing.
 */
export function isFresh(incoming: MirrorMessage, seen: Record<string, number>): boolean {
  const last = seen[incoming.origin];
  return last === undefined || incoming.seq > last;
}

/**
 * A message off the wire, or `null`.
 *
 * Total over garbage for the same reason `normalizeViews` is: nothing
 * guarantees the other end of a `BroadcastChannel` is a version of this app
 * that agrees about the payload. An older tab left open across a deploy is the
 * realistic case, and the right answer to a payload we cannot read is to
 * ignore it, not to throw inside an event handler where nothing will catch it.
 *
 * A window still sending the flat `selected` this message used to carry is
 * exactly that case, and it reads as standing nowhere: the reading still
 * syncs, the pointer does not. Better than the alternative, which is a tab
 * that predates the field dragging its peer's selection to `null` on every
 * message it sends.
 */
export function normalizeMessage(raw: unknown): MirrorMessage | null {
  if (!raw || typeof raw !== 'object') return null;
  const m = raw as Record<string, unknown>;
  if (typeof m.origin !== 'string' || m.origin === '') return null;
  if (typeof m.seq !== 'number' || !Number.isFinite(m.seq)) return null;
  if (!m.state || typeof m.state !== 'object') return null;
  return {
    origin: m.origin,
    seq: m.seq,
    state: normalizeState(m.state),
    attention: normalizeAttention(m.attention),
  };
}

function normalizeAttention(raw: unknown): Attention {
  if (!raw || typeof raw !== 'object') return emptyAttention();
  const a = raw as Record<string, unknown>;
  return {
    selected: typeof a.selected === 'string' ? a.selected : null,
    hovered: typeof a.hovered === 'string' ? a.hovered : null,
    file: typeof a.file === 'string' ? a.file : null,
    hoverLocked: a.hoverLocked === true,
    mode: MODES.includes(a.mode as ViewMode) ? (a.mode as ViewMode) : 'graph',
    specPath: Array.isArray(a.specPath)
      ? a.specPath.filter((s): s is string => typeof s === 'string')
      : [],
    specHover: typeof a.specHover === 'string' ? a.specHover : null,
    specPinned: Array.isArray(a.specPinned)
      ? a.specPinned.filter((s): s is string => typeof s === 'string')
      : [],
  };
}

/**
 * The channel name two windows have to agree on.
 *
 * Keyed by which repo is being read, so `mezz serve` — one origin hosting many
 * repos — does not sync a window reading one project into a window reading
 * another. Two windows on the same repo is exactly the case this is for; two
 * windows on different repos is two unrelated sessions and should stay that
 * way.
 */
export function channelName(repoKey: string): string {
  return `mezz-mirror:${repoKey}`;
}
