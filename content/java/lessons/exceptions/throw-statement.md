---
id: throw-statement-fundamentals
language: java
applies-to: [throw_statement]
title: "`throw` — raising an exception"
level: beginner
sources:
  - "Java_L Module 12 — Exceptions (ThrowingExceptions)"
  - "JLS §14.18 — The throw Statement"
  - "Effective Java, Item 72 — Favor the use of standard exceptions"
  - "Effective Java, Item 73 — Throw exceptions appropriate to the abstraction"
---

# `throw` — raising an exception

`throw expr;` interrupts normal control flow and starts looking for a matching `catch` somewhere up the call stack. The expression must evaluate to a `Throwable`; in practice it is almost always either a freshly-constructed exception (`throw new IllegalStateException(…)`) or a caught exception being rethrown (`throw e;`).

## Syntax

```java
public Order chargeOrder(Order order) {
    // Precondition — throw before doing any real work.
    if (order == null) {
        throw new NullPointerException("order must not be null");
    }
    if (order.getTotal() < 0) {
        throw new IllegalArgumentException(
            "order total must be non-negative, got " + order.getTotal());
    }
    try {
        return payments.charge(order);
    } catch (NetworkException e) {
        // Re-throw with a higher-level abstraction; preserve the cause.
        throw new OrderProcessingException("could not charge order " + order.getId(), e);
    }
}
```

## Key ideas

**Throw the most specific standard exception that fits.** `IllegalArgumentException` for bad arguments, `IllegalStateException` for "the object is not in a state where this call makes sense", `NullPointerException` for missing required arguments (yes, NPE — `Objects.requireNonNull` throws it), `UnsupportedOperationException` for "this implementation doesn't support that call". Inventing a custom subclass is only worth it when callers will programmatically distinguish *this* failure from others.

**Don't throw raw `Exception`, `RuntimeException`, or `Throwable`.** A catcher who wants to handle just *your* failure can't — they have to catch the supertype and risk swallowing unrelated failures. Be specific so callers can be specific too.

**Always include enough message context to diagnose the failure once.** The message reaches a developer reading a stack trace, possibly years later, possibly without local repro. "value out of range" tells nobody anything; "rate must be 0–1, got -0.5" tells them what was wrong without needing to read your code.

**Preserve the cause when wrapping.** Re-throwing as a higher-level exception is great — losing the underlying cause is not. Pass the original exception as the second argument (`new MyException("message", cause)`) so the stack trace shows both layers and `Throwable.getCause()` exposes the root.

**Checked vs unchecked changes how it shows up at the call site.** Throwing a checked exception (anything not extending `RuntimeException`) forces every caller to either catch it or declare it in their own `throws` — which is why `IllegalArgumentException` (unchecked) is preferred for programmer errors and a checked exception is reserved for recoverable conditions the caller really should think about.

## Related

- Rule: [`throw-generic-exception`](../../rules/exceptions/throw-generic-exception.md) — flags throws of raw `Exception` / `RuntimeException` / `Throwable`.
- Lesson: [`try-statement-fundamentals`](try-statement.md) — the other end of the exception flow.
- Lesson: [`catch-clause-fundamentals`](catch-clause.md) — handling what was thrown.
