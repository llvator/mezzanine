---
id: prefer-pattern-instanceof
language: java
applies-to: [instanceof_expression]
match:
  is_pattern: "false"
severity: info
kind: style
sources:
  - "JEP 394 — Pattern Matching for instanceof (Java 16)"
  - "Effective Java, Item 33 (related) — Consider typesafe heterogeneous containers"
  - "Java_L Module 11 — Polymorphism (InstanceofPattern)"
---

# Use the pattern form of `instanceof`

## Bad

```java
public String describe(Object n) {
    if (n instanceof EmailNotification) {
        EmailNotification email = (EmailNotification) n;
        return "email to " + email.recipient + " (" + email.subject + ")";
    }
    return "unknown";
}
```

## Good

```java
public String describe(Object n) {
    if (n instanceof EmailNotification email) {
        return "email to " + email.recipient + " (" + email.subject + ")";
    }
    return "unknown";
}
```

## Why

The classic `instanceof` + cast pair names the type twice and inserts a third name for the local variable. A typo or a refactor that changes one of the three but not the others compiles cleanly and throws `ClassCastException` at runtime — the cast says "trust me, I just checked" but nothing in the syntax enforces that the test and the cast match.

The pattern form (Java 16+) makes the test and the binding one declaration. The compiler guarantees the bound name has the tested type, and there is no place for the three names to drift out of sync. The local is in scope only where the check is provably true — including in the *else* branch of a negated test (`if (!(n instanceof X x)) return; … x …`) — which makes early-return patterns concise.

Java 16 made this final in 2021, so it's available in every supported runtime. The pattern form is strictly more concise and strictly safer than the classic; the only reason to keep the classic form is supporting a pre-16 baseline, which is rare in greenfield work.
