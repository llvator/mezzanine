---
id: while-true-needs-clear-exit
language: java
applies-to: [while_statement]
match:
  condition_is_literal_true: "true"
severity: info
kind: gotcha
sources:
  - "Java_L Module 05 — Control Flow (WhileLoops)"
  - "Effective Java, Item 57 — Minimize the scope of local variables (related: keep loop exit visible)"
---

# `while (true)` — make the exit obvious

## Bad

```java
public Order nextOrder() {
    while (true) {
        Order order = queue.poll();
        if (order != null) {
            return order;
        }
        if (shouldStop()) {
            return null;
        }
        sleep(BACKOFF_MS);
    }
}
```

## Good

```java
public Order nextOrder() {
    while (!shouldStop()) {
        Order order = queue.poll();
        if (order != null) {
            return order;
        }
        sleep(BACKOFF_MS);
    }
    return null;
}
```

## Why

`while (true)` is a deliberate infinite loop — perfectly legal and sometimes the cleanest shape. The cost is that the exit conditions are hidden inside the body. A reader has to scan every `break`, `return`, and `throw` to know when the loop ends. If two exit paths get added by different commits, you end up with logic the original author never imagined.

When the loop's exit really is a single boolean condition, lifting it into the header makes the code self-documenting: `while (!shouldStop())` reads as "loop until we're told to stop." That's a comprehension win at zero runtime cost, and it gives static analysis tools (including this one) the information needed to reason about termination.

`while (true)` is still the right shape when there are genuinely several independent exit conditions (parser loops with both EOF and error exits, event dispatchers with stop-and-error signals). In those cases the loop body is the documentation — make sure each `break`/`return` has an obvious cause.
