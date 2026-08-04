---
id: record-fundamentals
language: java
applies-to: [record_declaration]
title: "Records — immutable data carriers with one declaration"
level: intermediate
sources:
  - "Java_L Module 19 — Records & Sealed Classes"
  - "JEP 395 — Records (Java 16)"
  - "JLS §8.10 — Record Classes"
  - "Effective Java, Item 17 — Minimize mutability"
---

# Records — immutable data carriers with one declaration

A `record` is a class whose entire state is declared in its header. From `record Point(int x, int y) {}` the compiler generates a private final field per component, a canonical constructor that takes all components in order, an accessor per component (no `get` prefix), and structural `equals` / `hashCode` / `toString`. It's the answer to "I just want a tuple-shaped class" without the 30 lines of boilerplate.

## Syntax

```java
// Minimal record — three components, no body.
public record Point(int x, int y, int z) {}

// Use it like any class.
Point p = new Point(1, 2, 3);
int x = p.x();                  // accessor — no get-prefix
boolean same = p.equals(new Point(1, 2, 3));   // true — structural equality

// Compact constructor — validate or normalise without re-listing components.
public record Range(int low, int high) {
    public Range {
        if (low > high) {
            throw new IllegalArgumentException("low must be ≤ high");
        }
    }
}

// Records can implement interfaces and add methods.
public record Money(long cents, Currency currency) implements Comparable<Money> {
    public Money plus(Money other) {
        if (!currency.equals(other.currency)) {
            throw new IllegalArgumentException("currency mismatch");
        }
        return new Money(cents + other.cents, currency);
    }

    @Override
    public int compareTo(Money other) { return Long.compare(cents, other.cents); }
}
```

## Key ideas

**Components are the state — there is no other.** Every field a record carries is in the header. The body can declare *static* fields (constants) and methods, but instance fields are not allowed — try and the compiler rejects it. That restriction is the single property that makes a record predictable: two `Point(1, 2, 3)` values are guaranteed to be `.equals` regardless of how they were constructed.

**Records are implicitly final and extend `java.lang.Record`.** You cannot subclass a record, and a record cannot `extends` anything else (it can `implements` interfaces). That closes the inheritance dimension and lets the compiler reason about value-semantics safely.

**The compact constructor validates the incoming components.** Inside `public Range { … }` you can throw, normalise (`if (cents < 0) cents = 0;`), or compute derived state. The compiler assigns the parameters to the fields automatically *after* your code runs. This is the right place to enforce invariants — by the time the record exists, its state is guaranteed valid.

**Don't reach for a record if the type has identity, mutable state, or a non-component field.** A record models a *value* — two `Point(1, 2, 3)` records are the same point. A `Customer` with mutable balance, last-login timestamp, and order history is an entity, not a value; use a regular class. The "if you'd write `equals` and `hashCode` by hand on every component" rule is a good fit-test: if the answer is yes, a record collapses the boilerplate.

**Records pair beautifully with sealed types and pattern matching.** `sealed interface Shape permits Circle, Rectangle, Triangle` + `record Circle(double radius) implements Shape` + a pattern switch gives you closed sum types — the compiler enforces exhaustive case handling, the records give you value semantics, and the pattern switch destructures by component. Modern Java's answer to discriminated unions.

## Related

- Rule: [`record-with-instance-field`](../../rules/oop/record-with-instance-field.md) — flags records that declare instance fields outside the header.
- Lesson: [`class-fundamentals`](class-fundamentals.md) — when the type has identity / mutation / extra state, a regular class is the right shape.
- Lesson: [`class-inheritance-fundamentals`](class-inheritance.md) — `sealed` types are the natural complement to records.
