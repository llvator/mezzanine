---
id: enum-fundamentals
language: java
applies-to: [enum_declaration]
title: "Enums — type-safe named constants"
level: beginner
sources:
  - "Java_L Module 15 — Enums"
  - "JLS §8.9 — Enum Classes"
  - "Effective Java, Item 34 — Use enums instead of int constants"
  - "Effective Java, Item 38 — Emulate extensible enums with interfaces"
---

# Enums — type-safe named constants

An `enum` declares a small, fixed set of named constants that the JVM treats as singletons. Two references to `Color.RED` are guaranteed to point at the same object — that identity, plus compile-time exhaustiveness in `switch`, plus the ability to attach behaviour, is what makes enums the right answer to most "I have a closed set of options" problems.

## Syntax

```java
// Simplest form — just a list of names.
public enum Direction { NORTH, SOUTH, EAST, WEST }

// With fields, a constructor, and per-constant behaviour.
public enum HttpStatus {
    OK(200, "OK"),
    NOT_FOUND(404, "Not Found"),
    SERVER_ERROR(500, "Internal Server Error");

    private final int code;
    private final String reason;

    HttpStatus(int code, String reason) {
        this.code = code;
        this.reason = reason;
    }

    public int code()       { return code; }
    public String reason()  { return reason; }
    public boolean isError() { return code >= 400; }
}

// Per-constant method bodies (enums can be partially abstract).
public enum Operation {
    PLUS  { public int apply(int a, int b) { return a + b; } },
    MINUS { public int apply(int a, int b) { return a - b; } };
    public abstract int apply(int a, int b);
}
```

## Key ideas

**Enum constants are JVM-managed singletons.** The compiler emits each constant as a `public static final` field of the enum class, initialised once when the class is loaded. `Direction.NORTH == Direction.NORTH` is always true; serialisation, reflection, and clone all preserve identity. That's why `==` is the right comparison for enum constants — `.equals(…)` works but isn't more correct.

**Use enums anywhere a `String` or `int` carries a finite set of meanings.** "Order status: NEW, PAID, SHIPPED" is an enum, not a `String` you compare with `equals` (which loses compile-time checking and lets typos slip through). "Days of the week", "log levels", "HTTP methods" — all enums. *Effective Java*'s Item 34 puts it bluntly: the `int`-constant pattern that predates enums is now an anti-pattern.

**Instance state on an enum should be `final`.** Each constant is a singleton, so a mutable field is shared across every reference to that constant — `Color.RED.setBrightness(5)` mutates the only `RED` there ever is, surprising every other consumer. Use `final` fields populated in the constructor for per-constant data (the `code`/`reason` pattern above); reach for a parallel `Map<EnumType, ...>` (or `EnumMap`) when you need real mutable per-constant state.

**`switch` over an enum is exhaustive in expression position.** A switch *expression* (Java 14+) on an enum without a `default` will fail to compile if you miss a constant — that's the compiler enforcing that you considered each case. A switch *statement* lets the silent default through, which is why expression-form switch is the safer default for enum dispatch.

**Enums can implement interfaces but cannot extend other classes.** They already extend `java.lang.Enum`, which gives them `name()`, `ordinal()`, `valueOf(String)`, and `values()` for free. *Effective Java*'s Item 38 covers the "extensible enum" pattern when you genuinely need to add cases later — usually via an interface implemented by multiple enum types.

## Related

- Rule: [`enum-prefer-immutable-fields`](../../rules/oop/enum-prefer-immutable-fields.md) — flags enums that hold non-`final` instance fields.
- Lesson: [`class-fundamentals`](class-fundamentals.md) — enums *are* a kind of class; everything class-y applies.
- Lesson: [`switch-statement-fundamentals`](../control-flow/switch-statement.md) — exhaustive switching over enum constants.
