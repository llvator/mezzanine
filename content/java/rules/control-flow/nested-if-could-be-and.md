---
id: nested-if-could-be-and
language: java
applies-to: [if_statement]
match:
  then_kind: solitary_if_no_else
  has_else: "false"
severity: info
kind: structure
sources:
  - "Refactoring (Fowler) — Decompose Conditional / Consolidate Conditional Expression"
  - "Java_L Module 05 — Control Flow (IfElse, common pitfalls)"
---

# Two nested `if`s with one body — merge them with `&&`

## Bad

```java
public boolean canRetry(Order order) {
    if (order.isActive()) {
        if (order.hasRemainingAttempts()) {
            return true;
        }
    }
    return false;
}
```

## Good

```java
public boolean canRetry(Order order) {
    if (order.isActive() && order.hasRemainingAttempts()) {
        return true;
    }
    return false;
}
```

## Why

When an `if` contains only another `if` (with no `else` on either layer), the two conditions are logically conjoined — both must be true for the body to run. Writing them as `if (a && b) { … }` says exactly that, in one line, with no ambiguity about which condition guards which body.

The nested form is harder to scan (you have to read two condition expressions and match them up mentally with the indented body) and grows worse with three or four levels of nesting. Merging keeps the body at one indent level — the maintainability cliff arrives later.

Java's `&&` is short-circuiting: if `a` is `false`, `b` is never evaluated. So this rewrite preserves the behaviour of the nested form even when `b` has side effects or would throw on the false-of-`a` path — the merged form makes the same call sequence.

The rule only fires when both layers have *no* `else` clause. If either layer has an else, the merge changes semantics — the rule stays silent on those cases.
