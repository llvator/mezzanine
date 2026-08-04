---
id: prefer-list-of-over-array-literal
language: java
applies-to: [array_creation_expression]
match:
  has_initializer: "true"
severity: info
kind: style
sources:
  - "Effective Java, Item 28 — Prefer lists to arrays"
  - "Effective Java, Item 32 — Combine generics and varargs judiciously (related: covariant arrays)"
  - "Java_L Module 06 — Arrays (ArrayBasics)"
---

# Prefer `List.of(…)` to `new T[]{…}`

## Bad

```java
public class WeekendPolicy {
    private static final String[] WEEKEND_DAYS =
        new String[]{"SATURDAY", "SUNDAY"};

    public boolean isWeekend(String day) {
        for (String d : WEEKEND_DAYS) {
            if (d.equals(day)) return true;
        }
        return false;
    }
}
```

## Good

```java
public class WeekendPolicy {
    private static final List<String> WEEKEND_DAYS =
        List.of("SATURDAY", "SUNDAY");

    public boolean isWeekend(String day) {
        return WEEKEND_DAYS.contains(day);
    }
}
```

## Why

Array literals were Java's only option for fixed-size collections until Java 9. They still work, but they bring two failure modes that `List.of(…)` does not:

1. **Arrays are covariant**, so `String[]` is assignable to `Object[]`. Storing the wrong type compiles cleanly and blows up at runtime with `ArrayStoreException`. `List<String>` is invariant — the same mistake doesn't compile.
2. **Arrays aren't immutable**, just fixed-size. Anyone with a reference can mutate the contents. `List.of(…)` is genuinely unmodifiable; `add` / `set` throw `UnsupportedOperationException`.

`List.of(…)` also reads better at the call site (`contains`, `stream`, `forEach` come for free), prints meaningfully with `toString`, and integrates with the Collections API everywhere a `Collection<E>` is expected. The remaining genuine use cases for `T[]` are: varargs implementation, primitive performance work, and reflective glue. For configuration data, lookup tables, and `static final` constants, prefer the list form.
