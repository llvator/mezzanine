---
id: class-inheritance-fundamentals
language: java
applies-to: [class_declaration]
title: "Inheritance — `extends`, `abstract`, `final`, `sealed`"
level: beginner
sources:
  - "Java_L Module 09 — Inheritance (ExtendsExample, AbstractClasses, FinalKeyword)"
  - "JLS §8.1 — Class Declarations"
  - "Effective Java, Item 19 — Design and document for inheritance or else prohibit it"
  - "JEP 409 — Sealed Classes (Java 17)"
---

# Inheritance — `extends`, `abstract`, `final`, `sealed`

Inheritance is Java's "is-a" relationship: a subclass reuses (and may override) the fields and methods of its superclass. Four keywords shape the conversation — `extends` declares the relationship; `abstract` says "this class is incomplete"; `final` says "no subclass allowed"; `sealed` says "only this specific set of subclasses."

## Syntax

```java
// Plain inheritance — Manager IS-A Employee.
class Manager extends Employee {
    Manager(String name, String dept, double salary) {
        super(name, dept, salary);   // must be the first statement
        // … Manager-specific init …
    }

    @Override
    String getRole() { return "Manager"; }
}

// Abstract class — cannot be instantiated; declares what subclasses must do.
abstract class PaymentMethod {
    protected double balance;
    abstract boolean processPayment(double amount);   // no body
    double getBalance() { return balance; }           // shared concrete
}

// Final class — closes the hierarchy at this point.
public final class Money { /* … */ }

// Sealed class — explicit, exhaustive list of permitted subtypes (Java 17+).
public sealed class Shape permits Circle, Square, Triangle { /* … */ }
```

## Key ideas

**A subclass extends exactly one superclass.** Java does not have multiple inheritance of state — that's what interfaces are for (multiple inheritance of *type*, with `default` methods for shared behaviour). Every class has exactly one parent; the implicit parent is `Object` when you write no `extends` clause.

**The subclass constructor must call a superclass constructor.** Either explicitly with `super(args)` as the first statement, or implicitly via `super()` when the superclass has a no-arg constructor. Forgetting both when the superclass needs arguments is a compile error — and is the constraint that forces subclasses to think about the parent's invariants.

**`abstract` is for incomplete classes.** An abstract class can hold state and concrete methods (unlike an interface, which is now blurrier with `default` methods but is still stateless), and it can list abstract methods that subclasses must implement. Marking a class `abstract` is mainly a signal: "do not instantiate this directly; subclass it." If you write `abstract class Foo` but the body has no `abstract` members and no `protected` constructor, ask whether the class wants to be a plain (or `final`) class instead.

**`final` is the safe default for non-leaf classes.** *Effective Java*'s Item 19 puts the principle plainly: a class is either designed for inheritance (with documented overridable methods and a thought-through subclass contract) or it should be `final`. Allowing inheritance silently turns every public method into part of the API — every override that breaks the class's invariants is now your problem to debug.

**`sealed` gives you exhaustive polymorphism (Java 17+).** Marking a class `sealed` with a `permits` list means the compiler knows the full set of subclasses. A `switch` over a sealed type can be exhaustive without a `default` branch, which is exactly what you want for "handle every shape" code. Use `final`, `sealed`, or `non-sealed` on each permitted subclass to choose whether to close it further.

## Related

- Rule: [`abstract-class-without-abstract-methods`](../../rules/oop/abstract-class-without-abstract-methods.md) — flags `abstract` classes whose body has no `abstract` members.
- Lesson: [`class-fundamentals`](class-fundamentals.md) — the building blocks before inheritance.
- Lesson: [`interface-declaration-fundamentals`](interface-declaration.md) — the multiple-inheritance-of-type alternative.
- Lesson: [`constructor-declaration-fundamentals`](constructor-declaration.md) — `super(…)` and the call-order rules.
