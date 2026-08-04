---
id: try-statement-fundamentals
language: java
applies-to: [try_statement]
title: "`try` / `catch` / `finally` — handling exceptions"
level: intermediate
sources:
  - "Java_L Module 12 — Exceptions (TryCatch, ExceptionTypes, TryWithResources)"
  - "JLS §14.20 — The try statement"
---

# `try` / `catch` / `finally` — handling exceptions

When code might throw an exception, you wrap it in a `try` block and react in `catch` and/or `finally` clauses. Without this structure, an unhandled exception unwinds the call stack until it reaches the JVM and terminates the thread.

## Syntax

```java
try {
    // Code that might throw
    int result = Integer.parseInt(input);
} catch (NumberFormatException e) {
    // React to that specific exception type
    return -1;
} catch (Exception e) {
    // Or a broader category (use sparingly)
    return -1;
} finally {
    // Always runs — exception or no exception, return or no return
    log.info("done");
}
```

## Key ideas

**Catch by type.** Each `catch` clause names an exception type and binds the exception object to a variable. Java picks the **first** matching clause from top to bottom, so order narrow types before broad ones (`NumberFormatException` before `Exception`).

**`finally` always runs.** Whether the body completes normally, returns, or throws — the `finally` block runs before control leaves the `try`. Historically this was where you closed files and released locks. Modern code prefers try-with-resources for that case.

**Try-with-resources.** Java 7 added `try (Resource r = open()) { … }`. Anything declared in the parentheses must implement `AutoCloseable`; the JVM closes it automatically when the block exits, even on exception. This is the right way to handle any `Closeable` resource — files, sockets, database connections, locks.

**Checked vs unchecked.** Java distinguishes two exception flavours. *Checked* exceptions (subclasses of `Exception` but not `RuntimeException`) must be either caught or declared via `throws` — the compiler enforces this. *Unchecked* exceptions (subclasses of `RuntimeException` and `Error`) don't require either. Use checked for recoverable conditions a caller might handle; unchecked for programmer errors and unrecoverable failures.

## Related

- Rule: [`try-with-resources-opportunity`](../../rules/exceptions/try-with-resources-opportunity.md) — when to prefer the resource-managed form.
- Rule: [`checked-exception-over-use`](../../rules/exceptions/checked-exception-over-use.md) — when `throws` clauses become a contract burden.
