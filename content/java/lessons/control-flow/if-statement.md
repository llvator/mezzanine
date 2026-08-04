---
id: if-statement-fundamentals
language: java
applies-to: [if_statement]
title: "`if` / `else` — conditional execution and the dangling-else footgun"
level: beginner
sources:
  - "Java_L Module 05 — Control Flow (IfElse)"
  - "JLS §14.9 — The if Statement"
  - "Effective Java, Item 67 — Optimize judiciously (related: keep control flow obvious)"
---

# `if` / `else` — conditional execution and the dangling-else footgun

`if` is the most basic branching construct: a boolean condition decides whether a statement runs. The grammar is simple; the trap is that "a statement" includes both braced blocks *and* single statements with no braces, and the latter is the source of one of Java's classic bug classes.

## Syntax

```java
// Plain if
if (quantity <= 0) {
    throw new IllegalArgumentException("quantity must be positive");
}

// if / else
if (inventory.isAvailable(productId, quantity)) {
    payment.charge(customerId, total);
} else {
    notifier.outOfStock(customerId, productId);
}

// if / else if / else chain
if (status.isPending()) {
    return "PENDING";
} else if (status.isShipped()) {
    return "SHIPPED";
} else {
    return "UNKNOWN";
}
```

## Key ideas

**The condition must be `boolean`, not just "truthy".** Java is strict — `if (1)` doesn't compile, and `if (someObject)` doesn't compile. You always write the comparison explicitly: `if (count > 0)`, `if (list != null)`, `if (map.containsKey(k))`. This is intentional — it forces the boolean intent to be visible at every branch.

**`else` always binds to the *nearest* unmatched `if`.** Combined with brace-less single-statement bodies, this is the *dangling else* problem:

```java
if (a)
    if (b) doX();
else doY();          // looks like it belongs to `if (a)`; actually belongs to `if (b)`
```

Most style guides — Google, Sun, Effective Java — therefore mandate braces around every `if`/`else`/`for`/`while` body, even one-liners. The cost is one keystroke; the bug it prevents is a real production-incident class.

**`else if` is not a Java keyword.** It's a stylistic convention for the case where the else-branch is *another* `if` statement. The grammar sees `else { if (…) … }` and writers leave the braces off for readability. That convention works *because* you know what the structure means — but it does mean every link in the chain is its own `if_statement` node, and any of them can independently fall victim to the dangling-else bug.

**When the branch returns a value, `?:` is often clearer.** `int sign = x > 0 ? 1 : x < 0 ? -1 : 0;` reads better than a four-line if-else chain. The ternary expression `cond ? a : b` is an *expression* (it has a value) while `if` is a *statement* (it doesn't); use the ternary when the branches each produce a value the surrounding code needs.

## Related

- Rule: [`if-without-braces`](../../rules/control-flow/if-without-braces.md) — flags `if (cond) foo();` so the dangling-else trap is impossible to hit.
- Lesson: [`for-statement-fundamentals`](for-statement.md) — same brace discipline applies to `for` / `while` / `do-while` bodies.
