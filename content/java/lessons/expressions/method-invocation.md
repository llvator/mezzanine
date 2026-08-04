---
id: method-invocation-fundamentals
language: java
applies-to: [method_invocation]
title: Method calls — invoking behaviour on a receiver
level: beginner
sources:
  - "Java_L Module 07 — Methods (MethodCalls, PassByValue)"
  - "Java_L Module 08 — OOP Basics (Polymorphism)"
  - "JLS §15.12 — Method Invocation Expressions"
---

# Method calls — invoking behaviour on a receiver

A *method invocation* runs a named block of code, optionally on a specific object (the *receiver*), passing in arguments and getting back a value (or `void`). It's the most common expression in Java code; almost every line that does work is, or contains, a call.

## Syntax

```java
// On an instance — `account` is the receiver.
long balance = account.getBalance();

// On a class — static call, no receiver instance needed.
int n = Integer.parseInt("42");

// On the current object — receiver is `this`, often implicit.
this.deposit(100);
deposit(100);                    // same call, `this.` elided

// On the parent class — explicit upward dispatch.
super.toString();

// Chained — each call's return value is the next call's receiver.
String trimmed = " hi ".trim().toUpperCase();
```

## Key ideas

**The receiver decides which method runs.** For an instance call on a reference of declared type `T`, Java looks at the *runtime* type of the object — not the declared type — and dispatches to that class's override. This is dynamic dispatch (polymorphism), and it's how `List<String>` calls `add` correctly whether the runtime object is `ArrayList` or `LinkedList`.

**Arguments are passed by value.** Java copies primitives into parameter slots; for object types it copies the *reference*. The callee can mutate the object the reference points at, but cannot make the caller's variable point at a different object.

**Overload resolution is compile-time.** When multiple methods share a name, the compiler picks the most specific match using the *declared* types of the arguments — not the runtime types. That's distinct from dispatch on the receiver, which is runtime. Mixing the two trips beginners up: `f((Object) "hi")` and `f("hi")` may bind to different overloads even though the actual object is identical.

**Static calls don't dispatch.** A call like `Foo.bar()` is resolved against `Foo` at compile time. If a subclass `Bar extends Foo` declares `static bar()`, that's *hiding*, not overriding — the call site decides which class's static method runs based on the syntactic receiver.

**Chained calls require non-null intermediate results.** `a.b().c().d()` calls `c()` on whatever `b()` returns; if that's `null`, the next call throws `NullPointerException`. The `Optional` API (`Optional.ofNullable(x).map(…).orElse(…)`) and the `?.` operator in other languages exist to manage this; plain Java leaves you to guard each return manually or to trust the contract.

## Related

- Rule: [`arrays-aslist-mutability`](../../rules/types-and-collections/arrays-aslist-mutability.md) — when one specific call's return value is misleading.
- Rule: [`system-out-println`](../../rules/logging/system-out-println.md) — when a particular call target is itself the smell.
- Lesson: [`method-declaration`](../oop/method-declaration.md) — the other side of a call: what a method declaration says about the contract.
