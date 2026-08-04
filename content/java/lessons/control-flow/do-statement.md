---
id: do-statement-fundamentals
language: java
applies-to: [do_statement]
title: "`do-while` — body first, then condition"
level: beginner
sources:
  - "Java_L Module 05 — Control Flow (WhileLoops)"
  - "JLS §14.13 — The do Statement"
---

# `do-while` — body first, then condition

`do { body } while (cond);` is `while`'s sibling that flips the order: the body runs first, then the condition decides whether to repeat. The guaranteed first execution is the only thing that makes it different from a plain `while`. In practice that's a niche need, so do-while is rare in modern Java.

## Syntax

```java
// Classic "prompt at least once" idiom.
int choice;
do {
    choice = readNextChoice();
    handle(choice);
} while (choice != EXIT);
```

## Key ideas

**The trailing semicolon is required.** `do { … } while (cond);` is one statement; the semicolon closes it. Forgetting it is a syntax error, but on first encounter readers often gloss over the punctuation and misread the structure.

**The condition reads after the body, which inverts how you scan code.** Most loops let you know the exit criterion before stepping into the body. `do-while` makes you read the entire body first, then go back to learn when it ends — small cost per loop, real cost when reviewing dense code. Reach for `do-while` only when the "at least one iteration" guarantee is the point.

**Most do-while loops can be a plain `while` after hoisting.** "Run once, then maybe more" is often clearer as: do the first run as a regular statement, then enter a `while` for the rest. Or restructure the condition so it's true on entry. Keep `do-while` for the cases where neither rewrite reads naturally — typically interactive prompts and parser loops where the first read drives the condition.

**Termination is still your job.** Just like `while`, nothing prevents a runaway. The body must make progress toward making the condition false (or the loop must exit via `break`/`return`/`throw`).

## Related

- Lesson: [`while-statement-fundamentals`](while-statement.md) — the sibling form, and what most do-while loops should be refactored into.
- Lesson: [`for-statement-fundamentals`](for-statement.md) — when you know the count.
