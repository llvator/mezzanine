---
id: collections-fundamentals
language: java
applies-to: [declared_type]
title: "Collections — `List`, `Set`, `Map` and the interface-implementation split"
level: beginner
sources:
  - "Java_L Module 13 — Collections Framework"
  - "JLS §17 — Collections"
  - "Effective Java, Item 25 — Limit source files to a single top-level class (related context)"
  - "Effective Java, Item 64 — Refer to objects by their interfaces"
---

# Collections — `List`, `Set`, `Map` and the interface-implementation split

The Collections Framework is Java's standard library for groups of objects. Three interfaces shape almost all of it: `List<E>` (ordered, allows duplicates, indexed access), `Set<E>` (no duplicates, no positional access), `Map<K, V>` (key→value, no duplicate keys). Most Java code holds collections by their *interface* and picks an implementation at the point of construction — that split is one of the framework's clearest design wins.

## Syntax

```java
import java.util.*;

// Hold by interface; choose the implementation when you construct.
List<String> names = new ArrayList<>();          // fast indexed access
Set<String>  seen  = new HashSet<>();            // O(1) contains
Map<String, Integer> counts = new HashMap<>();    // O(1) lookup

names.add("Maria");
seen.add("Maria");
counts.merge("Maria", 1, Integer::sum);

// Immutable factories (Java 9+) — preferred for fixed data and constants.
List<String> weekend = List.of("Sat", "Sun");
Set<Integer> primes  = Set.of(2, 3, 5, 7, 11);
Map<String, Integer> ranks = Map.of("gold", 1, "silver", 2, "bronze", 3);
```

## Key ideas

**Pick the interface by what the data *is*, not how it's stored.** `List` when order matters or you'll index into it; `Set` when duplicates are nonsense; `Map` when you'll look up by key. The implementation choice (`ArrayList` vs `LinkedList`, `HashSet` vs `LinkedHashSet` vs `TreeSet`, `HashMap` vs `LinkedHashMap` vs `TreeMap`) is a follow-up — it controls iteration order, complexity guarantees, and null-handling, but rarely the *shape* of your code.

**Declare by interface, construct by implementation.** `List<String> names = new ArrayList<>()` is the canonical line. Holding the field as `List` means later swapping in an `ImmutableList` or a `LinkedList` doesn't ripple through every method signature. The opposite — `ArrayList<String> names` — locks every caller into knowing the concrete type. *Effective Java*'s Item 64 puts the rule plainly: refer to objects by their interfaces.

**`List.of(…)` / `Set.of(…)` / `Map.of(…)` are the modern immutable factories.** Java 9 added them to replace the older `Arrays.asList(…)` (fixed-size, mutable elements), `Collections.singletonList(x)` (one element), and `Collections.unmodifiableList(new ArrayList<>(…))` (verbose). The factory forms are unmodifiable end-to-end — `add` / `set` / `remove` all throw — and the bytecode is more efficient. For constants and configuration, reach for these first.

**Iteration has three idiomatic forms and one anti-pattern.** Enhanced for-each (`for (T item : coll)`) is the default. `forEach(action)` with a lambda or method reference is the same iteration as an expression. The `Iterator` API is the only way to *remove* during iteration. The anti-pattern is index-based iteration over a `List` you'd otherwise read sequentially — slower on `LinkedList`, brittle on any list, and noisier than for-each.

**`Map` is not a collection; it's a key-value table.** It doesn't implement `Collection`, and you read its contents through three different views: `keySet()`, `values()`, and `entrySet()`. The `entrySet()` form is usually the one you want for iteration — it avoids the second lookup `for (K k : map.keySet()) map.get(k)` does.

## Related

- Rule: [`prefer-list-of-over-singleton-list`](../../rules/types-and-collections/prefer-list-of-over-singleton-list.md) — replaces the legacy `Collections.singletonList`/`singletonMap` shapes.
- Rule: [`prefer-interface-as-variable-type`](../../rules/oop/prefer-interface-as-variable-type.md) — when a variable's declared type is the implementation instead of the interface.
- Rule: [`raw-types-warning`](../../rules/types-and-collections/raw-types-warning.md) — `List` without `<E>` defeats the type system.
- Rule: [`prefer-concurrent-collections`](../../rules/concurrency/prefer-concurrent-collections.md) — when shared mutation calls for `ConcurrentHashMap` over `Collections.synchronized…`.
- Lesson: [`declared-type-fundamentals`](declared-type.md) — the type-vs-implementation distinction in general.
