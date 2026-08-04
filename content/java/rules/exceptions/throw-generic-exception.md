---
id: throw-generic-exception
language: java
applies-to: [throw_statement]
match:
  exception_type: { in: [Exception, RuntimeException, Throwable, Error] }
severity: warning
kind: gotcha
sources:
  - "Effective Java, Item 72 — Favor the use of standard exceptions"
  - "Effective Java, Item 73 — Throw exceptions appropriate to the abstraction"
  - "Java_L Module 12 — Exceptions (ThrowingExceptions)"
---

# Don't throw raw `Exception` / `RuntimeException` / `Throwable`

## Bad

```java
public Order chargeOrder(Order order) {
    if (order == null) {
        throw new RuntimeException("order is null");
    }
    if (order.getTotal() < 0) {
        throw new Exception("bad total");
    }
    return payments.charge(order);
}
```

## Good

```java
public Order chargeOrder(Order order) {
    if (order == null) {
        throw new NullPointerException("order must not be null");
    }
    if (order.getTotal() < 0) {
        throw new IllegalArgumentException(
            "order total must be non-negative, got " + order.getTotal());
    }
    return payments.charge(order);
}
```

## Why

A throw of `RuntimeException`/`Exception`/`Throwable`/`Error` tells the caller nothing about what went wrong. A caller who wants to handle just *this* specific failure has to either catch the supertype (and risk swallowing unrelated failures) or string-match on the message (fragile, breaks on i18n). Specific exception types are the API by which `try`/`catch` discriminates between failures — picking a generic one collapses that vocabulary to nothing.

Java's standard exception hierarchy already covers most cases: `IllegalArgumentException` for bad arguments, `IllegalStateException` for "wrong state for this call", `NullPointerException` (yes — `Objects.requireNonNull` throws it) for missing required references, `UnsupportedOperationException` for "this implementation doesn't support that", `NoSuchElementException` for "looked it up but didn't find it". Reach for those before inventing a custom subclass.

`Error` is even worse: it's reserved for JVM-level failures (`OutOfMemoryError`, `StackOverflowError`) that the application code should not be raising or catching. Throwing one from app code abuses a contract the runtime relies on.

When the recoverable failure is something callers will programmatically distinguish — "we tried but the order was already shipped", "the file is locked by another process" — that's the case for a custom subclass. Name it for the domain failure, not for where it was thrown.
