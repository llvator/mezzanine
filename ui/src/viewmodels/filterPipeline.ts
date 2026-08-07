/**
 * What is narrowing the canvas, said out loud (UI-099).
 *
 * A dozen independent things can remove a node from the picture — a kind
 * unticked in a panel three scroll-lengths down, a committed search, a spec
 * entity focused in the other pane, a diff rung, a selection whose BFS reach
 * is the view, hidden files remembered from a scope the reader has since left.
 * Every one of them is correct on its own and none of them is *on screen*. The
 * reader sees a graph with something missing and no way to ask why, which is
 * the same silence `emptyCanvas` was opened for — except that a *partly*
 * filtered canvas never triggers that card, so it stayed silent forever.
 *
 * This module answers "what is applying right now", in the order the filters
 * actually apply in `displayPlan`. It is deliberately a **description of state
 * the reader already caused**, not a new control surface: every stage here is
 * something they can already change in the Filters pane, and the strip's only
 * new power is a `×` per stage.
 *
 * Pure — a plain input object in, a list out, no stores — for the reason
 * `scopeCrumbs` and `drawCeiling` are: the interesting failure is a strip that
 * *lies* (claims a filter that is off, or stays quiet about one that is on),
 * and that is a property worth testing without a browser.
 *
 *   npm run test:pipeline
 */

import type { DiffLevel } from './diffLevels';

/**
 * The stages, in the order `displayPlan` applies them.
 *
 * The order is the whole reason this is called a pipeline rather than a list:
 * "spec, then kinds, then search, then diff, then focus" explains why a search
 * that matched forty entities put eight on screen — the four stages after it
 * each took their cut. A set of unordered badges cannot say that.
 */
export type FilterStageId =
  | 'ghosts'
  | 'template-vars'
  | 'spec'
  | 'kinds'
  | 'languages'
  | 'files'
  | 'search'
  | 'diff'
  | 'focus'
  | 'relations'
  | 'levels'
  | 'hubs';

/** The application order. Exported so the test can assert against it rather
 *  than against a hand-copied list that would drift. */
export const STAGE_ORDER: readonly FilterStageId[] = [
  'ghosts',
  'template-vars',
  'spec',
  'kinds',
  'languages',
  'files',
  'search',
  'diff',
  'focus',
  'relations',
  'levels',
  'hubs',
];

export interface FilterStage {
  id: FilterStageId;
  /** What the filter is, in one or two words. */
  name: string;
  /** What it is set to right now. */
  value: string;
  /** The full sentence, on hover: what it removes, and what it leaves. */
  detail: string;
  /** What the `×` promises. Names the restoration, not the gesture — "Show
   *  every entity kind" rather than "Clear", so a strip of eight `×` buttons
   *  is still eight distinguishable actions. */
  clearHint: string;
  /** Stages that dim rather than hide. Worth distinguishing on the chip: a
   *  reader hunting a missing node should be told when it is still there. */
  dims: boolean;
}

/** Something the user unticked out of a dataset-derived list. */
export interface HiddenSet {
  /** Names of what is currently excluded, for the tooltip. */
  hidden: string[];
  /** How many the current dataset offers. */
  total: number;
}

export interface PipelineInput {
  /** Entity kinds — `generalEntityTypes` against `allEntityTypes`. */
  kinds: HiddenSet;
  /** The panel's Relationships section: which edge kinds are drawn, and in
   *  which direction. One chip per panel section is the rule the whole strip
   *  follows — a reader who clicks a chip has to land somewhere that holds
   *  the control it named. */
  relations: HiddenSet & { outgoingHidden: boolean; incomingHidden: boolean };
  languages: HiddenSet;
  /** Files: the names are paths, and there can be hundreds, so the tooltip
   *  shows the first few. Counted against the current dataset, never against
   *  the whole remembered exclusion set — a file hidden in a scope you have
   *  left is not filtering this picture. */
  files: HiddenSet;
  ghosts: {
    /** `showGhostNodes` is off: no external symbol is drawn at all. */
    allHidden: boolean;
    /** `showBuiltinGhosts` is off — the default. */
    builtinsHidden: boolean;
    /** Whether the graph holds any, at all / of the builtin sort. A filter
     *  with nothing to filter is not worth a chip. */
    present: boolean;
    builtinsPresent: boolean;
  };
  templateVars: { hidden: boolean; present: boolean };
  /** The spec pane's cross-filter. `null` when none is running. `paths: 0` is
   *  a real and important state — the focused entity declares no code — and is
   *  named rather than folded into "no filter" (see `stores/crossFilter.ts`). */
  spec: { entities: string[]; paths: number } | null;
  /** A *committed* search. Typing alone filters nothing and gets no chip. */
  search: { term: string; kept: number; hides: boolean } | null;
  /** The diff ladder, only when a diff is loaded *and* its master toggle is
   *  on. A diff that only colours the graph is not filtering it. */
  diff: { level: DiffLevel; dims: boolean } | null;
  /** A selection in the current graph: the canvas is its BFS reach. */
  focus: { name: string; depth: number; mode: 'force' | 'tree' } | null;
  /**
   * The panel's Level Filters section.
   *
   * Every rule in it is scoped to a selection — with nothing selected the view
   * has no levels to have rules about — so this stage, and the direction
   * toggles above, are suppressed when `focus` is null. Reporting a rule that
   * cannot currently bite is the second failure mode this module is built to
   * avoid, and it would fire on almost every canvas.
   */
  levels: {
    directHidden: boolean;
    crossLevelHidden: boolean;
    peerHiddenLevels: number[];
    /** Per-level tri-states explicitly set to `off`. Counted, not listed:
     *  the panel that sets them is where the detail belongs. */
    overridesOff: number;
  };
  /** `demoteHubs`, with how many are muted. Hides edges, never nodes. */
  hubs: { count: number } | null;
}

const plural = (n: number, one: string, many = `${one}s`) => (n === 1 ? one : many);

/** Up to `max` names, then a count. A tooltip listing 180 file paths is the
 *  same silence as no tooltip. */
export function nameList(names: string[], max = 6): string {
  if (names.length <= max) return names.join(', ');
  return `${names.slice(0, max).join(', ')}, and ${names.length - max} more`;
}

/**
 * The deepest level the view actually expands to.
 *
 * Mirrors `displayPlan`'s `getMaxLevel`: the tree-depth control caps it, and a
 * level switched off in the panel below that cap lowers it further. Reported
 * rather than `treeMaxDepth` alone, because "depth 3" over a view that stops
 * at 1 is exactly the kind of confident wrong answer this strip exists to
 * remove.
 */
export function effectiveDepth(
  levels: Record<number, { enabled: boolean } | undefined>,
  cap: number,
): number {
  for (let i = cap; i >= 1; i--) {
    if (levels[i]?.enabled) return i;
  }
  return 0;
}

const DIFF_VALUE: Record<DiffLevel, string> = {
  edits: 'edits only',
  rewiring: 'edits + rewiring',
  neighbourhood: 'edits + one hop',
};

const DIFF_DETAIL: Record<DiffLevel, string> = {
  edits:
    'Only entities the diff calls edited — their own source moved — plus '
    + 'everything added and removed. Impact-only ripple is dropped.',
  rewiring:
    'The edits, plus the far end of every relationship that appeared, even '
    + 'when that entity was never touched.',
  neighbourhood:
    'The edits, plus every direct neighbour of a changed entity, with all '
    + 'the wiring between what is shown.',
};

/**
 * Every filter currently narrowing the canvas, in application order.
 *
 * Returns `[]` when nothing is. That is the common case and the strip draws
 * nothing for it: a permanent bar reading "no filters" is one more thing to
 * read past on the way to the graph.
 */
export function filterPipeline(input: PipelineInput): FilterStage[] {
  const stages: FilterStage[] = [];

  // --- Ghosts. Two toggles, one chip: they answer the same question ("why
  // is the call to `len` not drawn"), and the builtin one is off by default,
  // so a reader who never chose it is the most likely to be puzzled by it.
  if (input.ghosts.allHidden && input.ghosts.present) {
    stages.push({
      id: 'ghosts',
      name: 'Ghosts',
      value: 'all hidden',
      detail:
        'Symbols that are called but not defined in this repo — library and '
        + 'builtin functions — are not drawn. Their callers still are.',
      clearHint: 'Draw external symbols',
      dims: false,
    });
  } else if (input.ghosts.builtinsHidden && input.ghosts.builtinsPresent) {
    stages.push({
      id: 'ghosts',
      name: 'Ghosts',
      value: 'builtins hidden',
      detail:
        'Calls into the standard library (print, len, Vec, console…) are not '
        + 'drawn. This is the default — it keeps builtin noise out of the '
        + 'business logic, and it is on until you turn it off.',
      clearHint: 'Draw builtin symbols',
      dims: false,
    });
  }

  if (input.templateVars.hidden && input.templateVars.present) {
    stages.push({
      id: 'template-vars',
      name: 'Templating',
      value: 'hidden',
      detail:
        'The ansible templating layer — the {{ … }} config variables and their '
        + 'definitions — is not drawn. Off by default: it is high-volume and '
        + 'buries the deploy topology.',
      clearHint: 'Draw the templating layer',
      dims: false,
    });
  }

  if (input.spec) {
    const { entities, paths } = input.spec;
    stages.push({
      id: 'spec',
      name: 'Spec',
      value: entities.length === 1 ? entities[0] : `${entities.length} entities`,
      detail:
        paths === 0
          ? `${nameList(entities)} declares no code (no cr: paths), so nothing `
            + 'passes this filter. That is an answer about the spec, not an '
            + 'empty selection.'
          : `Only code under the ${paths} ${plural(paths, 'path')} declared by `
            + `${nameList(entities)} is drawn. The scope itself is untouched.`,
      clearHint: 'Stop filtering by the spec',
      dims: false,
    });
  }

  if (input.kinds.hidden.length > 0) {
    const n = input.kinds.hidden.length;
    stages.push({
      id: 'kinds',
      name: 'Kinds',
      value: `${input.kinds.total - n} of ${input.kinds.total}`,
      detail:
        `${n} entity ${plural(n, 'kind')} hidden: ${nameList(input.kinds.hidden)}.`,
      clearHint: 'Show every entity kind',
      dims: false,
    });
  }

  if (input.languages.hidden.length > 0) {
    const n = input.languages.hidden.length;
    stages.push({
      id: 'languages',
      name: 'Languages',
      value: `${input.languages.total - n} of ${input.languages.total}`,
      detail: `${n} ${plural(n, 'language')} hidden: ${nameList(input.languages.hidden)}.`,
      clearHint: 'Show every language',
      dims: false,
    });
  }

  if (input.files.hidden.length > 0) {
    const n = input.files.hidden.length;
    stages.push({
      id: 'files',
      name: 'Files',
      value: `${n} hidden`,
      detail:
        `${n} ${plural(n, 'file')} in this scope excluded from the picture: `
        + `${nameList(input.files.hidden, 4)}. They are still analysed, and the `
        + 'exclusion survives a scope change on purpose.',
      clearHint: 'Un-hide every file',
      dims: false,
    });
  }

  if (input.search) {
    const { term, kept, hides } = input.search;
    stages.push({
      id: 'search',
      name: 'Search',
      value: `“${term}” · ${kept} kept`,
      detail: hides
        ? `${kept} committed ${plural(kept, 'match', 'matches')} and their direct `
          + 'neighbours are drawn; everything else is removed.'
        : `${kept} committed ${plural(kept, 'match', 'matches')} and their direct `
          + 'neighbours are drawn at full strength; everything else is dimmed, '
          + 'not removed — the surroundings are what make a hit an answer.',
      clearHint: 'Clear the search',
      dims: !hides,
    });
  }

  if (input.diff) {
    stages.push({
      id: 'diff',
      name: 'Diff',
      value: DIFF_VALUE[input.diff.level],
      detail:
        `${DIFF_DETAIL[input.diff.level]}\n\nTurning this off keeps the diff `
        + 'colours and stops it filtering.',
      clearHint: 'Stop filtering by the diff',
      dims: input.diff.dims,
    });
  }

  if (input.focus) {
    const { name, depth, mode } = input.focus;
    stages.push({
      id: 'focus',
      name: 'Focus',
      value: `${name} · ${depth} ${plural(depth, 'hop')}`,
      detail:
        depth === 0
          ? `The canvas is ${name} alone — every relationship level is switched `
            + 'off, so nothing around it is reached.'
          : mode === 'tree'
            ? `Tree view is rooted at ${name}: only what it reaches within `
              + `${depth} ${plural(depth, 'hop')} is drawn.`
            : `Only what ${name} reaches within ${depth} ${plural(depth, 'hop')} `
              + 'is drawn. This is usually the one that surprises — selecting a '
              + 'node narrows the canvas as well as filling the Details pane.',
      clearHint: 'Clear the selection and show the whole scope',
      dims: false,
    });
  }

  // Direction only bites while something is selected — it steers the BFS out
  // of the selection, and there is no BFS without one.
  const directional = input.focus !== null;
  const relParts: string[] = [];
  const hiddenRels = input.relations.hidden.length;
  if (hiddenRels > 0) {
    relParts.push(`${input.relations.total - hiddenRels} of ${input.relations.total}`);
  }
  if (directional && input.relations.outgoingHidden) relParts.push('incoming only');
  if (directional && input.relations.incomingHidden) relParts.push('outgoing only');
  if (relParts.length > 0) {
    const detail: string[] = [];
    if (hiddenRels > 0) {
      detail.push(
        `${hiddenRels} relationship ${plural(hiddenRels, 'kind')} not drawn: `
        + `${nameList(input.relations.hidden)}.`,
      );
    }
    if (directional && (input.relations.outgoingHidden || input.relations.incomingHidden)) {
      detail.push(
        input.relations.outgoingHidden && input.relations.incomingHidden
          ? 'Neither direction is followed out of the selection, so nothing '
            + 'around it is reached.'
          : input.relations.outgoingHidden
            ? 'Only what depends on the selection is followed; what it depends '
              + 'on is not.'
            : 'Only what the selection depends on is followed; what depends on '
              + 'it is not.',
      );
    }
    stages.push({
      id: 'relations',
      name: 'Relations',
      value: relParts.join(' · '),
      detail: detail.join(' '),
      clearHint: 'Draw every relationship, both directions',
      dims: false,
    });
  }

  const lv = input.levels;
  const levelParts: string[] = [];
  if (directional && lv.directHidden) levelParts.push('direct edges hidden');
  if (directional && lv.crossLevelHidden) levelParts.push('cross-level edges hidden');
  if (directional && lv.peerHiddenLevels.length > 0) {
    levelParts.push(`peer edges off at L${lv.peerHiddenLevels.join(', L')}`);
  }
  if (directional && lv.overridesOff > 0) {
    levelParts.push(`${lv.overridesOff} kind ${plural(lv.overridesOff, 'override')} set to off`);
  }
  if (levelParts.length > 0) {
    stages.push({
      id: 'levels',
      name: 'Levels',
      value: levelParts.length === 1 ? levelParts[0] : `${levelParts.length} rules`,
      detail:
        `Per-level rules in force: ${levelParts.join('; ')}. These shape what `
        + 'the view reaches out of the selection — a kind switched off at a '
        + 'level keeps those entities out of the picture entirely, not just '
        + 'their edges.',
      clearHint: 'Reset the level filters',
      dims: false,
    });
  }

  if (input.hubs) {
    stages.push({
      id: 'hubs',
      name: 'Hubs',
      value: `top ${input.hubs.count} muted`,
      detail:
        `Incoming edges into the ${input.hubs.count} most-depended-on nodes in `
        + 'this view are suppressed. The nodes are still drawn and still '
        + 'selectable — only the inbound hairball is gone.',
      clearHint: 'Draw hub edges again',
      dims: false,
    });
  }

  return stages;
}

/** One line for a folded toolbar, or a screen reader: "3 filters: Kinds,
 *  Search, Focus". */
export function pipelineSummary(stages: FilterStage[]): string {
  if (stages.length === 0) return 'No filters are narrowing the canvas.';
  return `${stages.length} ${plural(stages.length, 'filter')}: `
    + stages.map((s) => s.name).join(', ');
}
