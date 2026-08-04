---
id: for-statement-fundamentals
language: java
applies-to: [for_statement]
title: "`for` loops — classic, enhanced, and when each is right"
level: beginner
sources:
  - "Java_L Module 05 — Control Flow (ForLoops)"
  - "JLS §14.14 — The for Statement"
  - "Effective Java, Item 58 — Prefer for-each loops to traditional for loops"
---

# `for` loops — classic, enhanced, and when each is right

Java has two `for` loop forms with the same keyword but very different shapes. Knowing which to reach for is one of the first reading-the-code habits to internalise.

## Syntax

```java
// 1. Classic (C-style) — three clauses: init, condition, update.
for (int i = 0; i < items.size(); i++) {
    process(items.get(i), i);
}

// 2. Enhanced for-each — element pulled from a collection or array.
for (ProductPromotionItemModel promoItem : promoItems) {
    process(promoItem);
}
```

## Key ideas

**Classic for has three independent clauses.** The init runs once before entering, the condition is checked *before* each iteration (so the body may run zero times), and the update runs *after* each iteration. Any clause can be empty: `for (;;) { … }` is a valid infinite loop. The init's scope is the loop body — variables declared there don't leak past the closing brace.

**Enhanced for is a desugar over Iterable / arrays.** For collections, the compiler rewrites `for (T x : coll)` into the equivalent `Iterator` walk: `Iterator<T> it = coll.iterator(); while (it.hasNext()) { T x = it.next(); … }`. For arrays it's an indexed walk you don't see. The loop variable is read-only — assigning to `x` inside the body does *not* change the underlying collection.

**Pick enhanced when the index doesn't matter.** Iterating "every element once, in order, without modifying the collection" is the enhanced-for sweet spot — it's shorter, harder to off-by-one, and works identically for arrays, Lists, Sets, and any `Iterable`. Use classic when you genuinely need the index (parallel arrays, accessing neighbours, reverse iteration), need to remove elements (`Iterator.remove()`), or need to update the loop variable mid-body.

**Termination is your responsibility.** Both forms can run forever. Classic with a wrong condition; enhanced if the iterable is infinite (a `Stream.generate(…)` adapted to `Iterable`, or a custom iterator that never returns false from `hasNext()`). Don't assume "the loop will end" — pick a condition that provably progresses, and prefer enhanced-for since it removes the "did I update `i`?" failure mode entirely.

## Related

- Lesson: [`lambda-expression-fundamentals`](../expressions/lambda-expression.md) — once you reach for `collection.forEach(x -> …)`, you've left the loop family entirely.
