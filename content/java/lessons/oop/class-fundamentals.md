---
id: class-fundamentals
language: java
applies-to: [class_declaration]
title: Classes — the unit of object-oriented design
level: beginner
sources:
  - "Java_L Module 08 — OOP Basics (ClassBasics, Encapsulation, Constructors)"
  - "JLS §8 — Classes"
---

# Classes — the unit of object-oriented design

A *class* is a blueprint. It bundles **state** (fields) with **behaviour** (methods) and describes the shape of all objects of that type. Every Java program is a collection of classes; you can't write Java code outside one.

## Syntax

```java
public class Account {
    // Fields — the object's state
    private final String id;
    private long balance;

    // Constructor — how an instance is created
    public Account(String id, long initialBalance) {
        this.id = id;
        this.balance = initialBalance;
    }

    // Methods — what the instance can do
    public long getBalance() {
        return balance;
    }

    public void deposit(long amount) {
        balance += amount;
    }
}
```

## Key ideas

**Fields are state.** Each instance of `Account` has its own `id` and `balance`. `private` keeps them inaccessible from outside the class — the only way to read or change them is through methods the class chooses to expose. This is *encapsulation*: the class controls its own invariants.

**Constructors initialise.** A constructor has the same name as the class and no return type. It runs once when `new Account(…)` is called and is the only place where `final` fields can be assigned. If you don't write a constructor, Java synthesises a no-argument default.

**`this` is the current instance.** Inside an instance method or constructor, `this` refers to the object the call is on. Use it to disambiguate when a parameter shadows a field (`this.id = id`).

**Static vs instance.** A `static` member belongs to the class itself, not to any one instance — there is exactly one copy shared by everyone. Instance members exist once per object.

**A file holds one public class.** Java allows multiple classes in one file, but only one may be `public`, and the file name must match it (`Account.java` ⇒ `public class Account`).

## Related

- Rule: [`equals-without-hashcode`](../../rules/oop/equals-without-hashcode.md) — a class-scope contract violation.
- Rule: [`public-mutable-static-field`](../../rules/oop/public-mutable-static-field.md) — when a static field leaks state.
- Rule: [`default-package`](../../rules/oop/default-package.md) — why every class needs a `package` declaration.
