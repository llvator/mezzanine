---
id: switch-classic-colon-style
language: java
applies-to: [switch_statement]
match:
  case_style: colon
severity: info
kind: style
sources:
  - "JEP 361 — Switch Expressions (Java 14)"
  - "Effective Java, Item 67 (paraphrased) — Keep control flow obvious"
  - "Java_L Module 05 — Control Flow (SwitchExpressions)"
---

# Prefer the arrow form of `switch`

## Bad

```java
public String dayType(String day) {
    switch (day) {
        case "MONDAY":
        case "TUESDAY":
        case "WEDNESDAY":
        case "THURSDAY":
        case "FRIDAY":
            return "weekday";
        case "SATURDAY":
        case "SUNDAY":
            return "weekend";
        default:
            return "unknown";
    }
}
```

## Good

```java
public String dayType(String day) {
    return switch (day) {
        case "MONDAY", "TUESDAY", "WEDNESDAY", "THURSDAY", "FRIDAY" -> "weekday";
        case "SATURDAY", "SUNDAY" -> "weekend";
        default -> "unknown";
    };
}
```

## Why

The colon form falls through to the next case by default — forgetting a `break` turns "I handled Monday" into "I also ran Tuesday's branch and Wednesday's branch and …". The bug is silent, the indentation looks fine, and every Java developer has fixed it in someone else's code at least once.

The arrow form removes fall-through entirely: each case runs exactly its right-hand side. Multiple labels per case (`case "MON", "TUE" ->`) gives you the same "group these together" expressiveness without the trap. As an expression (`var y = switch (…)`), it also forces exhaustiveness — the compiler tells you if you forgot a branch — which the classic statement form never does.

Java 14 stabilised the arrow syntax in 2020, so it has been available in every supported Java version for years. New code should use it; existing classic switches that have grown new cases are the highest-value targets to migrate, because that's where fall-through bugs accumulate.
