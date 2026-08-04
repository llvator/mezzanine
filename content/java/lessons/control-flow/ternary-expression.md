---
id: ternary-expression-fundamentals
language: java
applies-to: [ternary_expression]
title: "Ternary `?:` — the conditional that has a value"
level: beginner
sources:
  - "Java_L Module 04 — Operators (LogicalOperators)"
  - "JLS §15.25 — Conditional Operator ? :"
  - "Effective Java, Item 67 (related) — Optimize judiciously"
---

# Ternary `?:` — the conditional that has a value

`cond ? a : b` is Java's only ternary operator and the only conditional that's an *expression* (it has a value) rather than a statement. When the branches each produce a value the surrounding code needs, the ternary is shorter and clearer than an `if`/`else` that assigns the same variable on both sides.

## Syntax

```java
// Pick a value based on a condition.
int absolute = n >= 0 ? n : -n;

// Inline default — common with null checks.
String label = title != null ? title : "untitled";

// As a method argument.
log.info("processed {} order{}", count, count == 1 ? "" : "s");

// Use Math / Objects helpers when they fit — they're often clearer.
int absolute2 = Math.abs(n);
String label2 = Objects.requireNonNullElse(title, "untitled");
```

## Key ideas

**The two branches must have a common type.** The compiler infers the ternary's result type as the most specific supertype of `a` and `b`. `cond ? 1 : 2.0` is `double` (Java widens `1`); `cond ? "x" : null` is `String`; `cond ? new ArrayList<>() : new LinkedList<>()` is `List`. If no common type exists, the line doesn't compile — that's a feature, since "one branch is a String and the other is an int" is rarely intentional.

**Auto-unboxing makes `cond ? boxed : 0` dangerous.** When one branch is a boxed type (`Integer`) and the other is a primitive (`int`), the compiler unboxes the boxed branch. If the boxed value is `null`, that's a NullPointerException at runtime — same shape as the boxed-equality footgun. Prefer keeping both branches the same kind of type.

**The ternary evaluates exactly one branch.** Short-circuit applies: `cond ? expensive() : cheap()` only calls `expensive()` when `cond` is true. Both branches must compile but only the selected one runs, so it's safe to put `array[i]` after a bounds check in the same ternary.

**Nest ternaries sparingly — and only when each level reads as a "case."** A two-level chain like `season == WINTER ? "cold" : season == SUMMER ? "hot" : "mild"` mirrors how you'd describe it in English. Beyond two levels, or when the branches have different shapes (one is a method call, another a literal), readers lose track of which `?` pairs with which `:`. Reach for a `switch` expression (Java 14+) or extract a method named for the decision.

**The ternary is an expression, `if` is a statement.** Use the ternary when the surrounding code needs the *value* — assignment, return, argument. Use `if`/`else` when the branches do *work* (call methods, mutate state). Trying to cram a ternary's `?` into a statement-y use almost always ends up reading worse than the `if`/`else` form.

## Related

- Rule: [`nested-ternary-discouraged`](../../rules/control-flow/nested-ternary-discouraged.md) — flags ternaries whose branches are themselves ternaries.
- Lesson: [`if-statement-fundamentals`](if-statement.md) — the statement form, for branches that *do work* rather than produce values.
- Lesson: [`switch-statement-fundamentals`](switch-statement.md) — what to reach for once you have three or more value-producing branches.
