---
id: optional-as-field-type
language: java
applies-to: [field_declaration]
match:
  type_name: Optional
severity: warning
kind: structure
sources:
  - "Effective Java, Item 55 — Return optionals judiciously"
  - "Stuart Marks (JDK architect) — Optional should never be used for fields"
  - "Java_L Module 18 — Optionals"
---

# `Optional` is for return values, not fields

## Bad

```java
public class Customer {
    private final String name;
    private final Optional<String> middleName;   // wrong shape
    private final Optional<Address> address;     // wrong shape

    public Customer(String name, Optional<String> middleName, Optional<Address> address) {
        this.name = name;
        this.middleName = middleName;
        this.address = address;
    }
}
```

## Good

```java
public class Customer {
    private final String name;
    private final String middleName;     // may be null
    private final Address address;       // may be null

    public Customer(String name, String middleName, Address address) {
        this.name = name;
        this.middleName = middleName;
        this.address = address;
    }

    // Optional appears at the API boundary — return values — not in storage.
    public Optional<String> getMiddleName() { return Optional.ofNullable(middleName); }
    public Optional<Address> getAddress()   { return Optional.ofNullable(address); }
}
```

## Why

`Optional<T>` was added in Java 8 with a specific purpose: as a *return type* that signals "this method may not produce a value, and the caller must explicitly handle the missing case." The JEP proposal, the JDK design team's commentary (notably Stuart Marks), and *Effective Java*'s Item 55 are all consistent on the same boundary: returns, yes; fields and parameters, no.

Three concrete reasons:

1. **Optional is not `Serializable`.** A field of type `Optional<X>` blocks the enclosing class from being serialised. The same goes for cloning, JPA persistence, and many other framework-managed lifecycles.
2. **It adds an indirection layer to every read.** A non-Optional `middleName` field is one heap reference; the `Optional<String>` form is *two* — the Optional wrapper, then the inner String. Every access goes through `Optional.get()` / `.isPresent()` / `.map()`, and the wrapper allocates one object per field per instance. Fine at the API boundary; expensive at scale in storage.
3. **It misrepresents the absence model.** Fields can be `null` at any time during an object's life (until set, after deserialisation, between mutations). Optional was designed as an *answer* to a question, not as the state of a slot. Using it as a slot leaks the "I'm answering you now" semantics into "I'm holding this forever."

The standard pattern is: store the underlying value (nullable when truly optional, never-null when required and validated in the constructor), and *return* `Optional.ofNullable(field)` from accessors that want to advertise the may-be-absent shape. Callers get the `Optional` API; storage stays one reference deep.

This rule also fires when the field's declared type is just `Optional` (raw). That's a strictly worse shape — the raw form gives up the element-type checking too — and should be fixed by either removing the Optional wrapper (per above) or, in the rare case Optional really is the right type, adding the type parameter.
