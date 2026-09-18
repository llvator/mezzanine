<script lang="ts">
  /**
   * The marked set, read as a relationship — UI-147.
   *
   * `EntityInfo` and `FolderInfo` answer about one thing; this answers about
   * the space between two, which is the question the marking gesture was
   * always for and the one nothing in the app could answer. It keeps their
   * shape on purpose — a verdict, then labelled rows, in descending order of
   * how likely a reader is to act on them — so the column reads as one panel
   * with three subjects rather than three panels.
   *
   * The order is the argument: the direction of the dependency first, because
   * that is what a reader is usually wrong about; then what they share, which
   * is what says whether two things belong together; then the routes, which
   * only matter once the first two have come up empty.
   *
   * Everything here is presentation. `viewmodels/markRelation.ts` decides what
   * is true, `stores/relate.ts` decides when to ask, and both are testable
   * without a DOM.
   */
  import { closeRelate, relateLoading, relation } from '../stores/relate';
  import { clearMarks } from '../stores/marks';
  import { drillIntoMarks } from '../stores/scope';
  import { focusNode } from '../stores/graph';
  import { graphData } from '../stores/graph';
  import type { Chain, DirectFlow, SharedList, SharedScope } from '../viewmodels/markRelation';

  /** Which flows are showing their entity pairs. Collapsed by default: the
   *  counts are the answer, and twelve rows of `parse → emit` under each
   *  direction would bury the shared lists below them. */
  let openPairs = new Set<string>();
  function togglePairs(key: string) {
    const next = new Set(openPairs);
    if (!next.delete(key)) next.add(key);
    openPairs = next;
  }

  $: sides = $relation?.sides ?? [];

  const arrow = (f: DirectFlow) => `${sides[f.from]?.label} → ${sides[f.to]?.label}`;
  const flowKey = (f: DirectFlow) => `${f.from}->${f.to}`;

  function kindLine(f: DirectFlow): string {
    return f.kinds.map((k) => `${k.count} ${k.label}`).join(' · ');
  }

  function chainLine(c: Chain): string {
    return [sides[c.from]?.label, ...c.via.map((v) => v.label), sides[c.to]?.label].join(' → ');
  }

  function sharedTitle(s: SharedScope): string {
    return sides.map((side, i) => `${side.label}: ${s.perSide[i] ?? 0}`).join(' · ');
  }

  /** Rows the cap left out. Counted from both parts, because the cap applies
   *  to each separately — see `MAX_SHARED_EXTERNAL`. */
  const hidden = (l: SharedList) => l.files + l.external - l.rows.length;

  /**
   * Jump to one end of a listed edge.
   *
   * The pair is the reason the direct flow lists entities at all — "these two
   * files are coupled" is a fact you act on by opening the function. Only
   * possible when the canvas is currently drawing that entity: at File level
   * it is not, and the row stays plain text rather than becoming a control
   * that does nothing.
   */
  function reveal(id: string) {
    const node = $graphData.nodes.find((n) => n.id === id);
    if (node) focusNode(node);
  }
  const drawn = (id: string) => $graphData.nodes.some((n) => n.id === id);
</script>

{#if $relation}
  <div class="relation" data-probe="relation-info">
    <div class="head">
      <span class="kind">Marked</span>
      <span class="tone tone-{$relation.tone}" data-probe="relation-tone">{$relation.tone}</span>
    </div>

    <p class="verdict" data-probe="relation-verdict">{$relation.verdict}</p>

    <!-- The sides, named in full. The verdict calls them by their last
         segment, which is ambiguous the moment two marked files are both
         called `mod.rs` — this row is where that is resolved. -->
    <div class="detail-row">
      <div class="detail-label">Between</div>
      {#each sides as side, i}
        <div class="line side" data-probe="relation-side">
          <span class="side-index">{i + 1}</span>
          <span class="side-path" title={side.path}>{side.path}</span>
          <span class="dim">
            {side.grain === 'file' ? 'file' : `folder · ${side.files} files`}
            · {side.entities} entities
          </span>
        </div>
      {/each}
    </div>

    <!-- 1. Which way it runs. -->
    <div class="detail-row">
      <div class="detail-label">Direct edges</div>
      {#if $relation.flows.length === 0}
        <div class="line dim" data-probe="relation-no-flow">
          Nothing runs between them in either direction.
        </div>
      {:else}
        {#each $relation.flows as flow (flowKey(flow))}
          <div class="flow" data-probe="relation-flow">
            <button
              type="button"
              class="flow-head"
              aria-expanded={openPairs.has(flowKey(flow))}
              title="Show the entity pairs behind these {flow.total} edges"
              on:click={() => togglePairs(flowKey(flow))}
            >
              <span class="flow-arrow">{arrow(flow)}</span>
              <span class="flow-total">{flow.total}</span>
              <span class="caret">{openPairs.has(flowKey(flow)) ? '▾' : '▸'}</span>
            </button>
            <div class="line dim kinds">{kindLine(flow)}</div>
            {#if openPairs.has(flowKey(flow))}
              <ul class="pairs">
                {#each flow.pairs as p}
                  <li>
                    {#if drawn(p.sourceId)}
                      <button type="button" class="ref" on:click={() => reveal(p.sourceId)}>{p.sourceName}</button>
                    {:else}<span class="ref-flat">{p.sourceName}</span>{/if}
                    <span class="dim">{p.label}</span>
                    {#if drawn(p.targetId)}
                      <button type="button" class="ref" on:click={() => reveal(p.targetId)}>{p.targetName}</button>
                    {:else}<span class="ref-flat">{p.targetName}</span>{/if}
                  </li>
                {/each}
                {#if flow.more > 0}
                  <li class="dim">…and {flow.more} more</li>
                {/if}
              </ul>
            {/if}
          </div>
        {/each}
      {/if}
    </div>

    <!-- 2 and 3. What they agree about, and who agrees about them. Both lists
         are an intersection across every marked side, so a row here is true of
         all of them and not merely of two. -->
    {#each [
      { label: 'Shared dependencies', list: $relation.sharedDeps,
        empty: 'They depend on nothing in common.', probe: 'relation-shared-deps' },
      { label: 'Shared dependents', list: $relation.sharedDependents,
        empty: 'Nothing depends on all of them.', probe: 'relation-shared-users' },
    ] as section}
      <div class="detail-row">
        <!-- Two counts, never summed. The files are the finding; the externals
             are the language, and a reader who cannot tell them apart cannot
             tell a real overlap from `String` and `Option`. -->
        <div class="detail-label">
          {section.label}
          <span class="count">
            {section.list.files} in repo{#if section.list.external > 0} · {section.list.external} external{/if}
          </span>
        </div>
        {#if section.list.rows.length === 0}
          <div class="line dim">{section.empty}</div>
        {:else}
          <ul class="shared" data-probe={section.probe}>
            {#each section.list.rows as s (s.key)}
              <li title={sharedTitle(s)}>
                <span class="shared-label" class:external={s.external}>{s.label}</span>
                {#if s.external}<span class="ext-tag">ext</span>{/if}
                <span class="dim shared-path">{s.external ? '' : s.path}</span>
                <span class="shared-count">{s.total}</span>
              </li>
            {/each}
            {#if hidden(section.list) > 0}
              <li class="dim">…and {hidden(section.list)} more</li>
            {/if}
          </ul>
        {/if}
      </div>
    {/each}

    <!-- 4. Only interesting once the above is empty, and the viewmodel only
         computes it for pairs no direct edge already answers for. -->
    {#if $relation.chains.length > 0}
      <div class="detail-row">
        <div class="detail-label">Connecting routes</div>
        <ul class="chains" data-probe="relation-chains">
          {#each $relation.chains as c}
            <li title="Thinnest hop carries {c.strength} edges">
              <span class="chain-line">{chainLine(c)}</span>
              <span class="shared-count">{c.strength}</span>
            </li>
          {/each}
        </ul>
      </div>
    {/if}

    <!-- What the numbers above were counted over. A reading taken before the
         repo graph arrived is a floor, not a total, and the difference matters
         most for exactly the two lists a reader came here to trust. -->
    {#if !$relation.wholeRepo}
      <p class="caveat" data-probe="relation-scoped">
        {$relateLoading ? 'Loading the whole repo…' : 'Read from the loaded scope only.'}
        Shared dependencies and dependents outside it are not counted yet.
      </p>
    {/if}

    <div class="actions">
      <button type="button" class="act primary" on:click={() => void drillIntoMarks()}
        title="Narrow the canvas to these scopes and close this reading">Drill in ↓</button>
      <button type="button" class="act" on:click={() => clearMarks()}>Clear marks</button>
      <button type="button" class="act" data-probe="relation-close" on:click={closeRelate}>Close</button>
    </div>
  </div>
{/if}

<style>
  .relation { font-size: 0.82rem; }

  .head {
    display: flex;
    align-items: baseline;
    gap: 6px;
    flex-wrap: wrap;
  }

  .kind {
    font-size: 0.62rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 1px 5px;
    border-radius: 2px;
    background: var(--bg-surface-alt);
    color: var(--text-muted);
    border: 1px solid var(--border);
  }

  /* The tone is the one place a colour carries meaning here: a cycle is a
     finding, an independent pair is not. Both stay legible as text — the hue
     is on the border and the background, never on the glyph. */
  .tone {
    font-size: 0.62rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 1px 5px;
    border-radius: 2px;
    border: 1px solid var(--border);
    color: var(--text-secondary);
  }
  .tone-mutual { background: rgba(244, 67, 54, 0.15); border-color: rgba(244, 67, 54, 0.5); }
  .tone-one-way { background: rgba(255, 152, 0, 0.15); border-color: rgba(255, 152, 0, 0.5); }
  .tone-siblings { background: rgba(76, 175, 80, 0.12); border-color: rgba(76, 175, 80, 0.4); }

  .verdict {
    margin: 8px 0 0;
    font-size: 0.86rem;
    line-height: 1.5;
    color: var(--text);
  }

  .detail-row { margin-top: 12px; }

  .detail-label {
    font-size: 0.68rem;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--text-muted);
    margin-bottom: 5px;
  }

  .count {
    color: var(--text-dim);
    letter-spacing: 0;
  }

  .line {
    font-size: 0.78rem;
    line-height: 1.5;
    color: var(--text-secondary);
  }
  .line + .line { margin-top: 3px; }
  .dim { color: var(--text-dim); }

  .side {
    display: flex;
    align-items: baseline;
    gap: 6px;
    flex-wrap: wrap;
  }
  .side-index {
    flex-shrink: 0;
    width: 15px;
    height: 15px;
    line-height: 15px;
    text-align: center;
    border-radius: 50%;
    border: 1px solid var(--border);
    font-size: 0.6rem;
    color: var(--text-muted);
  }
  .side-path { color: var(--text); word-break: break-all; }

  .flow + .flow { margin-top: 8px; }

  .flow-head {
    display: flex;
    align-items: baseline;
    gap: 6px;
    width: 100%;
    padding: 0;
    background: none;
    border: none;
    font: inherit;
    color: var(--text);
    cursor: pointer;
    text-align: left;
  }
  .flow-head:hover .flow-arrow { color: var(--accent); }
  .flow-arrow { font-weight: 600; overflow-wrap: anywhere; }
  .flow-total {
    margin-left: auto;
    font-variant-numeric: tabular-nums;
    color: var(--text-secondary);
  }
  .caret { color: var(--text-dim); font-size: 0.7rem; }
  .kinds { margin-top: 2px; }

  ul { list-style: none; margin: 5px 0 0; padding: 0; }

  .pairs li,
  .shared li,
  .chains li {
    display: flex;
    align-items: baseline;
    gap: 5px;
    padding: 2px 0;
    font-size: 0.75rem;
    line-height: 1.4;
    color: var(--text-secondary);
    border-top: 1px solid var(--border);
  }

  .ref {
    padding: 0;
    background: none;
    border: none;
    font: inherit;
    color: var(--accent);
    cursor: pointer;
    overflow-wrap: anywhere;
  }
  .ref:hover { text-decoration: underline; }
  .ref-flat { color: var(--text); overflow-wrap: anywhere; }

  .shared-label { color: var(--text); font-weight: 600; overflow-wrap: anywhere; }
  .shared-label.external { font-weight: 400; }
  .ext-tag {
    flex-shrink: 0;
    font-size: 0.58rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 0 4px;
    border-radius: 2px;
    border: 1px solid var(--border);
    color: var(--text-dim);
  }
  .shared-path {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    direction: rtl;
    text-align: left;
  }
  .shared-count {
    margin-left: auto;
    flex-shrink: 0;
    font-variant-numeric: tabular-nums;
    color: var(--text-dim);
  }

  .chain-line { overflow-wrap: anywhere; }

  .caveat {
    margin: 12px 0 0;
    font-size: 0.72rem;
    line-height: 1.5;
    color: var(--text-dim);
    font-style: italic;
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 14px;
  }

  .act {
    padding: 3px 10px;
    border-radius: 12px;
    border: 1px solid var(--border);
    background: var(--bg-surface-alt);
    color: var(--text-secondary);
    font: inherit;
    font-size: 0.72rem;
    cursor: pointer;
  }
  .act:hover { background: var(--bg-hover); color: var(--text); }
  .act.primary { border-color: var(--accent); color: var(--accent); }
</style>
