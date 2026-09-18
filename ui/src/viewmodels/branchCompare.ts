/**
 * What comparing two branches actually compares (UI-143).
 *
 * The picker could always be handed a branch name — `From` and `To` are free
 * text and the engine resolves any ref — so the missing half was never the
 * plumbing. It was the *base*. A reviewer asking "what does this branch do"
 * pairs it against `main` and gets main's own last fortnight reported back as
 * work the branch deleted, because the two tips genuinely differ by both. The
 * base that answers the question they asked is where the branches diverged.
 *
 * That is the same rule the stash picker already follows — a stash is compared
 * against the commit it was taken on, not against HEAD (UI-107) — and this is
 * it stated for branches, where the divergence point has to be fetched rather
 * than read off a parent.
 *
 * Pure and store-free, its only imports types, so `node --test` reaches it
 * without svelte:
 *
 *   npm run test:branchcompare
 */

import type { BranchRef, Commit } from '../stores/diff';

/**
 * The branches a repository integrates into, most-conventional first.
 *
 * A guess at the default side of the picker, and only that: it decides which
 * name the two dropdowns open on, and a reviewer whose repository calls it
 * something else changes one dropdown once. Nothing downstream reads it.
 */
const INTEGRATION_BRANCHES = ['main', 'master', 'develop', 'trunk'];

/** How much of a subject rides along in a plan's sentence. */
const SUBJECT_MAX = 48;

function short(subject: string, max = SUBJECT_MAX): string {
  const line = subject.split('\n', 1)[0].trim();
  return line.length <= max ? line : `${line.slice(0, max - 1).trimEnd()}…`;
}

/**
 * The branch list as the two dropdowns show it: local branches first, then
 * remote-tracking ones, each group keeping the server's recency order.
 *
 * Local first because a review is nearly always of something checked out, and
 * `origin/*` is the same branch seen through the last fetch — a list that
 * interleaved them would put two names for one line of work side by side with
 * nothing saying which is stale.
 */
export function orderBranches(all: BranchRef[]): BranchRef[] {
  return [...all.filter((b) => !b.remote), ...all.filter((b) => b.remote)];
}

/** Which two branches the panel opens on. Either may be undefined in a
 *  repository that has too few branches to compare. */
export interface BranchPair {
  base?: BranchRef;
  compare?: BranchRef;
}

/**
 * A first guess at the pair, so the common review opens ready to run.
 *
 * The checked-out branch is the strongest signal there is, and it points in
 * *either* direction depending on which branch it is. Sitting on a feature
 * branch, it is the thing under review and `main` is what it is measured
 * against. Sitting on `main` — which is where a reviewer pulling someone
 * else's work sits — it is the base, and the branch under review is the other
 * one that has been touched most recently.
 *
 * Both are guesses and both are one dropdown away from being changed; what
 * they must never do is arrive equal, because a branch compared against itself
 * is the one pairing that can say nothing at all.
 */
export function defaultPair(all: BranchRef[]): BranchPair {
  const ordered = orderBranches(all);
  const head = ordered.find((b) => b.is_head);
  const integration = (from: BranchRef[]) => INTEGRATION_BRANCHES
    .map((name) => from.find((b) => !b.remote && b.name === name))
    .find(Boolean);

  if (head && integration([head])) {
    // On the integration branch: it is the base, and the review is of whatever
    // else has been worked on — the server's order is most-recent first.
    const compare = ordered.find((b) => !b.remote && b.name !== head.name)
      ?? ordered.find((b) => b.name !== head.name);
    return compare ? { base: head, compare } : { base: undefined, compare: head };
  }

  const compare = head ?? ordered[0];
  if (!compare) return {};
  const others = ordered.filter((b) => b.name !== compare.name);
  return { base: integration(others) ?? others.find((b) => !b.remote) ?? others[0], compare };
}

/** What the panel will send, and what it says it will show. */
export interface BranchPlan {
  /** The ref sent as `from_ref` — a hash for a divergence point, a name
   *  otherwise, because a name means "wherever that branch is". */
  from: string;
  /** The ref sent as `to_ref`. Always the branch's name. */
  to: string;
  /** One sentence naming what the comparison will show. */
  note: string;
  /** Why this pairing cannot be run, or `null`. A refusal is not a failure:
   *  it names a comparison that would be empty or meaningless, and the panel
   *  says so before spending two worktrees and two analyses finding out. */
  refusal: string | null;
}

/**
 * Turn a chosen pair into the comparison to run.
 *
 * `divergedAt` is the server's answer about the pair, `null` for two branches
 * with no commit in common — which is rare and real (an orphan branch, a
 * grafted history), and is why `fromDivergence` cannot simply be assumed to
 * have a base to use.
 *
 * The tip-to-tip reading is kept rather than removed. It is the honest answer
 * to a different question — "how do these two trees differ right now", which
 * is what someone asks before a merge — and the note is what stops the two
 * being mistaken for each other.
 */
export function branchPlan(
  base: BranchRef | undefined,
  compare: BranchRef | undefined,
  divergedAt: Commit | null,
  fromDivergence: boolean,
): BranchPlan {
  if (!base || !compare) {
    return {
      from: '', to: '',
      note: 'Pick a branch on each side.',
      refusal: 'Two branches are needed to compare two branches.',
    };
  }
  if (base.name === compare.name) {
    return {
      from: base.name, to: compare.name,
      note: `${compare.name} against itself.`,
      refusal: 'A branch compared against itself has nothing to show. Pick a different base.',
    };
  }

  if (fromDivergence && divergedAt) {
    // The divergence point *is* the branch's own tip: everything on it is
    // already in the base. Said here rather than left to the engine, which
    // would spend two checkouts and two analyses to arrive at `+0 −0 ~0`.
    if (divergedAt.hash === compare.tip) {
      return {
        from: divergedAt.hash, to: compare.name,
        note: `${compare.name} is already contained in ${base.name}.`,
        refusal:
          `${compare.name} and ${base.name} diverge at ${compare.name}'s own tip, so there `
          + `is nothing on it that ${base.name} does not already have.`,
      };
    }
    return {
      from: divergedAt.hash, to: compare.name,
      note:
        `Everything on ${compare.name} that ${base.name} does not have, measured from where `
        + `they diverged — ${divergedAt.short_hash} ${short(divergedAt.message)}.`,
      refusal: null,
    };
  }

  if (fromDivergence) {
    // Asked for, and unavailable. Falling back silently would show a tip-to-tip
    // comparison under a sentence promising a divergence one.
    return {
      from: base.name, to: compare.name,
      note:
        `${base.name} and ${compare.name} share no history, so there is no point they `
        + `diverged from. Comparing the two tips instead.`,
      refusal: null,
    };
  }

  return {
    from: base.name, to: compare.name,
    note:
      `The tip of ${base.name} against the tip of ${compare.name}. Anything landed on `
      + `${base.name} since the two parted reads here as something ${compare.name} removed.`,
    refusal: null,
  };
}
