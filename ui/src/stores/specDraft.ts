/**
 * The spec composer's state and its one write — UI-145.
 *
 * The rules for *what* a draft may be live in `viewmodels/specDraft.ts`; this
 * module is the parts that need the app: the graph the pickers read, the
 * panel-open state, and the `POST /api/spec/entity` that writes the `.elv`.
 *
 * **The engine writes, not the browser.** The panel renders a preview so the
 * reader can see the lines before they land in a tracked file, but the
 * response carries the source the engine actually wrote and that is what the
 * result shows. A preview that disagreed with the file would be worse than no
 * preview, so only one of them is ever the record.
 *
 * **Nothing here refetches the graph.** `mezz watch` is watching the `.elv`
 * that just changed and re-analyzes on save, so the new entity arrives
 * through the same live-reload path every other edit does. A refetch fired
 * from here would race that one and show the reader a graph from before their
 * own write half the time.
 *
 * **`mezz serve` has no such route.** The tree there arrived from a URL a
 * stranger pasted (ADR 0008), and a 404 is how a server says it will not be
 * written to — the panels read `specWritable` and offer no button at all,
 * rather than offering one that fails on click.
 */

import { get, writable } from 'svelte/store';

import { apiUrl } from '../vscodeAdapter';
import { currentEndpoint } from './connection';
import { fullGraphDataStore } from './scope';
import {
  draftFor,
  parentOptions,
  specFiles,
  targetFile,
  validateDraft,
  type ParentOption,
  type SpecDraft,
  type SpecKindCode,
  type SpecTarget,
} from '../viewmodels/specDraft';

/** What the engine says it wrote. Mirrors `CreateResponse` in
 *  `src/server/spec_handler.rs`. */
export interface SpecWriteResult {
  file: string;
  entity_id: string;
  line: number;
  parented: boolean;
  source: string;
  note?: string;
}

/** The composer is closed, collecting, sending, or done. Modelled as one
 *  value rather than three booleans, because "sending" and "done" are not
 *  states the form can be in at once and two booleans can say they are. */
export type ComposerPhase =
  | { kind: 'closed' }
  | { kind: 'editing'; error: string | null }
  | { kind: 'sending' }
  | { kind: 'written'; result: SpecWriteResult };

export const composerPhase = writable<ComposerPhase>({ kind: 'closed' });

/** What the open composer is documenting. Kept beside the phase rather than
 *  inside it so the panel can still name its subject on the result screen. */
export const composerTarget = writable<SpecTarget | null>(null);

/** The draft being edited. Null whenever the composer is closed. */
export const composerDraft = writable<SpecDraft | null>(null);

/** Whether this engine will accept a write at all — false in `mezz serve`,
 *  and false once a request has come back 404. Panels hide the button rather
 *  than offering one that cannot work. */
export const specWritable = writable(true);

/** The parents offerable for the draft's current kind. Recomputed rather
 *  than stored, so switching kind can never leave a Category selected as a
 *  Feature's parent. */
export function currentParents(kind: SpecKindCode): ParentOption[] {
  return parentOptions(get(fullGraphDataStore), kind);
}

/** The `.elv` files the kinds that stand alone may be written into. */
export function currentSpecFiles(): string[] {
  return specFiles(get(fullGraphDataStore));
}

/** Open the composer on a target. */
export function openComposer(target: SpecTarget, kind: SpecKindCode = 'f'): void {
  composerTarget.set(target);
  composerDraft.set(draftFor(target, kind, currentParents(kind), currentSpecFiles()));
  composerPhase.set({ kind: 'editing', error: null });
}

export function closeComposer(): void {
  composerPhase.set({ kind: 'closed' });
  composerDraft.set(null);
  composerTarget.set(null);
}

/**
 * Switch the kind mid-draft.
 *
 * The parent is dropped unless the new kind can still hold it: `f` under a
 * Category and `fu` under a Feature are different pickers, and carrying a
 * selection across would send the engine a parent it refuses — or worse, one
 * it accepts and files the entity in the wrong place.
 */
export function setKind(kind: SpecKindCode): void {
  composerDraft.update((draft) => {
    if (!draft) return draft;
    const parents = parentOptions(get(fullGraphDataStore), kind);
    const stillValid = parents.some((p) => p.id === draft.parentId);
    return {
      ...draft,
      kind,
      parentId: stillValid ? draft.parentId : parents.length === 1 ? parents[0].id : null,
    };
  });
}

/** Patch the draft in place. */
export function updateDraft(patch: Partial<SpecDraft>): void {
  composerDraft.update((draft) => (draft ? { ...draft, ...patch } : draft));
}

/**
 * Write the draft.
 *
 * The token rides in the body for the same reason the agent-spawn route takes
 * it there: this route changes the repo rather than reading it, so it does
 * not inherit the loopback exemption the data API grants, and the bundled UI
 * — served same-origin — is admitted by its `Origin` instead.
 */
export async function submitDraft(): Promise<void> {
  const draft = get(composerDraft);
  if (!draft) return;
  const problems = validateDraft(draft);
  if (problems.length) {
    composerPhase.set({ kind: 'editing', error: problems[0] });
    return;
  }

  composerPhase.set({ kind: 'sending' });
  const { token } = currentEndpoint();
  const parents = currentParents(draft.kind);
  try {
    const resp = await fetch(apiUrl('/api/spec/entity'), {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        kind: draft.kind,
        name: draft.name.trim(),
        parent_id: draft.parentId,
        description: draft.description.trim() || null,
        code_refs: draft.codeRefs.map((p) => p.trim()).filter(Boolean),
        file: targetFile(draft, parents),
        token,
      }),
    });
    if (resp.status === 404) {
      // Not a failed write — a server that has no such route. Say so once and
      // stop offering the button.
      specWritable.set(false);
      composerPhase.set({
        kind: 'editing',
        error: 'This engine does not accept spec writes. `mezz watch` does; `mezz serve` does not.',
      });
      return;
    }
    if (!resp.ok) {
      composerPhase.set({
        kind: 'editing',
        error: (await resp.text()) || `HTTP ${resp.status}`,
      });
      return;
    }
    composerPhase.set({ kind: 'written', result: (await resp.json()) as SpecWriteResult });
  } catch (e) {
    composerPhase.set({
      kind: 'editing',
      error: e instanceof Error ? e.message : String(e),
    });
  }
}
