---
id: method-reference-fundamentals
language: java
applies-to: [method_reference]
title: "Method references — `::` as a compact lambda"
level: intermediate
sources:
  - "Java_L Module 16 — Lambdas and Method References"
  - "JLS §15.13 — Method Reference Expressions"
  - "Effective Java, Item 43 — Prefer method references to lambdas"
---

# Method references — `::` as a compact lambda

A method reference (`Class::method`, `instance::method`, `Class::new`) is shorthand for a lambda whose body is a single call to a named target. Where a lambda *describes* a function inline, a method reference *names* one that already exists. Both produce instances of the same functional interface — the JVM doesn't distinguish them at runtime.

## Syntax

```java
List<String> names = List.of("Maria", "Alex", "Bo");

// 1. Static method reference — Class::staticMethod
names.stream().mapToInt(Integer::parseInt);             // s -> Integer.parseInt(s)

// 2. Unbound instance method reference — Class::instanceMethod
names.stream().map(String::toUpperCase);                // s -> s.toUpperCase()

// 3. Bound instance method reference — instance::method
PrintStream out = System.out;
names.forEach(out::println);                            // s -> out.println(s)

// 4. Constructor reference — Class::new
names.stream().map(StringBuilder::new);                 // s -> new StringBuilder(s)
```

## Key ideas

**Four shapes, one mental model.** Every method reference targets a single callable — a static method, an unbound instance method (where the lambda's first parameter becomes the receiver), a bound instance method (where the receiver is captured at the reference site), or a constructor. The compiler matches the target's signature against the functional interface the context expects; a mismatch is a compile error, not a runtime surprise.

**Prefer a method reference when the lambda body is one call.** `s -> s.toUpperCase()` reads as "take s, do .toUpperCase() to it" — two mentions of `s` to make one operation. `String::toUpperCase` is the same operation with no invented parameter name. *Effective Java*'s Item 43 puts the rule directly: if the method reference is shorter and at least as readable, use it. The lambda form earns its keep when the body *adds* work — a transformation, a conditional, a multi-call chain.

**The receiver of a bound reference is captured once, at the reference site.** `out::println` captures whatever `out` referred to at that moment; reassigning `out` afterward doesn't change the reference. This is exactly the same capture rule that applies to lambdas, just visible through different syntax — and it's why bound references over fields can hold a class instance alive longer than expected.

**`Class::method` is ambiguous between static and unbound-instance.** `String::valueOf` is static (the method belongs to the `String` class). `String::length` is unbound instance (the lambda's first parameter becomes `this`). The compiler resolves which one based on the target functional interface; the syntax doesn't distinguish. A reader without IDE help has to know the method to know which form they're seeing.

**Constructor references shine for collection conversions.** `.collect(Collectors.toCollection(LinkedHashSet::new))` reads as "into a fresh `LinkedHashSet`." For factory-style code (`Function<String, StringBuilder> factory = StringBuilder::new`), the constructor reference makes the "I make these on demand" intent obvious.

## Related

- Lesson: [`lambda-expression-fundamentals`](lambda-expression.md) — the more general syntax; method references are a specialised shape.
- Lesson: [`method-invocation-fundamentals`](method-invocation.md) — what a method reference reduces to once the JVM calls through it.
- Rule: [`lambda-block-body-extract-method`](../../rules/expressions/lambda-block-body-extract-method.md) — when a lambda's *block* body has grown past the one-expression shape, the next step is usually a named method (which can be reached via `::`).
