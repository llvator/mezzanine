---
id: prefer-list-of-over-singleton-list
language: java
applies-to: [method_invocation]
match:
  call_target: { in: [Collections.singletonList, Collections.singletonMap, Collections.singleton, Collections.emptyList, Collections.emptyMap, Collections.emptySet] }
severity: info
kind: style
sources:
  - "JEP 269 — Convenience Factory Methods for Collections (Java 9)"
  - "Effective Java, Item 50 — Make defensive copies when needed (related: returning immutable views)"
  - "Java_L Module 13 — Collections Framework"
---

# Prefer `List.of(…)` / `Map.of(…)` / `Set.of(…)` over the `Collections.singleton…` / `Collections.empty…` helpers

## Bad

```java
import java.util.Collections;
import java.util.List;
import java.util.Map;

public class WeekendPolicy {
    private static final List<String> WEEKEND_DAYS  = Collections.singletonList("SAT");
    private static final List<String> NO_HOLIDAYS   = Collections.emptyList();
    private static final Map<String, Integer> RATES = Collections.singletonMap("standard", 1);
}
```

## Good

```java
import java.util.List;
import java.util.Map;

public class WeekendPolicy {
    private static final List<String> WEEKEND_DAYS  = List.of("SAT");
    private static final List<String> NO_HOLIDAYS   = List.of();
    private static final Map<String, Integer> RATES = Map.of("standard", 1);
}
```

## Why

The `Collections.singletonList(x)`, `Collections.emptyList()`, and friends predate Java 9. They returned immutable collections back when there was no other concise way to construct one. Java 9 added the `List.of(…)`, `Set.of(…)`, and `Map.of(…)` factories, which:

- read the same regardless of element count (`List.of()`, `List.of("a")`, `List.of("a", "b", "c")` — same shape),
- give better performance for small collections (specialised internal implementations for 0–10 elements),
- enforce immutability the same way (mutator methods throw `UnsupportedOperationException`),
- and remove the import of `java.util.Collections` for static utility methods that are no longer needed.

There's no behavioural difference at the call site for the common cases — both forms return unmodifiable collections — but the modern shape is uniform, shorter, and one less unfamiliar name in the file. Mixed usage (`List.of(…)` for two-or-more, `Collections.singletonList(…)` for one) is the smell: pick the modern form once and keep it consistent.

`Collections.unmodifiableList(new ArrayList<>(…))` is the older verbose pattern for "I built this mutably and want to publish it immutably." That's still legitimate when the build step needs mutation; consider `Stream.toList()` (Java 16+) when the construction is from a stream.
