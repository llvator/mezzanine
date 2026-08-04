---
id: raw-types-warning
language: java
applies-to: [declared_type]
match:
  name:
    in: [List, Map, Set, Collection, Iterator, Iterable, ArrayList, HashMap, HashSet, LinkedList, TreeMap, TreeSet, Optional, Future, CompletableFuture, Class, Comparable, Comparator, Stream]
  generic_args:
    absent: true
severity: warning
kind: gotcha
sources:
  - "Effective Java, Item 26 — Don't use raw types"
  - "JLS §4.8 — Raw Types"
---

# Don't declare generic types raw

## Bad

```java
List names = new ArrayList();
names.add("Ada");
names.add(42);                                // silently allowed
for (Object o : names) { /* ... */ }
```

## Good

```java
List<String> names = new ArrayList<>();
names.add("Ada");
names.add(42);                                // compile error — what we wanted
for (String o : names) { /* ... */ }
```

## Why

Raw types disable generic type-checking for the entire declaration. The compiler can no longer reject `names.add(42)` when `names` was meant to be `List<String>` — the type error becomes a `ClassCastException` at the point of use, often nowhere near the bad write.

Use a diamond (`new ArrayList<>()`) or `<String>` explicit type argument. If you genuinely need an unbounded generic (rare — usually because you're writing reflection or framework code), say `List<?>` rather than `List`. The wildcard expresses "I don't know the element type" precisely while keeping unchecked operations rejected.
