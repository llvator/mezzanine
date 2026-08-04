---
id: field-declaration-fundamentals
language: java
applies-to: [field_declaration]
title: Fields — the state attached to a class or instance
level: beginner
sources:
  - "Java_L Module 08 — OOP Basics (ClassBasics, Encapsulation)"
  - "JLS §8.3 — Field Declarations"
---

# Fields — the state attached to a class or instance

A *field* is a variable declared directly inside a class body. It stores data that lives as long as the class (when `static`) or as long as an instance (when not). Every modifier on the line — `private`, `static`, `final` — narrows what the rest of the program can do with that data.

## Syntax

```java
public class Account {
    // Instance, immutable after construction.
    private final String id;

    // Instance, mutable.
    private long balance;

    // Class-level constant — one copy, never reassigned.
    public static final int MAX_OVERDRAFT = 100;

    public Account(String id, long initial) {
        this.id = id;
        this.balance = initial;
    }
}
```

## Key ideas

**Visibility decides who can see the field.** `private` (the default for fields in modern code) limits access to the declaring class; `protected` lets subclasses and the same package in; `public` exposes it to every caller. Leaving the visibility modifier off gives package-private access — visible to other classes in the same `package`, invisible everywhere else.

**`static` means class-scoped.** A non-static field exists once per instance — each `new Account(…)` gets its own `balance`. A `static` field exists once per class — `Account.MAX_OVERDRAFT` is shared by every instance and survives even if no instances exist. Static fields are how Java models class-level constants and shared caches.

**`final` means "assigned exactly once".** A `final` field must be initialised either at the declaration or in every constructor; after that, the reference cannot be reassigned. For primitives this means the value is fixed; for objects, the *reference* is fixed, but the object it points at may still be mutated unless that object is itself immutable.

**Annotations like `@Resource`, `@Autowired`, `@Inject` are framework hooks.** They don't change Java semantics — the field is still a normal field — but a framework (Spring, JEE, Guice) scans for them at startup and writes a value in before user code runs. That's why injected fields typically have no initialiser: the framework sets them.

## Related

- Rule: [`public-mutable-static-field`](../../rules/oop/public-mutable-static-field.md) — when `public static` non-final fields leak global state.
- Lesson: [`class-fundamentals`](class-fundamentals.md) — fields are one half of a class (state); methods are the other (behaviour).
