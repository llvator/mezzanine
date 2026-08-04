---
id: arrays-aslist-mutability
language: java
applies-to: [method_invocation]
match:
  call_target: Arrays.asList
severity: warning
kind: gotcha
sources:
  - "Java Collections Framework — Arrays.asList Javadoc"
---

# `Arrays.asList(...)` returns a fixed-size view, not a real list

## Bad

```java
List<String> items = Arrays.asList("a", "b", "c");
items.add("d");                              // UnsupportedOperationException
items.remove(0);                             // UnsupportedOperationException
```

## Good

```java
// Need a mutable list?
List<String> items = new ArrayList<>(Arrays.asList("a", "b", "c"));

// Just need an immutable literal-style list?
List<String> items = List.of("a", "b", "c");
```

## Why

`Arrays.asList(...)` returns a fixed-size `java.util.Arrays$ArrayList` — a thin wrapper backed by the original array. You can call `set(i, v)` (it writes through to the array), but `add` and `remove` throw `UnsupportedOperationException` because the size is fixed. Worse, mutations *do* leak into the original array if the caller still holds a reference.

For an immutable list literal, prefer `List.of(...)` (Java 9+) — it actually rejects all mutation and disallows `null` elements. For a mutable list, copy through `new ArrayList<>(...)` explicitly so the resizing contract is unmistakable.
