<!--
  Writing the spec entity that would have explained this file (UI-145).

  Opened from the Details column at the point where the absence shows up —
  where "Claimed by" would have been on a code entity, and where "Spec" would
  have been on a folder. That placement is the feature: the reader has just
  worked out what the thing does, which is the only moment the sentence is
  cheap to write, and the panel is where they found out nobody had written it.

  The form asks for four things and no more, because `guide/elevator-language.md`
  asks for four: a kind, a name, one sentence, and the code it claims. A
  description is optional here exactly as it is in the language — a body-less
  `f name` is a complete definition and the guide's recommended first move, so
  the button stays enabled with the box empty rather than demanding prose
  nobody is ready to write.

  What it will do is shown before it does it. The write lands in a tracked
  `.elv` and touches two places in that file — the definition at the end and a
  child line inside the parent — so both are rendered, and the file it is
  about to edit is named. The preview comes from `viewmodels/specDraft.ts`;
  what the result shows afterwards comes back from the engine, which is the
  half that actually wrote.
-->
<script lang="ts">
  import {
    composerDraft, composerPhase, composerTarget, closeComposer,
    currentParents, currentSpecFiles, setKind, submitDraft, updateDraft,
  } from '../stores/specDraft';
  import {
    SPEC_KINDS, previewChildRef, previewSource, requiresParent, targetFile,
    validateDraft, type SpecKindCode,
  } from '../viewmodels/specDraft';

  $: draft = $composerDraft;
  $: phase = $composerPhase;
  $: parents = draft ? currentParents(draft.kind) : [];
  $: files = currentSpecFiles();
  /** A kind with no parent tier picks its own file; a kind with one follows
   *  its parent's, which is what keeps `import` out of this. */
  $: picksOwnFile = draft ? parents.length === 0 : false;
  $: destination = draft ? targetFile(draft, parents) : null;
  $: problems = draft ? validateDraft(draft) : [];
  $: childLine = draft ? previewChildRef(draft) : null;

  /** The refs as one editable line. `cr:` takes a list and a reader
   *  documenting a Feature usually claims two or three files, so the box has
   *  to accept more than the one the panel seeded it with. */
  let refsText = '';
  let lastDraftName: string | null = null;
  $: if (draft && draft.name !== lastDraftName && refsText === '') {
    refsText = draft.codeRefs.join(', ');
    lastDraftName = draft.name;
  }

  function commitRefs(): void {
    updateDraft({ codeRefs: refsText.split(',').map((p) => p.trim()).filter(Boolean) });
  }

  function pickKind(kind: SpecKindCode): void {
    setKind(kind);
  }

  function onKeydown(event: KeyboardEvent): void {
    if (event.key === 'Escape') closeComposer();
  }
</script>

<svelte:window on:keydown={onKeydown} />

{#if draft && phase.kind !== 'closed'}
  <div class="composer" data-probe="spec-composer">
    <div class="head">
      <span class="title">Add to spec</span>
      <button type="button" class="close" title="Close (Esc)" on:click={closeComposer}>×</button>
    </div>

    {#if $composerTarget}
      <div class="subject" data-probe="spec-composer-subject">
        {#if $composerTarget.label}
          {$composerTarget.label}
        {:else}
          documenting <span class="mono">{$composerTarget.path || '(repo root)'}</span>
        {/if}
      </div>
    {/if}

    {#if phase.kind === 'written'}
      <!-- What the engine wrote, not what the form guessed it would. -->
      <div class="result" data-probe="spec-composer-result">
        <div class="line">
          Wrote <span class="mono">{phase.result.entity_id}</span>
          to <span class="mono">{phase.result.file}:{phase.result.line}</span>
        </div>
        <pre class="preview">{phase.result.source}</pre>
        {#if phase.result.note}
          <div class="line warn" data-probe="spec-composer-note">{phase.result.note}</div>
        {:else if !phase.result.parented}
          <div class="line dim">
            Not filed under anything yet — `elevator --check` will list it as an orphan, which is
            a to-deepen note rather than a fault.
          </div>
        {/if}
        <div class="line dim">The graph updates when the analyzer picks the file up.</div>
        <div class="actions">
          <button type="button" class="primary" on:click={closeComposer}>Done</button>
        </div>
      </div>
    {:else}
      <label class="field">
        <span class="label">Kind</span>
        <div class="kinds">
          {#each SPEC_KINDS as kind}
            <button
              type="button"
              class="kind-btn"
              class:active={draft.kind === kind.code}
              title={kind.hint}
              data-probe="spec-kind-{kind.code}"
              on:click={() => pickKind(kind.code)}
            >{kind.label}</button>
          {/each}
        </div>
      </label>

      <label class="field">
        <span class="label">Name</span>
        <input
          class="input mono"
          data-probe="spec-composer-name"
          value={draft.name}
          placeholder="diff_streaming"
          on:input={(e) => updateDraft({ name: e.currentTarget.value })}
        />
      </label>

      {#if parents.length > 0}
        <label class="field">
          <span class="label">Under</span>
          <select
            class="input"
            data-probe="spec-composer-parent"
            value={draft.parentId ?? ''}
            on:change={(e) => updateDraft({ parentId: e.currentTarget.value || null })}
          >
            <option value="">{requiresParent(draft.kind) ? '— pick one —' : '— nothing —'}</option>
            {#each parents as parent}
              <option value={parent.id}>{parent.qualifiedName}</option>
            {/each}
          </select>
        </label>
      {/if}

      <label class="field">
        <span class="label">Description</span>
        <textarea
          class="input"
          rows="3"
          data-probe="spec-composer-description"
          placeholder="One sentence of mechanism. Optional — a named sketch is a complete definition."
          value={draft.description}
          on:input={(e) => updateDraft({ description: e.currentTarget.value })}
        ></textarea>
      </label>

      <label class="field">
        <span class="label">Claims</span>
        <input
          class="input mono"
          data-probe="spec-composer-refs"
          bind:value={refsText}
          placeholder="src/server/mod.rs, ui/src/"
          on:blur={commitRefs}
          on:change={commitRefs}
        />
      </label>

      {#if picksOwnFile}
        <label class="field">
          <span class="label">File</span>
          <input
            class="input mono"
            list="spec-files"
            data-probe="spec-composer-file"
            value={draft.file}
            placeholder="spec/concepts.elv"
            on:input={(e) => updateDraft({ file: e.currentTarget.value })}
          />
          <datalist id="spec-files">
            {#each files as file}<option value={file}></option>{/each}
          </datalist>
        </label>
      {/if}

      <!-- Both edits, before either happens. -->
      <div class="field">
        <span class="label">Writes</span>
        <div class="destination mono" data-probe="spec-composer-destination">
          {destination ?? '— no file chosen —'}
        </div>
        <pre class="preview" data-probe="spec-composer-preview">{previewSource(draft, parents)}</pre>
        {#if childLine}
          <div class="line dim">
            and <span class="mono">{childLine}</span> inside that parent's body
          </div>
        {/if}
      </div>

      {#if phase.kind === 'editing' && phase.error}
        <div class="line warn" data-probe="spec-composer-error">{phase.error}</div>
      {:else if problems.length}
        <div class="line dim" data-probe="spec-composer-problem">{problems[0]}</div>
      {/if}

      <div class="actions">
        <button type="button" on:click={closeComposer}>Cancel</button>
        <button
          type="button"
          class="primary"
          data-probe="spec-composer-submit"
          disabled={problems.length > 0 || phase.kind === 'sending'}
          on:click={() => void submitDraft()}
        >{phase.kind === 'sending' ? 'Writing…' : 'Create'}</button>
      </div>
    {/if}
  </div>
{/if}

<style>
  .composer {
    margin: 8px 0;
    padding: 10px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg-surface-alt);
  }

  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 6px;
  }

  .title {
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--text-secondary);
  }

  .close {
    border: none;
    background: none;
    color: var(--text-dim);
    cursor: pointer;
    font-size: 15px;
    line-height: 1;
    padding: 0 2px;
  }

  .close:hover {
    color: var(--text);
  }

  .subject {
    font-size: 11px;
    color: var(--text-dim);
    margin-bottom: 8px;
  }

  .field {
    display: block;
    margin-bottom: 8px;
  }

  .label {
    display: block;
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-dim);
    margin-bottom: 3px;
  }

  .input {
    width: 100%;
    box-sizing: border-box;
    padding: 4px 6px;
    font-size: 12px;
    font-family: inherit;
    color: var(--text);
    background: var(--bg-surface);
    border: 1px solid var(--border);
    border-radius: 4px;
  }

  .input:focus {
    outline: none;
    border-color: var(--accent);
  }

  textarea.input {
    resize: vertical;
  }

  .mono {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  }

  .kinds {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }

  .kind-btn {
    padding: 3px 8px;
    font-size: 11px;
    color: var(--text-secondary);
    background: var(--bg-surface);
    border: 1px solid var(--border);
    border-radius: 4px;
    cursor: pointer;
  }

  .kind-btn:hover {
    background: var(--bg-hover);
    color: var(--text);
  }

  .kind-btn.active {
    background: var(--accent);
    color: var(--accent-fg);
    border-color: var(--accent);
  }

  .destination {
    font-size: 11px;
    color: var(--text-secondary);
    margin-bottom: 4px;
  }

  .preview {
    margin: 0;
    padding: 6px 8px;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 11px;
    line-height: 1.45;
    color: var(--text-secondary);
    background: var(--bg-deep);
    border: 1px solid var(--border-subtle);
    border-radius: 4px;
    overflow-x: auto;
    white-space: pre;
  }

  .line {
    font-size: 11px;
    line-height: 1.45;
    color: var(--text-secondary);
    margin-top: 5px;
  }

  .line.dim {
    color: var(--text-dim);
  }

  .line.warn {
    color: var(--accent);
  }

  .result .preview {
    margin-top: 5px;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 6px;
    margin-top: 10px;
  }

  .actions button {
    padding: 4px 10px;
    font-size: 11px;
    color: var(--text-secondary);
    background: var(--bg-surface);
    border: 1px solid var(--border);
    border-radius: 4px;
    cursor: pointer;
  }

  .actions button:hover:not(:disabled) {
    background: var(--bg-hover);
    color: var(--text);
  }

  .actions button.primary {
    background: var(--accent);
    color: var(--accent-fg);
    border-color: var(--accent);
    font-weight: 600;
  }

  .actions button:disabled {
    color: var(--text-disabled);
    cursor: default;
  }
</style>
