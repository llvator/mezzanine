---
id: default-method-in-interface
language: java
applies-to: [method_declaration]
match:
  is_default: "true"
severity: info
kind: structure
sources:
  - "Java_L coding guidelines — interfaces.md (`Are the methods in your class default method -> avoid breaking the existing class that implement the interface`)"
  - "Effective Java, Item 21 — Design interfaces for posterity"
---

# Use `default` interface methods sparingly

## Bad

```java
public interface Repository<T> {
    Optional<T> findById(String id);

    // Convenient — but every existing implementation now silently inherits
    // a body that may not match its contract.
    default List<T> findAll() {
        throw new UnsupportedOperationException("not implemented");
    }
}
```

## Good

```java
public interface Repository<T> {
    Optional<T> findById(String id);
    List<T> findAll();   // abstract — forces a deliberate choice in each impl
}

// Plus, if there's a sensible default, an abstract base class:
public abstract class AbstractRepository<T> implements Repository<T> {
    @Override
    public List<T> findAll() {
        return Collections.emptyList();
    }
}
```

## Why

`default` methods were added to Java 8 specifically to let `java.util.Collection` and friends gain new methods (`stream()`, `forEach`) without breaking every implementation in the wild. That's a narrow use case — *retroactively extending a stable, widely-implemented interface*. Reaching for `default` in your own interfaces inverts the seam: implementers no longer have to think about the new method, and you've baked behaviour into the interface that should have been opt-in.

When you genuinely need a default implementation, an abstract base class expresses it more honestly — implementers explicitly opt in by extending it. The interface stays a pure contract.

A separate good use of `default`: skip making *utility* methods that don't depend on instance state — those should be `static` on the interface, not `default`.
