<script lang="ts">
  /**
   * One row of the Changes tree — a folder and everything under it, or a file
   * (UI-154).
   *
   * The folder row does two things and keeps them on two controls, which is
   * the whole of the design here. Its name toggles the subtree, because that
   * is what a name in a tree does everywhere else and a reader expanding their
   * way down should not be re-scoping the canvas at every step. The target
   * beside it filters the graph to that folder, which is the question a tree
   * makes askable in the first place: the list says three files changed under
   * `ui/src/stores`, and the next thing anyone wants is to look at what that
   * folder is.
   *
   * `path` is the folder's own, after compaction — a row labelled
   * `src/parser/rust` scopes to the rust folder, not to `src`. See
   * `viewmodels/changeTree.ts`.
   */
  import ChangeRow from './ChangeRow.svelte';
  import { collapsedChangeFolders, toggleChangeFolder } from '../stores/changedFiles';
  import { addScopes, drillIn, selectedScopes } from '../stores/scope';
  import type { ChangeTreeNode } from '../viewmodels/changeTree';

  export let node: ChangeTreeNode;
  export let depth = 0;

  /** One level of nesting, in pixels. Deliberately small: paths here are four
   *  and five deep and the pane is 360px wide, so a generous indent would
   *  spend the column the filenames need. */
  const INDENT = 10;

  $: open = node.kind === 'folder' && !$collapsedChangeFolders.has(node.path);
  $: scoped = node.kind === 'folder' && $selectedScopes.has(node.path);

  /**
   * Point the graph at this folder.
   *
   * Plain click replaces the scope, which is what "show me this folder" means
   * and what drilling into a folder circle on the canvas already does. Held
   * modifier adds it instead, so two folders of one change can be looked at
   * together — the same pair of gestures the scope tree offers, named in the
   * tooltip because a modifier nobody is told about is a modifier nobody uses.
   */
  function scope(path: string, event: MouseEvent) {
    if (event.shiftKey || event.altKey || event.metaKey) void addScopes([path]);
    else void drillIn(path);
  }
</script>

{#if node.kind === 'file'}
  <ChangeRow row={node.row} showDir={false} indent={depth * INDENT} />
{:else}
  {@const folder = node}
  <div class="folder-row" style="padding-left: {depth * INDENT}px">
    <button
      type="button"
      class="disclose"
      data-probe="change-folder"
      data-path={folder.path}
      aria-expanded={open}
      title={`${open ? 'Collapse' : 'Expand'} ${folder.path} — ${folder.totals.files} changed ${folder.totals.files === 1 ? 'file' : 'files'} below it.`}
      on:click={() => toggleChangeFolder(folder.path)}
    >
      <span class="chevron" aria-hidden="true">{open ? '▼' : '▶'}</span>
      <span class="label">{folder.label}</span>
    </button>
    <span class="totals" aria-hidden="true">
      <span class="files">{folder.totals.files}</span>
      <span class="add">+{folder.totals.additions}</span>
      <span class="del">−{folder.totals.deletions}</span>
    </span>
    <button
      type="button"
      class="scope"
      class:on={scoped}
      data-probe="change-folder-scope"
      data-path={folder.path}
      aria-label={`Filter the graph to ${folder.path}`}
      title={`Filter the graph to ${folder.path} — the canvas is re-scoped to this folder, all of it, not only the files that changed.\n\nHold shift to add it to the current scope instead of replacing it.`}
      on:click={(e) => scope(folder.path, e)}
    >
      <!-- A target rather than a funnel: this points the canvas at a place,
           and the funnel glyph is already the facet chips' job one pane up. -->
      <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round">
        <circle cx="12" cy="12" r="6" />
        <path d="M12 1v3M12 20v3M1 12h3M20 12h3" />
      </svg>
    </button>
  </div>

  {#if open}
    {#each folder.children as child (child.kind + child.path)}
      <svelte:self node={child} depth={depth + 1} />
    {/each}
  {/if}
{/if}

<style>
  .folder-row {
    display: flex;
    align-items: center;
    gap: 4px;
    padding-right: 2px;
  }
  .folder-row:hover { background: color-mix(in srgb, var(--bg-hover) 40%, transparent); }

  .disclose {
    flex: 1 1 auto;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 3px;
    padding: 2px 2px;
    border: none;
    border-radius: 3px;
    background: transparent;
    color: var(--text-secondary);
    font: inherit;
    font-size: 0.74rem;
    text-align: left;
    cursor: pointer;
  }
  .disclose:hover { color: var(--text); }
  .chevron { flex: none; width: 9px; color: var(--text-dim); font-size: 0.6rem; }
  .label { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

  .totals {
    flex: none;
    display: flex;
    gap: 4px;
    font-size: 0.65rem;
    font-family: 'Monaco', 'Menlo', monospace;
  }
  .files { color: var(--text-dim); }
  /* Add/remove hues identify a state and stay put across themes — the rule
     the rows and SourceDiff follow, and the reason these are literals. */
  .add { color: #4CAF50; }
  .del { color: #F44336; }

  .scope {
    flex: none;
    display: inline-flex;
    align-items: center;
    padding: 2px;
    border: none;
    border-radius: 3px;
    background: transparent;
    /* Dim until wanted: the row is read far more often than it is scoped on,
       and a control at full contrast on every folder would compete with the
       filenames. Never hidden until hover, though — a control that only
       exists under a pointer is one a keyboard reader never finds. */
    color: var(--text-dim);
    cursor: pointer;
  }
  .scope:hover { color: var(--accent); background: var(--bg-hover); }
  .scope:focus-visible { color: var(--accent); }
  /* The folder the graph is already scoped to, so the click's effect is
     visible on the row that caused it. */
  .scope.on { color: var(--accent); }
</style>
