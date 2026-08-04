---
id: array-creation-fundamentals
language: java
applies-to: [array_creation_expression]
title: "Array creation — `new T[n]` vs `new T[]{…}`"
level: beginner
sources:
  - "Java_L Module 06 — Arrays (ArrayBasics)"
  - "JLS §15.10.1 — Array Creation Expressions"
  - "Effective Java, Item 28 — Prefer lists to arrays"
---

# Array creation — `new T[n]` vs `new T[]{…}`

A Java array is a fixed-size, type-homogeneous container. The two creation forms differ in what they fix at construction: the sized form fixes the length and leaves elements at type defaults; the literal form fixes both length *and* contents.

## Syntax

```java
// Sized form — length n, every slot holds the type's default (0 / false / null).
int[] quantities = new int[5];          // [0, 0, 0, 0, 0]
String[] names = new String[3];          // [null, null, null]

// Literal form — length and contents inferred from the brace list.
int[] primes = new int[]{2, 3, 5, 7, 11};
String[] greetings = new String[]{"hi", "yo"};

// Initializer shorthand — only legal at declaration time.
int[] votes = {1, 1, 2, 3, 5};

// Multidimensional — outer length required; inner lengths optional.
int[][] grid = new int[3][3];   // fully allocated
int[][] jagged = new int[3][];  // outer allocated; inner rows null until set
```

## Key ideas

**Length is fixed at allocation and cannot change.** `array.length` is a property, not a method, and is set once at `new`. To grow or shrink, allocate a new array and copy — that's almost always the signal to reach for `ArrayList` instead, which handles the dance for you.

**The sized form returns default-initialised slots.** Numeric arrays start at `0`, `boolean` at `false`, reference arrays at `null`. The default for references is the source of a very common NullPointerException — iterating a `String[]` you just allocated will hit `null` on every slot until you assign each one.

**Arrays are covariant; generics are not.** `String[]` is assignable to `Object[]`, which sounds convenient but lets you put a non-`String` into a `String[]` reference and discover the mismatch at runtime (`ArrayStoreException`). `List<String>` is not assignable to `List<Object>`, which catches the same mistake at compile time. That's the main reason to prefer `List` to `T[]` in modern code.

**For fixed-size literal data, `List.of(…)` reads as well and avoids the gotchas.** `List.of("hi", "yo")` is immutable, type-safe, can be used directly with collections APIs, and prints clearly with `toString`. Use a true array only when an API forces one (varargs implementation, primitive performance work, `Object[]` reflective glue).

## Related

- Rule: [`prefer-list-of-over-array-literal`](../../rules/types-and-collections/prefer-list-of-over-array-literal.md) — suggests `List.of(…)` over `new T[]{…}` when the literal is reference-typed.
- Rule: [`arrays-aslist-mutability`](../../rules/types-and-collections/arrays-aslist-mutability.md) — the older `Arrays.asList(…)` shape and why `List.of` replaced it.
- Lesson: [`declared-type-fundamentals`](declared-type.md) — why `List<T>` usually beats `T[]`.
