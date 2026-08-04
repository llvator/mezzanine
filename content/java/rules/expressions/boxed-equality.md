---
id: boxed-equality
language: java
applies-to: [equality_expression]
match:
  lhs_type:
    in: [Integer, Long, Boolean, Character, Double, Float, Short, Byte]
  rhs_type:
    in: [Integer, Long, Boolean, Character, Double, Float, Short, Byte]
severity: warning
kind: gotcha
sources:
  - "Effective Java, Item 61 — Prefer primitive types to boxed primitives"
---

# Compare boxed primitives with `.equals`, not `==`

## Bad

```java
public boolean isAdult(Integer age, Integer threshold) {
    return age == threshold;
}
```

## Good

```java
public boolean isAdult(Integer age, Integer threshold) {
    return age.equals(threshold);
}
```

## Why

`==` on boxed primitives compares object identity, not value. The JVM caches small `Integer` values (`-128..=127` by default), so the comparison happens to work inside the cached range and silently breaks outside it — a classic intermittent bug that only shows up once production data exceeds the cache boundary.

Use `Objects.equals(a, b)` if either operand might be `null`. Better still, prefer the primitive `int` whenever the value can't legitimately be absent — `Optional<Integer>` is a clearer way to express "may be missing" than a boxed type whose nullability is invisible at the call site.
