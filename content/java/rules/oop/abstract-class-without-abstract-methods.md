---
id: abstract-class-without-abstract-methods
language: java
applies-to: [class_declaration]
match:
  is_abstract: "true"
  has_abstract_methods: "false"
severity: info
kind: structure
sources:
  - "Effective Java, Item 19 — Design and document for inheritance or else prohibit it"
  - "Sonar rule java:S1610 — Abstract classes without fields should be converted to interfaces"
  - "Java_L Module 09 — Inheritance (AbstractClasses)"
---

# `abstract` class with no abstract methods — pick a clearer shape

## Bad

```java
public abstract class JsonSerializer {
    public String toJson(Object value) {
        // … shared logic only — no abstract members …
        return GSON.toJson(value);
    }

    public Object fromJson(String json, Class<?> type) {
        return GSON.fromJson(json, type);
    }
}
```

## Good

```java
// Option A: It's really a utility class — make it un-instantiable.
public final class JsonSerializer {
    private JsonSerializer() { /* prevent instantiation */ }

    public static String toJson(Object value) {
        return GSON.toJson(value);
    }

    public static Object fromJson(String json, Class<?> type) {
        return GSON.fromJson(json, type);
    }
}

// Option B: It's really a contract — make it an interface.
public interface JsonSerializer {
    String toJson(Object value);
    Object fromJson(String json, Class<?> type);
}
```

## Why

`abstract` exists to declare incompleteness — "subclasses must finish me." A class that's `abstract` but has no `abstract` members declares the contract but never enforces it. Subclasses get told "you're inheriting from an abstract class" but the compiler can't check that they actually contributed anything; the only effect of `abstract` is that callers can't write `new JsonSerializer()`.

There's almost always a better-fitting shape:

- **Utility code with only static helpers** → a `final` class with a `private` constructor. The compiler now enforces the "no instances" contract, and readers see the intent immediately.
- **A contract that callers implement** → an `interface`. Stateless, multiple-inheritance-friendly, and the compiler enforces that implementors provide each method.
- **A real partial implementation** → make at least one method `abstract`. That's the genuine reason to use the keyword.

This rule doesn't fire on abstract classes that hold instance fields or that protect a partial state machine — those have legitimate "shared state and subclass-supplied behaviour" patterns. It fires when the class is `abstract` purely for "users shouldn't instantiate me" reasons, which is what `final` + `private` constructor (or `interface`) say more clearly.
