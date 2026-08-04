---
id: non-conventional-type-parameter-name
language: java
applies-to: [type_parameters]
match:
  has_non_conventional_name: "true"
severity: info
kind: naming
sources:
  - "Java Generics Tutorial — Type Parameter Naming Conventions"
  - "Effective Java, Item 68 — Adhere to generally accepted naming conventions"
  - "Java_L Module 14 — Generics"
---

# Use single-letter names for type parameters

## Bad

```java
public class Cache<KeyType, ValueType> {
    private final Map<KeyType, ValueType> store = new HashMap<>();

    public ValueType get(KeyType key) { return store.get(key); }
    public void put(KeyType key, ValueType value) { store.put(key, value); }
}

public static <Element> List<Element> only(Iterable<Element> xs) { /* … */ }
```

## Good

```java
public class Cache<K, V> {
    private final Map<K, V> store = new HashMap<>();

    public V get(K key) { return store.get(key); }
    public void put(K key, V value) { store.put(key, value); }
}

public static <E> List<E> only(Iterable<E> xs) { /* … */ }
```

## Why

The convention since Java 5 is that type parameters are single uppercase letters:

- `T` — a generic Type
- `E` — an Element type in a collection
- `K, V` — Key and Value types in a map
- `R` — a Return type
- `T1, T2, U` — additional type parameters when several appear together

The reason isn't aesthetic — it's that readers scan code looking for *classes* (which are capitalised words like `Account`, `Customer`, `Order`) and need to distinguish them from *type parameters* (which are placeholders). When a type parameter is also a capitalised word — `Element`, `KeyType` — a reader has to look it up to know whether `Element` is a class in the file's imports or a generic placeholder declared above. Single letters make that distinction unmistakable at a glance.

This is a naming-only rule, not a structural one — the code with `<KeyType, ValueType>` works correctly. But it costs the reader an extra parse on every method signature where the type parameters appear. The convention is universal enough (JDK, Guava, Spring, every major Java codebase) that the saving compounds.

The rule fires whenever any of the declared parameters violates the shape "uppercase letter, optionally followed by one digit." `T`, `K`, `V`, `T1`, `T2`, `R`, `U` all pass. `Type`, `t`, `Element`, `ResultType`, `KeyType` all fail.
