---
id: switch-statement-fundamentals
language: java
applies-to: [switch_statement]
title: "`switch` — classic statement and modern expression"
level: beginner
sources:
  - "Java_L Module 05 — Control Flow (SwitchExpressions)"
  - "JLS §14.11 — The switch Statement"
  - "JEP 361 — Switch Expressions (Java 14)"
---

# `switch` — classic statement and modern expression

`switch` is Java's multi-way branch. Two syntactic forms coexist: the classic colon-and-`break` statement inherited from C, and the arrow-based expression introduced in Java 14. They share a keyword and a selector, but their failure modes are entirely different.

## Syntax

```java
// Classic statement form — colon labels, fall-through unless `break`.
switch (day) {
    case "MONDAY":
    case "TUESDAY":
    case "WEDNESDAY":
    case "THURSDAY":
    case "FRIDAY":
        System.out.println("weekday");
        break;
    case "SATURDAY":
    case "SUNDAY":
        System.out.println("weekend");
        break;
    default:
        System.out.println("unknown");
}

// Modern expression form — arrow labels, no fall-through, returns a value.
String kind = switch (day) {
    case "MONDAY", "TUESDAY", "WEDNESDAY", "THURSDAY", "FRIDAY" -> "weekday";
    case "SATURDAY", "SUNDAY" -> "weekend";
    default -> "unknown";
};
```

## Key ideas

**Fall-through is the classic form's defining footgun.** With colon labels, control flows from one `case` into the next unless an explicit `break` stops it — that's what lets multiple labels share a body, but it also means a forgotten `break` silently runs the next case too. The arrow form (`case … -> …`) removes fall-through entirely: exactly the matched branch runs, then control leaves the switch.

**The expression form must be exhaustive.** Used in expression position (`var y = switch (…)`), the compiler requires every possible selector value to be handled — usually via a `default` branch, or for `enum` / sealed selectors by enumerating all constants. The statement form has no such requirement, which is part of why it's easier to leave bugs in.

**The selector accepts more shapes than C.** `switch` works on integers, `String`, `enum` constants, and (Java 21+) any reference type via pattern matching. A single `case` can list multiple labels (`case "MON", "TUE" ->`) instead of using stacked-label fall-through.

**Use `yield` for block-bodied expression cases.** When an arrow case needs multiple statements, it takes a brace block and uses `yield value;` instead of `return value;`. `return` would leave the enclosing method; `yield` leaves only the switch.

## Related

- Rule: [`switch-classic-colon-style`](../../rules/control-flow/switch-classic-colon-style.md) — flags the fall-through-by-default form so you can migrate to arrows.
- Lesson: [`if-statement-fundamentals`](if-statement.md) — when you have two branches, an `if` reads better than a two-arm switch.
