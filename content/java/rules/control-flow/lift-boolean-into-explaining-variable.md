---
id: lift-boolean-into-explaining-variable
language: java
applies-to: [if_statement]
match:
  condition_complexity: long
severity: info
kind: structure
sources:
  - "Refactoring (Fowler, 2nd ed.) — Introduce Explaining Variable / Extract Variable"
  - "Clean Code (Martin) — Chapter 17 G19, Use Explanatory Variables"
---

# Long boolean conditions deserve a named explaining variable

## Bad

```java
public class OrderFilter {
    public void mark(Order order, Config config, Status status, boolean forced) {
        if (!forced
                && config.isFilterOldSequence()
                && status.getLastSequence() != null
                && status.getLastSequence().compareTo(order.getSequence()) >= 0) {
            markFiltered(order, status);
        }
    }
}
```

## Good

```java
public class OrderFilter {
    public void mark(Order order, Config config, Status status, boolean forced) {
        final boolean outdatedSequence = config.isFilterOldSequence()
                && status.getLastSequence() != null
                && status.getLastSequence().compareTo(order.getSequence()) >= 0;

        if (!forced && outdatedSequence) {
            markFiltered(order, status);
        }
    }
}
```

## Why

A reader hitting a four-clause `&&` chain has to hold every operand in mind at once and re-derive what the whole expression *means* before they can move on. The name on a local variable encodes that meaning once: future readers — including the author six months later — read `outdatedSequence` and know what the predicate is testing without re-parsing it.

The lifted form is also where the next edit lands cleanly. Adding a fifth condition to the inline chain pushes the line past the screen edge; adding it to the named local keeps the `if` readable and lets the diff reviewer see exactly which clause changed. The name should describe **what is true** (`outdatedSequence`, `eligibleForRetry`), not **what is computed** (`sequenceLessThanLast`) — the goal is to replace mechanism with intent.

JIT inlining makes the extra local free at runtime, so the cost is one line of source for a large readability win. Skip the lift only when the name would be longer than the expression itself, or when the condition is already one operation (`if (list.isEmpty())` doesn't deserve a temporary).
