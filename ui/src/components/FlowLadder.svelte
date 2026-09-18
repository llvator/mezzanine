<script lang="ts">
  /**
   * UI-146 — which way the dependencies run, in the Details column.
   *
   * One component for both panels, because it is one question asked at two
   * scales: a folder is read through its files, a file through its entities,
   * and a reader moving between the two should not be learning a second
   * layout. `FolderInfo` and `EntityInfo` each pass a subject and, when they
   * have one, the member to mark.
   *
   * Two halves, and they answer different questions on purpose:
   *
   * - **the standing** — where the subject itself sits among its siblings. The
   *   half that places a folder in the repo.
   * - **the ladder** — what is inside it, in layers. The half that says what
   *   the folder is made of.
   *
   * Layer 0 is the foundation: it depends on nothing else in the set. Each
   * layer above stands on the ones below, so a change at the bottom flows
   * upward. The arrows on the canvas point the other way — at what each node
   * needs — which is why the caption states the axis rather than leaving it to
   * be inferred.
   */
  import { focusNode, graphData } from '../stores/graph';
  import { scopeFlowOf, siblingStandingOf } from '../stores/flow';
  import type { FlowSubject } from '../viewmodels/scopeFlow';

  export let subject: FlowSubject;
  /** A member key to mark in the ladder — the entity the panel is about, when
   *  the subject is the file around it. Null when the subject IS the thing
   *  being described. */
  export let highlight: string | null = null;

  /** Rows past this are summarised rather than listed. A layer with forty
   *  members is a fact worth stating; forty rows in a hover column is a
   *  directory listing that pushes the Quality block off screen. */
  const PER_LAYER = 10;

  $: flow = $scopeFlowOf(subject);
  $: standing = $siblingStandingOf(subject);
  $: subjectStanding = standing ? standing.flow.reading.standing.get(standing.key) ?? null : null;
  $: layers = flow.reading.layers;
  $: memberCount = flow.members.size;
  $: noun = flow.memberNoun === 'file' ? 'file' : 'entity';
  /** Only the cycles worth naming: a two-member tangle inside a file is
   *  mutual recursion and is usually deliberate; the panel still lists it,
   *  because "these two cannot be read in either order" is true either way. */
  $: cycles = flow.reading.cycles;

  /**
   * Which rows a click could actually act on.
   *
   * The ladder is computed from the analysed graph and the canvas draws a
   * collapsed, filtered one — so an entity row is real at Entity level and has
   * no node behind it at File or Folder level. A button that silently does
   * nothing is worse than plain text: it teaches that the panel is broken. So
   * rows are offered only while there is something to select, and read as text
   * the rest of the time.
   */
  $: drawnIds = new Set($graphData.nodes.map((n) => n.id));
  $: clickable = (key: string): boolean =>
    flow.members.get(key)?.grain === 'entity' && drawnIds.has(key);

  function selectMember(key: string): void {
    const node = $graphData.nodes.find((n) => n.id === key);
    if (node) focusNode(node);
  }

  function roleLabel(key: string): string {
    const s = flow.reading.standing.get(key);
    if (!s) return '';
    if (s.role === 'foundation') return 'foundation';
    if (s.role === 'entry') return 'entry point';
    if (s.role === 'isolated') return 'unconnected';
    return '';
  }

  function rowTitle(key: string): string {
    const s = flow.reading.standing.get(key);
    if (!s) return key;
    const parts = [
      key,
      `layer ${s.layer} of ${layers.length}`,
      `${s.upstream} ${noun}${s.upstream === 1 ? '' : 's'} upstream`,
      `${s.downstream} downstream`,
    ];
    if (s.cycle) parts.push(`in a cycle with ${s.cycle.length} other${s.cycle.length === 1 ? '' : 's'}`);
    return parts.join(' · ');
  }
</script>

{#if memberCount > 0}
  <div class="detail-row" data-probe="flow-ladder" data-flow-path={subject.path}>
    <div class="detail-label">Flow</div>

    <!-- Where the subject itself sits. Absent at the repo root and for an
         only child, where the number would be arithmetic rather than a
         reading. -->
    {#if standing && subjectStanding}
      <div class="line" data-probe="flow-standing"
        title="Among the {standing.flow.members.size} children of {standing.parent || 'the repo root'}, by the dependencies between them">
        Layer {subjectStanding.layer}
        <span class="dim">of {standing.flow.reading.layers.length} in {standing.parent || '(repo root)'}</span>
        <span class="counts">
          <span title="What it depends on, transitively">↑ {subjectStanding.upstream}</span>
          <span title="What depends on it, transitively — the blast radius">↓ {subjectStanding.downstream}</span>
        </span>
      </div>
    {/if}

    <div class="line dim" data-probe="flow-summary">
      {memberCount} {noun}{memberCount === 1 ? '' : 's'} inside, in
      {layers.length} layer{layers.length === 1 ? '' : 's'}
      <span class="axis" title="Layer 0 depends on nothing else here. Each layer stands on the ones below it, so a change at the bottom flows upward — the canvas arrows point the other way, at what each row needs.">upstream → downstream</span>
    </div>

    <ol class="ladder" data-probe="flow-layers">
      {#each layers as members, layer}
        {#if members.length > 0}
          <li class="rung">
            <span class="rung-index" title="Layer {layer}">{layer}</span>
            <span class="rung-members">
              {#each members.slice(0, PER_LAYER) as key}
                {@const marked = key === highlight}
                {@const role = roleLabel(key)}
                {#if clickable(key)}
                  <button
                    type="button"
                    class="member"
                    class:marked
                    class:cycle={!!flow.reading.standing.get(key)?.cycle}
                    title={rowTitle(key)}
                    on:click={() => selectMember(key)}
                  >{flow.members.get(key)?.label}{#if role}<span class="role">{role}</span>{/if}</button>
                {:else}
                  <span
                    class="member"
                    class:marked
                    class:folder={flow.members.get(key)?.grain === 'folder'}
                    class:cycle={!!flow.reading.standing.get(key)?.cycle}
                    title={rowTitle(key)}
                  >{flow.members.get(key)?.label}{#if role}<span class="role">{role}</span>{/if}</span>
                {/if}
              {/each}
              {#if members.length > PER_LAYER}
                <span class="more">+{members.length - PER_LAYER} more</span>
              {/if}
            </span>
          </li>
        {/if}
      {/each}
    </ol>

    <!-- Named, not merely counted. A cycle is the one thing in this reading
         that has no hierarchy to report, and saying "4 in a cycle" without
         saying which four leaves the reader unable to act on it. -->
    {#if cycles.length > 0}
      <div class="line cycles" data-probe="flow-cycles"
        title="These have no upstream end — each reaches the other, so the layer they share is the only honest answer">
        {#each cycles.slice(0, 3) as members}
          <div class="cycle-row">⟳ {members.map((m) => flow.members.get(m)?.label ?? m).join(' ⇄ ')}</div>
        {/each}
        {#if cycles.length > 3}
          <div class="cycle-row dim">+{cycles.length - 3} more cycles</div>
        {/if}
      </div>
    {/if}
  </div>
{/if}

<style>
  .detail-row { margin-top: 12px; }

  .detail-label {
    font-size: 0.68rem;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--text-muted);
    margin-bottom: 5px;
  }

  .line {
    font-size: 0.78rem;
    line-height: 1.5;
    color: var(--text-secondary);
  }
  .dim { color: var(--text-dim); }

  .counts {
    margin-left: 6px;
    display: inline-flex;
    gap: 8px;
    color: var(--text-muted);
    cursor: help;
  }

  .axis {
    margin-left: 6px;
    font-size: 0.68rem;
    letter-spacing: 0.04em;
    color: var(--text-muted);
    cursor: help;
  }

  .ladder {
    list-style: none;
    margin: 6px 0 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .rung {
    display: flex;
    align-items: baseline;
    gap: 6px;
  }

  /* The layer number is the axis. Kept as a fixed-width gutter so the rungs
     line up into a column a reader can run their eye down. */
  .rung-index {
    flex: 0 0 auto;
    min-width: 14px;
    text-align: right;
    font-size: 0.66rem;
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
  }

  .rung-members {
    display: flex;
    flex-wrap: wrap;
    gap: 3px;
  }

  .member {
    display: inline-flex;
    align-items: baseline;
    gap: 4px;
    padding: 1px 6px;
    border-radius: 3px;
    border: 1px solid var(--border);
    background: var(--bg-surface-alt);
    color: var(--text-secondary);
    font: inherit;
    font-size: 0.72rem;
    text-align: left;
    overflow-wrap: anywhere;
  }

  button.member { cursor: pointer; }
  button.member:hover { border-color: var(--accent); color: var(--accent); }

  /* A subfolder stands for a whole subtree, so it is drawn as a group rather
     than as a leaf. */
  .member.folder { border-style: dashed; }

  .member.marked {
    border-color: var(--accent);
    color: var(--text);
    font-weight: 600;
  }

  /* Same glyph as the cycle list below, so a marked row and its explanation
     are visibly the same claim. */
  .member.cycle::before {
    content: '⟳';
    font-size: 0.66rem;
    color: var(--text-muted);
  }

  .role {
    font-size: 0.6rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-dim);
  }

  .more {
    font-size: 0.68rem;
    color: var(--text-dim);
    align-self: center;
  }

  .cycles {
    margin-top: 6px;
    cursor: help;
  }
  .cycle-row {
    font-size: 0.72rem;
    color: var(--text-muted);
    overflow-wrap: anywhere;
  }
</style>
