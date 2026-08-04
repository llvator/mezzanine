---
id: lambda-expression-fundamentals
language: java
applies-to: [lambda_expression]
title: "Lambdas — anonymous functions for functional interfaces"
level: intermediate
sources:
  - "Java_L Module 13 — Lambdas and Functional Interfaces"
  - "JLS §15.27 — Lambda Expressions"
  - "Effective Java, Item 42 — Prefer lambdas to anonymous classes"
---

# Lambdas — anonymous functions for functional interfaces

A *lambda expression* is a compact syntax for declaring a function inline. The compiler converts each lambda into an instance of a *functional interface* — an interface declaring exactly one abstract method — based on the type the surrounding context expects.

## Syntax

```java
import java.util.List;
import java.util.Map;
import java.util.function.Function;

public class Demo {
    public void demo(List<String> names, Map<String, Integer> counts) {
        // Single-parameter, expression body — no parens, no braces, no return.
        names.forEach(name -> System.out.println(name));

        // Multi-parameter, expression body — parens around the list.
        names.sort((a, b) -> a.length() - b.length());

        // Block body — braces required, `return` required for non-void.
        Function<String, Integer> parse = s -> {
            String trimmed = s.trim();
            return Integer.parseInt(trimmed);
        };

        // Auto-grows-on-miss — the lambda is the value factory.
        counts.computeIfAbsent("hits", k -> 0);
    }
}
```

## Key ideas

**A lambda is an interface instance, not a free-standing function.** The compiler picks the *target type* from context — the parameter type of `forEach`, the right-hand side of a `Function<String, Integer>` declaration, the second arg of `computeIfAbsent`. Every lambda becomes an instance of some single-abstract-method interface (`Consumer`, `Function`, `Predicate`, `Comparator`, …); two lambdas with identical bodies but different target types are different types at runtime.

**Parameter syntax has three shapes.** A single bare identifier (`x -> …`) needs no parens. A list of inferred-type parameters needs parens (`(a, b) -> …`) — and so does a single parameter when you want explicit types (`(int a) -> …`). The compiler picks the parameter types from the target interface; spelling them out is legal but rare.

**Expression body vs block body.** A single expression after the `->` is implicitly the return value — no `return` keyword, no semicolon. A `{ … }` block body, in contrast, is a full statement list: every non-void branch must `return` explicitly, and every statement ends in `;`. Block-body lambdas of more than four or five lines usually want to become a named method — the lambda was meant to be a value, not a place to grow logic.

**Captures are read-only references.** A lambda may reference local variables from the enclosing method, but only ones that are *effectively final* (assigned exactly once). The compiler captures the variable's value at the moment the lambda is created, not a live binding — reassigning the outer variable after the lambda is built has no effect. Fields, in contrast, are captured through `this` and remain live; mutating a field from inside a lambda sees the mutation.

**Lambdas are not closures over `this` the way anonymous classes are.** Inside a lambda, `this` refers to the *enclosing* instance, not to the lambda itself. An anonymous inner class would shadow `this` with its own instance; a lambda doesn't. That's usually what you want, but it's the visible behaviour change when refactoring an anonymous class to a lambda.

**Prefer a method reference when the body is one call.** `name -> System.out.println(name)` is just `System.out::println`. The method-reference form is shorter, names the call target explicitly, and avoids inventing a parameter name. Keep the lambda form when the body actually adds work (transformation, conditional, multi-call).

## Related

- Lesson: [`method-invocation`](method-invocation.md) — what most lambda bodies actually do.
- Rule: [`prefer-diamond-over-guava-lists`](../../rules/types-and-collections/prefer-diamond-over-guava-lists.md) — modern Java idioms that, like lambdas, replaced earlier ceremony.
