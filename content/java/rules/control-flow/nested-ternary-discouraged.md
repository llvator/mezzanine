---
id: nested-ternary-discouraged
language: java
applies-to: [ternary_expression]
match:
  is_nested: "true"
severity: info
kind: style
sources:
  - "Google Java Style Guide §4.5 — Line-wrapping (related: keep expressions scannable)"
  - "Sonar rule java:S3358 — Ternary operators should not be nested"
  - "Java_L Module 04 — Operators (LogicalOperators)"
---

# Nested ternaries are hard to read — extract or switch

## Bad

```java
public String shippingTier(double weight) {
    return weight < 1.0
        ? "light"
        : weight < 5.0
            ? "standard"
            : weight < 20.0
                ? "heavy"
                : "oversize";
}
```

## Good

```java
public String shippingTier(double weight) {
    if (weight < 1.0)  return "light";
    if (weight < 5.0)  return "standard";
    if (weight < 20.0) return "heavy";
    return "oversize";
}

// Or, when the input is enum / String, a switch expression reads even better:
public String shippingTier(WeightClass w) {
    return switch (w) {
        case LIGHT     -> "light";
        case STANDARD  -> "standard";
        case HEAVY     -> "heavy";
        case OVERSIZE  -> "oversize";
    };
}
```

## Why

A single ternary reads as "if A then x else y" — one branch decision, one value. Once the `else` branch is itself a ternary, the reader has to mentally re-pair `?`s with `:`s and track which condition still applies under which branch. Get the indentation slightly off and the visual structure stops matching the logical structure entirely; a fix that flips two `:`s compiles cleanly and changes behaviour.

The fix depends on what the chain is really doing:

- **Early-return chain** — a sequence of `if (cond) return value;` statements reads top-to-bottom, with each condition explicit. No reader has to count question marks.
- **Multi-way selector** — a `switch` expression (Java 14+) is exhaustive on enums and sealed types, gives every case a label, and produces a value just like the ternary did.
- **Computed value with two cases** — keep the ternary. The rule only fires once nesting kicks in.

This rule fires when *either* branch of a ternary is itself a ternary. Two levels is the threshold most style guides use because that's where the readability cliff starts; beyond two, it gets worse rather than just longer.
