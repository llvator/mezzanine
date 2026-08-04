---
id: else-without-braces
language: java
applies-to: [if_statement]
match:
  else_kind: single_statement
severity: warning
kind: style
sources:
  - "Google Java Style Guide §4.1.1 — Use of optional braces"
  - "Java_L Module 05 — Control Flow (IfElse, common pitfalls)"
---

# Always brace `else` bodies — even one-liners

## Bad

```java
public String classify(int x) {
    if (x > 0) {
        return "positive";
    } else
        return "non-positive";   // brace-less else; a second line below is a footgun magnet
}

// Six months later:
public String classify(int x) {
    if (x > 0) {
        return "positive";
    } else
        log.debug("non-positive case");
        return "non-positive";   // ALWAYS runs — indentation lies
}
```

## Good

```java
public String classify(int x) {
    if (x > 0) {
        return "positive";
    } else {
        return "non-positive";
    }
}
```

## Why

This is the mirror of the [`if-without-braces`](if-without-braces.md) hazard: the same single-statement-vs-block grammar exists on the `else` side, and the same "add a log line and break the contract" failure mode applies. Braces around every branch make the structure invariant under future edits.

The narrow legitimate exception is the chained `else if` form — `else if (…) …` is conventional and reads cleanly. This rule does not fire on `else_kind: if` precisely because chained else-ifs are how the language is meant to express multi-way branching. It fires only on the `else <single-statement>;` shape, where the single statement is *not* itself another `if`.

A formatter like `google-java-format` adds these braces automatically; in projects without auto-format, this rule is the safety net.
