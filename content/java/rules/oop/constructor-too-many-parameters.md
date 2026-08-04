---
id: constructor-too-many-parameters
language: java
applies-to: [constructor_declaration]
match:
  parameter_count: { in: ["5", "6", "7", "8", "9", "10", "11", "12"] }
severity: info
kind: structure
sources:
  - "Effective Java, Item 2 — Consider a builder when faced with many constructor parameters"
  - "Refactoring (Fowler) — Replace Constructor with Factory Method"
  - "Java_L Module 08 — OOP Basics (Constructors)"
---

# Constructor takes too many parameters — consider a builder

## Bad

```java
public class NutritionFacts {
    public NutritionFacts(
            int servingSize,
            int servings,
            int calories,
            int fat,
            int sodium,
            int carbohydrate,
            int protein,
            int cholesterol) {
        // … assign all eight ...
    }
}

// Call site — five `0`s, two of which are in the wrong slots; no compiler error.
NutritionFacts cola = new NutritionFacts(240, 8, 100, 0, 35, 27, 0, 0);
```

## Good

```java
public class NutritionFacts {
    private NutritionFacts(Builder b) { /* … */ }

    public static class Builder {
        private final int servingSize;
        private final int servings;
        private int calories = 0;
        private int fat = 0;
        // … one with-method per field …

        public Builder(int servingSize, int servings) {
            this.servingSize = servingSize;
            this.servings = servings;
        }

        public Builder calories(int v) { this.calories = v; return this; }
        public Builder fat(int v)      { this.fat = v;      return this; }
        public NutritionFacts build()  { return new NutritionFacts(this); }
    }
}

// Call site — every value is labelled; missing optional fields default cleanly.
NutritionFacts cola = new NutritionFacts.Builder(240, 8)
    .calories(100)
    .sodium(35)
    .carbohydrate(27)
    .build();
```

## Why

A long positional argument list is hard to read at the call site and easy to mis-order. `new NutritionFacts(240, 8, 100, 0, 35, 27, 0, 0)` reveals nothing about which `0` is fat and which is cholesterol; a slip swaps them and the bug compiles cleanly. As parameters accumulate, the failure shape goes from "I forgot one" to "I mixed two up" — and there's no compiler help once the types match.

A builder gives every value a name at the call site, lets optional parameters stay unset (no telescoping overloads), and keeps the resulting instance immutable. Records (Java 16+) are the right answer when the type really is just data and all fields are mandatory; builders shine when there are sensible defaults or optional setters.

The 4–5 parameter threshold isn't a hard rule — it's where readability typically starts to suffer for same-typed args (lots of `int` / `String`). Constructors with five obviously-distinct types (`String name, BigDecimal price, Currency currency, LocalDate effective, Vendor vendor`) read fine. The smell is positional ambiguity, not parameter count alone — let that judgement override the threshold.
