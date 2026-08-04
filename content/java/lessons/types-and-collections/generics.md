---
id: generics-fundamentals
language: java
applies-to: [type_parameters]
title: "Generics — `<T>` parameters, bounds, and wildcards"
level: intermediate
sources:
  - "Java_L Module 14 — Generics"
  - "JLS §4.4 — Type Variables, §4.5 — Parameterized Types"
  - "Effective Java, Item 28 — Prefer lists to arrays (covariance contrast)"
  - "Effective Java, Item 31 — Use bounded wildcards to increase API flexibility (PECS)"
---

# Generics — `<T>` parameters, bounds, and wildcards

Generics let a class or method declare placeholders for types it works with — `T`, `E`, `K, V` — so the same code can be reused across many concrete types while the compiler still catches mismatches. The vocabulary is small (parameter, argument, bound, wildcard) but the rules around variance and erasure are where most surprises live.

## Syntax

```java
// Generic class — one type parameter, no bound.
public class Box<T> {
    private final T value;
    public Box(T value) { this.value = value; }
    public T get()      { return value; }
}

// Generic method — declares `<T>` before the return type.
public static <T> T identity(T x) { return x; }

// Bounded type parameter — T must be a Number subtype.
public static <T extends Number> double sum(List<T> xs) {
    double total = 0;
    for (T x : xs) total += x.doubleValue();
    return total;
}

// Bounded wildcards — PECS: Producer Extends, Consumer Super.
public static double sumOf(List<? extends Number> producer) { /* read T values out */ }
public static <T> void copy(List<? super T> consumer, List<? extends T> producer) {
    for (T item : producer) consumer.add(item);
}
```

## Key ideas

**Type parameters are placeholders, not subtypes.** `<T>` says "this code is parameterised by some type T that the caller picks." A class `Box<T>` is one declaration that produces many concrete types at use — `Box<String>`, `Box<Integer>`, `Box<List<String>>`. Inside `Box`, you can store and return `T`; you cannot `new T()`, cast `Object` to `T` safely without unchecked-warnings, or call `T.class`. Those limits all stem from one design choice: erasure.

**Generics are erased at runtime.** The JVM doesn't see `List<String>` and `List<Integer>` as different types — both are just `List` once compiled. That keeps generics binary-compatible with pre-generics Java, but it means `instanceof List<String>` doesn't compile (use `instanceof List<?>`), `T[]` arrays can't be created directly, and the compiler will not stop you from putting an `Object` into a raw `List` and reading it as a `String` later.

**Bounds let you call methods on `T`.** Without a bound, the compiler treats `T` as `Object` — you can only call `Object` methods. `<T extends Comparable<T>>` says "T must be comparable to itself," which lets the body call `t.compareTo(other)`. Multiple bounds use `&`: `<T extends Number & Comparable<T>>`. Bounds are always upper (extends); generics don't have "is-a-superclass-of" bounds on type parameters — wildcards do.

**Wildcards are about variance at the use site.** `List<? extends Number>` is "some list of a specific-but-unknown subtype of Number" — you can read `Number`s out, you cannot add anything (because the unknown subtype might be `Integer` and you'd be adding a `Double`). `List<? super Number>` is the inverse — write Numbers in, read out `Object`. The mnemonic is **PECS**: Producer **E**xtends, Consumer **S**uper. APIs that read from a collection use `<? extends T>`; APIs that write to one use `<? super T>`.

**Naming convention: one capital letter.** The convention since Java 5 is `T` for a generic type, `E` for an element type in collections, `K, V` for map keys and values, `R` for a return type, `T1, T2` when several appear together. Lowercase names (`t`), spelled-out names (`Element`, `Type`), or class-style names (`ItemType`) violate the convention and are flagged by most style guides; reading code is easier when the letters consistently signal "this is a type parameter, not a class."

## Related

- Rule: [`non-conventional-type-parameter-name`](../../rules/types-and-collections/non-conventional-type-parameter-name.md) — flags type parameters that aren't single-letter conventional names.
- Rule: [`raw-types-warning`](../../rules/types-and-collections/raw-types-warning.md) — using a generic type without `<…>` defeats the whole system.
- Rule: [`prefer-diamond-over-guava-lists`](../../rules/types-and-collections/prefer-diamond-over-guava-lists.md) — modern `new ArrayList<>(…)` idiom for inferred type arguments.
- Lesson: [`declared-type-fundamentals`](declared-type.md) — generic types in declaration position.
