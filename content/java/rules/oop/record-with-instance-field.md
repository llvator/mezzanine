---
id: record-with-instance-field
language: java
applies-to: [record_declaration]
match:
  has_instance_field: "true"
severity: warning
kind: structure
sources:
  - "JEP 395 — Records (Java 16)"
  - "JLS §8.10.1 — Record Components"
  - "Java_L Module 19 — Records & Sealed Classes"
---

# Records carry state through components, not instance fields

## Bad

```java
public record Range(int low, int high) {
    private boolean validated;          // instance field — not allowed

    public boolean isValid() {
        if (!validated) {
            // … expensive check …
            validated = true;
        }
        return true;
    }
}
```

## Good

```java
public record Range(int low, int high) {
    public Range {
        // Validate in the compact constructor; the record is valid from
        // birth and we don't need to remember that we checked.
        if (low > high) {
            throw new IllegalArgumentException("low must be <= high");
        }
    }
}
```

## Why

Records are *value* types: the components in the header are the entirety of the state, and two records with the same components are by definition `.equals`. The Java compiler enforces that contract by *forbidding* instance fields in record bodies — adding one is a compile error.

If the rule is firing, the parser saw an instance field declaration inside a record body and the code is already broken. Two ways the situation usually arises:

1. **The author is mid-refactor from a class to a record.** Move every instance field into the header. If a field exists to *cache* a derived value (a memoised hash, a computed display string), the cache has no place in a record — the value is supposed to be cheap enough to recompute on every read.
2. **The "field" is actually a `static final` constant.** Those are allowed on records (they're class-level, not per-instance) and won't trigger the rule. If yours is `private final` rather than `private static final`, that's the mismatch — make it `static`.

The rule fires on any non-`static` field declaration inside the record body. The fix is always one of: move it into the header, make it `static`, or convert the record back to a class because the type genuinely needs identity or mutable instance state.
