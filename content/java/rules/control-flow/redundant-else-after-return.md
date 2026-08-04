---
id: redundant-else-after-return
language: java
applies-to: [if_statement]
match:
  then_kind:
    in: [single_return, single_throw]
  has_else: "true"
severity: info
kind: style
sources:
  - "Refactoring (Fowler) — Replace Nested Conditional with Guard Clauses"
  - "Effective Java, Item 67 (paraphrased) — Keep control flow obvious"
---

# Drop the `else` when the `if` body already returns

## Bad

```java
public int abs(int x) {
    if (x < 0) {
        return -x;
    } else {
        return x;
    }
}

public Order place(Order order) {
    if (!order.isValid()) {
        throw new IllegalArgumentException("invalid order");
    } else {
        return repository.save(order);
    }
}
```

## Good

```java
public int abs(int x) {
    if (x < 0) {
        return -x;
    }
    return x;
}

public Order place(Order order) {
    if (!order.isValid()) {
        throw new IllegalArgumentException("invalid order");
    }
    return repository.save(order);
}
```

## Why

When the then-branch ends with `return` (or `throw`), control never falls through to the code after the `if`. That means everything *after* the `if` already runs only when the condition is false — it *is* the implicit else. Writing `else { … }` makes that explicit, which sounds clearer but adds two costs: one more indent level for the body, and an artificial pairing the reader must trace.

The early-return (guard-clause) shape flattens the function: each precondition handled at the top, the main logic at one indent level. It also reads top-to-bottom — the first thing the reader learns is "what we don't accept and why", then "what we do with valid input." Multi-step business logic written this way is dramatically easier to scan than the same logic nested inside an `if/else` tree.

The rule fires on `single_return` and `single_throw` then-branches (with or without an enclosing block — `if (x) return foo;` and `if (x) { return foo; }` are equivalent). It does *not* fire when the then-branch merely happens to end with a return after other statements — that case is more nuanced and the rewrite is not always cleaner.
