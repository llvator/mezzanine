---
id: enum-prefer-immutable-fields
language: java
applies-to: [enum_declaration]
match:
  has_mutable_field: "true"
severity: warning
kind: gotcha
sources:
  - "Effective Java, Item 17 — Minimize mutability"
  - "Effective Java, Item 34 — Use enums instead of int constants"
  - "Java_L Module 15 — Enums"
---

# Enum instance fields should be `final`

## Bad

```java
public enum HttpStatus {
    OK(200),
    NOT_FOUND(404);

    private int code;            // not final — anyone with a reference can mutate it
    private int hitCount = 0;    // not final — and shared across every reference

    HttpStatus(int code) {
        this.code = code;
    }

    public int code()           { return code; }
    public void recordHit()     { hitCount++; }  // mutating a singleton
    public void overrideCode(int c) { this.code = c; }
}
```

## Good

```java
public enum HttpStatus {
    OK(200),
    NOT_FOUND(404);

    private final int code;

    HttpStatus(int code) {
        this.code = code;
    }

    public int code() { return code; }
}

// Per-constant *mutable* state, when you genuinely need it, lives outside
// the enum so the singleton stays immutable:
private static final Map<HttpStatus, AtomicLong> HIT_COUNTS =
    new EnumMap<>(HttpStatus.class);
```

## Why

Enum constants are JVM-managed singletons — there is exactly one `HttpStatus.OK` for the lifetime of the program. A non-`final` instance field on the enum is shared across *every* reference to that constant, so a mutation in one method is observable everywhere `HttpStatus.OK` is used: by other threads, in other classes, in tests that share the JVM. The mutation outlives the call that made it; the enum is no longer a constant in any meaningful sense.

Two failure modes follow:

1. **Surprising aliasing.** Code that does `if (status == HttpStatus.OK)` is comparing against a *value* it assumes is fixed. If `OK.code` can be reassigned, the same comparison can mean different things in different runs of the program.
2. **Thread-safety hazards.** Two threads hitting `recordHit()` race on a non-`final`, non-atomic field. The fix is usually `AtomicLong`, but the deeper point is that "let the enum hold mutable state" was the wrong shape — the mutable state belongs in a separate keyed-by-enum structure like `EnumMap<HttpStatus, AtomicLong>`.

If you need per-constant *configuration* (a code, a label, a default), put it in a `final` field populated by the constructor. If you need per-constant *mutable state*, put it in a `Map<EnumType, Whatever>` keyed by the enum — the enum stays a clean type-safe key, the mutation lives in the value.

`static` fields on the enum are fine (they're class-level state, not per-constant), and so are immutable references to mutable objects (a `final List<X>` whose contents you never touch). The rule fires on non-`static`, non-`final` instance fields specifically.
