---
id: instanceof-expression-fundamentals
language: java
applies-to: [instanceof_expression]
title: "`instanceof` — runtime type checks and pattern binding"
level: beginner
sources:
  - "Java_L Module 11 — Polymorphism (InstanceofPattern)"
  - "JLS §15.20.2 — Type Comparison Operator instanceof"
  - "JEP 394 — Pattern Matching for instanceof (Java 16)"
---

# `instanceof` — runtime type checks and pattern binding

`x instanceof T` answers a single question: at this moment, does the value referenced by `x` belong to a subtype of `T`? It evaluates to a `boolean`, returns `false` for `null`, and is overwhelmingly used as a guard before downcasting. Java 16 added a *pattern* form that does the check and the cast in one expression.

## Syntax

```java
Object n = readNotification();

// Classic form — check, then cast separately.
if (n instanceof EmailNotification) {
    EmailNotification email = (EmailNotification) n;
    send(email.subject, email.recipient);
}

// Pattern form (Java 16+) — check and bind in one expression.
if (n instanceof EmailNotification email) {
    send(email.subject, email.recipient);
}

// The pattern variable is in scope only where the check is provably true.
if (!(n instanceof SmsNotification sms)) {
    return;
}
// here `sms` is in scope — the !-branch returned

// Works in switch since Java 21 for an exhaustive multi-way dispatch.
String summary = switch (n) {
    case EmailNotification e -> "email to " + e.recipient;
    case SmsNotification s   -> "sms to " + s.recipient;
    default                  -> "unknown";
};
```

## Key ideas

**`null instanceof T` is always `false`.** No exception, no special case — `null` belongs to no class, including `Object`. This is the property that makes the pattern form safe: if the input is `null`, the check fails and the bound name never comes into scope.

**The pattern form replaces the most common code-shape mistake.** The classic `if (n instanceof X) X x = (X) n;` repeats the type name three times, and a mismatch between the test type and the cast type compiles cleanly but throws `ClassCastException` at runtime. The pattern form makes the test and the binding one declaration, so they cannot drift apart.

**Pattern variables follow flow-sensitive scoping.** The compiler tracks whether the check is provably true on each branch and only lets you use the name where it is. That's how `if (!(n instanceof X x)) return;` works: the `!` branch returned, so the rest of the method is the branch where the check held. This is the same flow analysis that powers Kotlin's smart-casts.

**Reach for polymorphism before `instanceof`.** Long `instanceof` chains are usually a sign that behaviour should live on the type itself — add an abstract method to the supertype and override it. `instanceof` belongs in code that *receives* an unknown type from outside (deserialisation, event dispatching, equality), not in code that already owns the hierarchy. Sealed interfaces with switch patterns are the modern, exhaustive form when you do need multi-way dispatch.

## Related

- Rule: [`prefer-pattern-instanceof`](../../rules/expressions/prefer-pattern-instanceof.md) — flags the classic `instanceof + cast` shape in favour of the pattern form.
- Lesson: [`interface-declaration-fundamentals`](../oop/interface-declaration.md) — polymorphism is the usual alternative to `instanceof`.
