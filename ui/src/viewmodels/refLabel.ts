/**
 * How a comparison names the two things it is comparing (UI-139).
 *
 * The Changes pane headed itself `ba8c43b → working`, which is honest and is
 * most of a question: a hash is a name a reader has to go and look up, and the
 * one they were about to look it up in — the commit picker — is the list the
 * app already holds. So the subject goes beside the hash.
 *
 * Two sentinels are not commits and must not be dressed as them. `working` is
 * the tree on disk and `staged` is the index; both are states rather than
 * objects, neither has a subject, and rendering either as a bare lowercase
 * word next to a resolved commit reads as a ref that failed to resolve.
 *
 * Pure and store-free, its only imports being types, so `node --test` reaches
 * it without svelte:
 *
 *   npm run test:reflabel
 */

import type { Commit, Stash } from '../stores/diff';

/** What one side of the comparison renders as. */
export interface RefLabel {
  /** The short form, in the header. */
  text: string;
  /** The long form, on hover — the full hash and the whole subject. */
  title: string;
  /** True when this side is the working tree or the index rather than a
   *  commit. The pane styles them differently: they move under the reader,
   *  and a hash does not. */
  live: boolean;
}

/**
 * How much of a subject survives into the header.
 *
 * The column is narrow and the subject is the *second* thing read — the hash
 * identifies, the subject reminds. A commit whose subject does not fit is
 * still identified by the hash it is sitting next to, so truncating is cheap
 * here in a way it would not be if the subject were the only name.
 */
const SUBJECT_MAX = 52;

/** The first line, which is the subject; the body is not a label. */
export function subjectOf(message: string): string {
  return message.split('\n', 1)[0].trim();
}

export function truncateSubject(subject: string, max = SUBJECT_MAX): string {
  return subject.length <= max ? subject : `${subject.slice(0, max - 1).trimEnd()}…`;
}

/**
 * Find the commit a ref names, in the list the picker already loaded.
 *
 * Matched on either spelling and in both directions, because a ref reaches
 * here in whatever form the thing that produced it used: `diff.json` echoes
 * back what the picker sent, which is a full hash, while a hand-typed `From`
 * may be a seven-character prefix and `git` itself abbreviates to whatever is
 * unambiguous. A prefix comparison covers all of them and cannot collide
 * inside one repository's own list at these lengths.
 */
function findCommit(ref: string, commits: Commit[]): Commit | undefined {
  return commits.find(
    (c) => c.hash === ref || c.short_hash === ref
      || c.hash.startsWith(ref) || ref.startsWith(c.hash),
  );
}

function findStash(ref: string, stashes: Stash[]): Stash | undefined {
  return stashes.find((s) => s.hash === ref || s.hash.startsWith(ref));
}

/**
 * Name one side of a comparison.
 *
 * Order matters: the sentinels are checked first and never looked up, since
 * `working` is not a ref and asking the commit list about it would be asking
 * the wrong question in a way that happens to return nothing. `HEAD` is
 * resolved when the list can — its first entry is HEAD — but is *kept* as the
 * word `HEAD` in the text, because that is what the reader chose and a hash
 * substituted for it would silently stop meaning "wherever I am".
 */
export function refLabel(
  ref: string | undefined,
  commits: Commit[] = [],
  stashes: Stash[] = [],
): RefLabel {
  if (!ref) return { text: '?', title: 'No ref', live: false };

  if (ref === 'working') {
    return {
      text: 'Working tree',
      title: 'The files as they stand on disk, including unsaved work that is not committed anywhere.',
      live: true,
    };
  }
  if (ref === 'staged') {
    return {
      text: 'Staged',
      title: 'The index — what you are about to commit.',
      live: true,
    };
  }

  const stash = findStash(ref, stashes);
  if (stash) {
    const subject = subjectOf(stash.message);
    return {
      text: `${stash.selector} ${truncateSubject(subject)}`.trimEnd(),
      title: `${stash.selector} — ${stash.hash}\n${subject}`,
      live: false,
    };
  }

  const commit = findCommit(ref, commits);
  if (!commit) {
    // A ref the picker's list does not hold: `HEAD~12`, a branch name, a
    // commit older than the window. Named as itself rather than guessed at —
    // the alternative is an empty subject, which reads as a commit with no
    // message rather than as one that was never looked up.
    return { text: ref, title: ref, live: false };
  }

  const subject = subjectOf(commit.message);
  // `HEAD` keeps its own name: it is what the reader picked, and it means
  // "wherever I am", which a hash stops meaning the moment anything lands.
  const name = ref === 'HEAD' ? 'HEAD' : commit.short_hash;
  return {
    text: `${name} ${truncateSubject(subject)}`.trimEnd(),
    title: `${commit.hash}\n${subject}\n\n${commit.author} · ${commit.date}`,
    live: false,
  };
}
