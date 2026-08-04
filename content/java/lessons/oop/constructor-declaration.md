---
id: constructor-declaration-fundamentals
language: java
applies-to: [constructor_declaration]
title: "Constructors — initializing a fresh instance"
level: beginner
sources:
  - "Java_L Module 08 — OOP Basics (Constructors)"
  - "JLS §8.8 — Constructor Declarations"
  - "Effective Java, Item 1 — Consider static factory methods instead of constructors"
  - "Effective Java, Item 2 — Consider a builder when faced with many constructor parameters"
---

# Constructors — initializing a fresh instance

A constructor sets up a freshly allocated object before any other code can see it. It looks like a method but isn't: it has no return type, must share its class name, and runs exactly once per instance — at construction.

## Syntax

```java
public class Account {
    private final String holder;
    private final long balanceCents;

    // Primary constructor — accepts everything the invariant needs.
    public Account(String holder, long balanceCents) {
        if (holder == null || holder.isBlank()) {
            throw new IllegalArgumentException("holder is required");
        }
        if (balanceCents < 0) {
            throw new IllegalArgumentException("balance cannot be negative");
        }
        this.holder = holder;
        this.balanceCents = balanceCents;
    }

    // Convenience constructor — delegates to the primary via `this(…)`.
    public Account(String holder) {
        this(holder, 0L);
    }
}

class CheckingAccount extends Account {
    public CheckingAccount(String holder, long opening) {
        super(holder, opening);     // superclass constructor first
    }
}
```

## Key ideas

**If you write *any* constructor, Java stops providing the implicit no-arg one.** A class with only a `MyClass(int x)` constructor cannot be built with `new MyClass()` — frameworks that require a no-arg constructor (Hibernate entities, some JSON libraries) need you to write one explicitly. Adding the first parameterised constructor is exactly the moment to decide whether you still want the no-arg form.

**The body's first line can delegate, via `this(…)` or `super(…)`.** `this(…)` calls another constructor on the same class so you don't repeat field-assignment boilerplate. `super(…)` calls a superclass constructor — required when the superclass has no no-arg constructor, and inserted implicitly as `super()` when you omit it.

**Use the constructor to enforce invariants, not just to copy parameters.** Anything the class promises to be true for the rest of its life ("balance is non-negative", "name is non-blank", "list is non-null") must be checked here, because no later method can recover an already-broken object. Failing fast with `IllegalArgumentException` is the standard pattern.

**Many parameters is a smell; many overloads is worse.** When a constructor takes more than a handful of arguments — especially if several are the same type and easy to swap — readers can't tell `new Account(a, b, c, d, e)` apart at the call site. The standard fix is a builder (Item 2 in *Effective Java*) or a static factory method named for the construction mode (`Account.opened(…)`, `Account.empty(holder)`).

**Don't call overridable methods from a constructor.** When a subclass's constructor runs `super(…)`, the subclass's fields aren't initialised yet, so any overridden method called from the superclass constructor will see uninitialised state. Restrict constructor bodies to `private`/`final`/`static` method calls.

## Related

- Rule: [`constructor-too-many-parameters`](../../rules/oop/constructor-too-many-parameters.md) — flags constructors with too many positional args as a builder-pattern smell.
- Lesson: [`class-fundamentals`](class-fundamentals.md) — what the constructor is *for*.
- Lesson: [`method-declaration-fundamentals`](method-declaration.md) — constructors are not methods, but they share the modifier and parameter shape.
