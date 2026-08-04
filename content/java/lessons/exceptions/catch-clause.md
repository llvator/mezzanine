---
id: catch-clause-fundamentals
language: java
applies-to: [catch_clause]
title: "`catch` — handling thrown exceptions"
level: beginner
sources:
  - "Java_L Module 12 — Exceptions (TryCatch)"
  - "JLS §14.20 — The try statement"
  - "Effective Java, Item 77 — Don't ignore exceptions"
  - "Effective Java, Item 75 — Include failure-capture information in detail messages"
---

# `catch` — handling thrown exceptions

A `catch` clause binds a thrown exception to a local name and runs a recovery block. It runs only if the matching `try` body raised an exception assignable to the declared type (or one of the types in a multi-catch). The order of catches matters: the first matching one wins, so more specific types must come first.

## Syntax

```java
try {
    var data = client.fetch(url);
    cache.store(data);
} catch (TimeoutException e) {
    // Specific first — TimeoutException is a subclass of IOException.
    metrics.incrementTimeout();
    throw new ServiceUnavailableException("upstream slow", e);
} catch (IOException | SQLException e) {
    // Multi-catch — same handling, different declared types.
    log.warn("fetch failed", e);
    throw new ServiceUnavailableException("upstream failed", e);
} catch (RuntimeException e) {
    // Catching RuntimeException is OK only when you have a specific recovery
    // strategy. A bare "log and continue" is almost always wrong.
    log.error("unexpected runtime failure", e);
    throw e;   // re-throw so the caller still sees it
}
```

## Key ideas

**An empty `catch` body silently swallows the failure.** No log, no rethrow, no metric — the caller thinks the `try` succeeded and runs on with corrupt state. *Effective Java*'s Item 77 calls this out explicitly: it is the single most damaging exception-handling anti-pattern. If the exception really is expected and recoverable, leave a one-line comment explaining *why* you're ignoring it.

**Catch the most specific type you can act on.** A `catch (Exception e)` says "I will handle anything that can go wrong here," which is rarely true. Catch the specific type whose recovery you actually implement (`FileNotFoundException`, `JsonParseException`), and let the rest propagate to a higher-level handler that does have a strategy.

**Multi-catch (Java 7+) is the way to share handlers across types.** `catch (A | B e)` lets one block handle several exception classes without duplicating the body. The compiler treats `e`'s static type as the common supertype, so you can only call methods that exist on all of them.

**Wrap before rethrowing when you cross an abstraction boundary.** A persistence layer should not leak `SQLException` to a web controller. Catch it, wrap it in something the layer above understands (`RepositoryException`), and pass the original as the cause so the stack trace and `getCause()` still expose the root.

**Don't lose the exception chain.** `throw new MyException("…", e)` preserves the original. `throw new MyException("…")` (no cause) erases it — the new exception's stack trace points only at the rethrow site, not at the failure. This is the single most common reason a bug report shows a useless stack trace.

## Related

- Rule: [`empty-catch-block`](../../rules/exceptions/empty-catch-block.md) — flags catches with no body, the canonical swallow-exception bug.
- Lesson: [`try-statement-fundamentals`](try-statement.md) — the block whose failures `catch` handles.
- Lesson: [`throw-statement-fundamentals`](throw-statement.md) — what reaches a catch clause and what to wrap-and-rethrow with.
