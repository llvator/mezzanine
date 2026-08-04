---
id: while-statement-fundamentals
language: java
applies-to: [while_statement]
title: "`while` — condition-checked iteration"
level: beginner
sources:
  - "Java_L Module 05 — Control Flow (WhileLoops)"
  - "JLS §14.12 — The while Statement"
---

# `while` — condition-checked iteration

`while (cond) body` repeats the body as long as the condition stays true. The condition is checked *before* each iteration, so an initially-false condition means the body never runs at all. Reach for `while` when the iteration count isn't known up-front; reach for `for` when it is.

## Syntax

```java
// Standard form — check then run.
int remaining = orders.size();
while (remaining > 0) {
    process(orders.get(--remaining));
}

// "Process until exhausted" idiom — common with iterators and streams.
Iterator<Order> it = orders.iterator();
while (it.hasNext()) {
    process(it.next());
}

// Sentinel-loop variant — read until end-of-stream.
String line;
while ((line = reader.readLine()) != null) {
    handle(line);
}
```

## Key ideas

**Termination is your responsibility.** Nothing in the language stops a wrong condition from looping forever. The body must move the world toward making the condition false — increment a counter, consume an iterator, shrink a remaining-work value. If you can't point at the line that progresses, the loop probably doesn't terminate.

**Zero iterations is a feature, not a bug.** Because the check happens first, `while (collection.isEmpty()) { … }` correctly does nothing on empty input. This is exactly what you want for "process all of these, however many there are." If you need *at least one* iteration regardless, that's the `do-while` form ([`do-statement-fundamentals`](do-statement.md)).

**`while (true)` is an explicit infinite loop.** It's a deliberate signal that the loop terminates from inside via `break`, `return`, or `throw`. That's fine when the exit conditions are too complex for the header (multiple sources of "we're done"), but each `while (true)` should have an obvious exit. If yours doesn't, refactor the exit into the condition.

**Braces apply here too.** `while (cond) foo();` has the same dangling-statement footgun as `if (cond) foo();` — adding a second line under the same indentation silently runs it unconditionally. Brace every loop body, even one-liners.

## Related

- Rule: [`while-true-needs-clear-exit`](../../rules/control-flow/while-true-needs-clear-exit.md) — flags `while (true)` so you can document the exit path.
- Lesson: [`for-statement-fundamentals`](for-statement.md) — when you know the iteration count, prefer `for`.
- Lesson: [`do-statement-fundamentals`](do-statement.md) — when you need at least one execution.
