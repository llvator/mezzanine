---
id: interface-declaration-fundamentals
language: java
applies-to: [interface_declaration]
title: "Interfaces — contracts independent of inheritance"
level: beginner
sources:
  - "Java_L Module 10 — Interfaces (InterfaceBasics)"
  - "JLS §9 — Interfaces"
  - "Effective Java, Item 20 — Prefer interfaces to abstract classes"
  - "Effective Java, Item 21 — Design interfaces for posterity"
---

# Interfaces — contracts independent of inheritance

An `interface` declares a set of capabilities a class commits to providing — the *what*, not the *how*. A class can implement any number of interfaces but extend only one class, which is why interfaces are the primary tool for sharing behaviour across unrelated hierarchies.

## Syntax

```java
public interface Printable {
    // Abstract method — implementations must provide it.
    // `public abstract` is implicit on interface methods; you don't write them.
    void print();
}

public interface Exportable {
    String exportTo(String format);

    // Default method — body lives on the interface so existing implementors
    // pick it up without changes. Added in Java 8 for API evolution.
    default String exportToCsv() {
        return exportTo("csv");
    }

    // Static method — utility tied to the interface's namespace, not to any
    // particular instance. Added in Java 8.
    static Exportable empty() {
        return format -> "";
    }
}

// Implementing multiple interfaces is fine and idiomatic.
public class Invoice implements Printable, Exportable {
    public void print() { /* … */ }
    public String exportTo(String format) { /* … */ }
}
```

## Key ideas

**Members have surprising default modifiers.** Every method body-less declaration is implicitly `public abstract`; every field is implicitly `public static final`. Writing those keywords is legal but redundant — IDEs flag them as "modifier is redundant on interface members". The fact that fields are *constants* is a frequent gotcha: you can't add per-instance state to an interface, which is part of the design.

**Default methods are for API evolution, not for sharing implementation.** Java 8 added `default` so that adding a method to an interface wouldn't break existing implementors. That's a narrow purpose. Using `default` to share substantive logic across implementors signals the type is really an abstract class in disguise — and abstract classes are still the right tool when you need stored state or a private helper.

**Multiple inheritance of *type*, not state.** A class can implement many interfaces, so a type can be `Printable`, `Exportable`, `Comparable` all at once — no diamond problem because no fields are inherited. If two interfaces declare conflicting `default` methods, the compiler forces the implementing class to disambiguate explicitly.

**Functional interfaces are a special shape.** An interface with exactly one abstract method (a "SAM type") can be the target of a lambda — that's what makes `Predicate`, `Function`, `Comparator` and friends work. Marking the interface `@FunctionalInterface` is optional but tells future maintainers not to add a second abstract method (which would break every lambda implementation).

**Sealed interfaces restrict the implementor set.** Java 17+ `sealed interface Shape permits Circle, Rectangle, Triangle` makes the type closed — `switch` over a sealed type can be exhaustive without a `default`, and IDEs know the full implementor list for analysis.

## Related

- Rule: [`interface-default-method-heavy`](../../rules/oop/interface-default-method-heavy.md) — flags interfaces that have grown several `default` methods, hinting the type wants to be an abstract class.
- Lesson: [`class-fundamentals`](class-fundamentals.md) — abstract classes share many traits with interfaces but allow state.
- Lesson: [`lambda-expression-fundamentals`](../expressions/lambda-expression.md) — single-abstract-method interfaces are the targets of lambdas.
