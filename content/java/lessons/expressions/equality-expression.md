---
id: equality-expression-fundamentals
language: java
applies-to: [equality_expression]
title: "`==` and `!=` — reference vs value equality"
level: beginner
sources:
  - "Java_L Module 04 — Operators (RelationalOperators, EqualityAndComparison)"
  - "JLS §15.21 — Equality Operators"
---

# `==` and `!=` — reference vs value equality

Java has two distinct notions of equality, and the `==` operator covers only one of them. Getting this wrong is one of the most common Java bugs.

## Syntax

```java
// Primitives — value comparison, works as you'd expect
int a = 5;
int b = 5;
boolean same = a == b;          // true

// Object references — identity comparison
String x = new String("hi");
String y = new String("hi");
boolean identical = x == y;     // false — different objects
boolean equal = x.equals(y);    // true  — same content
```

## Key ideas

**For primitives, `==` compares values.** `5 == 5` is `true`. Floats are a slight exception — `==` works but is fragile because `0.1 + 0.2 != 0.3` exactly (use a tolerance for floating-point comparisons).

**For object references, `==` compares object identity.** It asks "do these two variables point to the literal same object in memory?" That is almost never the question you actually want. `"hi" == new String("hi")` is `false` — they are different objects holding equal content. Use `.equals(other)` for content comparison.

**Boxed primitives bite hard.** `Integer a = 1000; Integer b = 1000; a == b` returns `false` even though both hold `1000` — they're two different `Integer` objects. The JVM caches small `Integer` values (`-128..=127`), so within the cache range `==` *happens* to work, and code that worked in testing breaks in production when the values grow. Always use `.equals` for boxed types.

**Enums are an exception.** Enum constants have exactly one instance per declared value, so `Color.RED == Color.RED` is always `true`. `==` on enums is preferred over `.equals` because it's null-safe and reads more like a value comparison.

**`null` is identity-only.** `something == null` is the correct way to check for null. `null.equals(x)` would throw `NullPointerException`. Use `Objects.equals(a, b)` when either side might be null.

## Related

- Rule: [`boxed-equality`](../../rules/expressions/boxed-equality.md) — fires when `==` compares locally-declared boxed-primitive variables.
- Rule: [`equals-without-hashcode`](../../rules/oop/equals-without-hashcode.md) — when overriding `.equals` requires also overriding `.hashCode`.
