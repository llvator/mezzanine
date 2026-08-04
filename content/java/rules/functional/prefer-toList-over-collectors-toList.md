---
id: prefer-toList-over-collectors-toList
language: java
applies-to: [method_invocation]
match:
  call_target: { in: [Collectors.toList, Collectors.toUnmodifiableList, Collectors.toSet, Collectors.toUnmodifiableSet] }
severity: info
kind: style
sources:
  - "JEP 408 — Stream.toList (Java 16)"
  - "Effective Java, Item 45 — Use streams judiciously"
  - "Java_L Module 17 — Streams API"
---

# Prefer `Stream.toList()` over `.collect(Collectors.toList())`

## Bad

```java
import java.util.List;
import java.util.stream.Collectors;

public class Reports {
    public List<String> activeCustomerNames(List<Order> orders) {
        return orders.stream()
            .filter(Order::isActive)
            .map(Order::getCustomerName)
            .collect(Collectors.toList());
    }
}
```

## Good

```java
import java.util.List;

public class Reports {
    public List<String> activeCustomerNames(List<Order> orders) {
        return orders.stream()
            .filter(Order::isActive)
            .map(Order::getCustomerName)
            .toList();
    }
}
```

## Why

`Stream.toList()` was added in Java 16 specifically to replace the verbose `.collect(Collectors.toList())` shape that had been the only option since streams shipped. It does the same job, but:

- it returns an **unmodifiable** `List` (the `Collectors.toList()` form returns a mutable `ArrayList`, which has been a longstanding "do not rely on this" detail — the spec only promises *some* List, not which kind),
- it drops the import of `java.util.stream.Collectors` when that's the only thing pulling it in,
- it's shorter at every call site, and
- the JVM has more room to optimise (no intermediate `Collector` object, no mutable accumulation).

The same upgrade applies to `Collectors.toSet()` / `toUnmodifiableSet()` / `toUnmodifiableList()` — although there's no Java-16 `toSet()` shortcut on `Stream` itself, the `Collectors.toUnmodifiableSet()` form is itself preferred over the mutable `Collectors.toSet()` for end-user APIs that shouldn't expose mutation.

There are two genuine reasons to keep the `Collectors.toList()` form:

1. You need a *mutable* `List` to add to later. In that case, prefer the more explicit `.collect(Collectors.toCollection(ArrayList::new))` — it documents the intent.
2. You're targeting a pre-Java-16 baseline. Rare in greenfield work, but real in long-lived codebases.

For everything else, the modern form is the default.
