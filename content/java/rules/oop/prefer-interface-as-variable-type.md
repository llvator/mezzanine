---
id: prefer-interface-as-variable-type
language: java
applies-to: [declared_type]
match:
  name:
    in: [ArrayList, LinkedList, HashMap, LinkedHashMap, TreeMap, HashSet, LinkedHashSet, TreeSet, ArrayDeque]
  generic_args:
    present: true
severity: info
kind: style
sources:
  - "Effective Java, Item 64 — Refer to objects by their interfaces"
---

# Declare variables by their interface, not their implementation

## Bad

```java
public void process() {
    ArrayList<String> names = new ArrayList<>();   // tied to ArrayList specifically
    HashMap<String, Integer> counts = new HashMap<>();

    enrich(names);
    publish(counts);
}

private void enrich(ArrayList<String> names) { ... }   // signature locked to ArrayList
private void publish(HashMap<String, Integer> counts) { ... }
```

## Good

```java
public void process() {
    List<String> names = new ArrayList<>();        // interface on the left
    Map<String, Integer> counts = new HashMap<>();

    enrich(names);
    publish(counts);
}

private void enrich(List<String> names) { ... }     // accepts any List
private void publish(Map<String, Integer> counts) { ... }
```

## Why

The implementation type is an implementation detail; the interface is the contract. Declaring a local, field, parameter, or return type as the interface (`List`, `Map`, `Set`, `Deque`) lets you swap implementations later — `ArrayList` → `CopyOnWriteArrayList` for concurrent reads, `HashMap` → `LinkedHashMap` for predictable iteration order, `HashMap` → `ConcurrentHashMap` for multi-threaded access — without touching any of the call sites.

The narrow legitimate exceptions are when the implementation contract matters at the use site: `EnumMap` (better than `Map` for enum keys), `IdentityHashMap` (identity vs equals semantics), `LinkedHashMap` if you specifically rely on insertion order. In those cases, the implementation type *is* the contract you're depending on, and declaring it is honest.

This rule deliberately stays quiet on `new ArrayList<>()` on the right of the `=` — the *instantiation* must pick a concrete type, only the *declaration* benefits from the abstraction.
