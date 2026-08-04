---
id: declared-type-fundamentals
language: java
applies-to: [declared_type]
title: Declared types — naming the shape of a value
level: beginner
sources:
  - "Java_L Module 02 — Variables (PrimitiveTypes, ReferenceTypes)"
  - "Java_L Module 11 — Generics"
  - "JLS §4 — Types, Values, and Variables"
---

# Declared types — naming the shape of a value

Every variable, field, parameter, and return value in Java carries a *declared type*. The compiler uses it to decide which methods are callable, which assignments are legal, and which conversions are silent vs. need a cast. The type you write at the declaration site is the contract the rest of the code reads.

## Syntax

```java
// Primitive types — the value itself is stored.
int count = 0;
double ratio = 0.5;
boolean ready = false;

// Reference types — a pointer to an object on the heap.
String name = "Ada";
Account owner = new Account("a-42", 0);

// Parameterised (generic) types — the type plus its type arguments.
List<String> names = new ArrayList<>();
Map<String, Account> byId = new HashMap<>();

// Raw type — generic class used without arguments. Legal, but the compiler warns.
List items = new ArrayList();
```

## Key ideas

**Primitives are not objects.** `int`, `long`, `double`, `boolean`, `char`, `byte`, `short`, `float` live by value, can't be `null`, and have no methods. Each one has a boxed wrapper (`Integer`, `Long`, …) that *is* an object — useful when a collection or generic insists on a reference type, dangerous when you use `==` on the wrapper.

**Reference types refer to heap objects.** The variable holds an address; two variables pointing at the same object see the same mutations. Reference types are everything class- or interface-shaped: `String`, `List`, your own `Account`, arrays (`int[]`, `String[]`).

**Generics make container types specific.** `List<String>` is a list whose elements the compiler will treat as `String`. The type arguments are checked at compile time and erased at runtime — at runtime, `List<String>` and `List<Integer>` are both just `List`. That's why you can't write `new List<String>[10]` and why `instanceof List<String>` is illegal.

**Declare with the widest sensible interface.** Use `List` rather than `ArrayList` on a field or parameter type unless callers genuinely need the concrete class's extra contract. The narrower the declared type, the more code you'll have to change to swap the implementation later.

## Related

- Rule: [`raw-types-warning`](../../rules/types-and-collections/raw-types-warning.md) — when a generic type is used without arguments.
- Rule: [`legacy-thread-safe-collections`](../../rules/concurrency/legacy-thread-safe-collections.md) — `Vector` / `Hashtable` / `Stack` as declared types.
- Rule: [`legacy-date-time`](../../rules/types-and-collections/legacy-date-time.md) — `Date` / `Calendar` as declared types.
- Rule: [`prefer-interface-as-variable-type`](../../rules/oop/prefer-interface-as-variable-type.md) — narrow concrete types vs. broader interfaces.
