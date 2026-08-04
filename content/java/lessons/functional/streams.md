---
id: streams-fundamentals
language: java
applies-to: [lambda_expression]
title: "Streams — declarative pipelines over collections"
level: intermediate
sources:
  - "Java_L Module 17 — Streams API"
  - "JLS / JDK Javadoc — java.util.stream"
  - "Effective Java, Item 45 — Use streams judiciously"
  - "Effective Java, Item 46 — Prefer side-effect-free functions in streams"
---

# Streams — declarative pipelines over collections

A `Stream<T>` describes a sequence of operations to apply to a source of elements. Nothing runs until you ask for a result — the *intermediate* operations (`filter`, `map`, `sorted`) build up a recipe; a single *terminal* operation (`collect`, `forEach`, `findFirst`, `count`) consumes the source and produces the answer. That deferred-execution shape is the single most important thing to internalise about streams.

## Syntax

```java
import java.util.List;
import java.util.stream.Collectors;

List<Order> orders = …;

// A typical pipeline.
List<String> activeCustomerNames = orders.stream()
    .filter(Order::isActive)            // intermediate — produces a new Stream
    .map(Order::getCustomerName)        // intermediate — transforms each element
    .distinct()                         // intermediate — drops duplicates
    .sorted()                           // intermediate — orders the elements
    .toList();                          // terminal — collects the result (Java 16+)

// Reducing to a single value.
long total = orders.stream()
    .filter(Order::isPaid)
    .mapToLong(Order::getCents)
    .sum();

// Grouping.
Map<Status, List<Order>> byStatus = orders.stream()
    .collect(Collectors.groupingBy(Order::getStatus));
```

## Key ideas

**Streams are not collections; they're pipelines.** A `Stream` doesn't hold elements — it holds the *plan* for how to process them. You can iterate a `List` ten times; you can only run a stream pipeline once. After the terminal operation fires, the stream is consumed; calling another operation on it throws `IllegalStateException`.

**Intermediate operations are lazy; terminal operations are eager.** `orders.stream().filter(o -> { print("filtering"); return true; })` does *nothing* without a terminal call — no print, no work. Add `.count()` and the filter runs once per element. The laziness is what lets the JVM fuse operations and skip unnecessary work (e.g., `filter(x -> x > 0).findFirst()` stops at the first match instead of filtering the whole stream).

**Side-effect-free functions only.** *Effective Java*'s Item 46 is firm: the functions you pass to `map`, `filter`, `peek`, `reduce` should compute their result purely from inputs and return values. A lambda that mutates a shared variable (a `+=`, a `list.add(x)`) works in a sequential stream but breaks unpredictably in `parallelStream()` — and even sequentially, a `peek` with side effects can disappear if the JVM optimises it away.

**Prefer `.toList()` over `.collect(Collectors.toList())` (Java 16+).** The `Stream.toList()` terminal returns an unmodifiable `List` and is shorter than the legacy collector form. The collector form still exists for the rare case where you specifically need a `mutable` list (its return is `ArrayList`-backed); for the overwhelming "give me the elements" case, `toList()` is cleaner.

**`Map`-like terminal operations live on `Collectors`.** `Collectors.groupingBy(keyFn)` builds a `Map<K, List<T>>`. `Collectors.toMap(keyFn, valueFn)` builds a `Map<K, V>` (and throws on duplicate keys — use the three-argument form to specify a merge function). `Collectors.partitioningBy(predicate)` is the special-case `Map<Boolean, List<T>>` for a yes/no split.

**`forEach` is the exit hatch — and the smell.** Reaching for `forEach` to mutate external state is the signal that you're using a stream as a souped-up `for` loop. If the goal is a transformation, return the new value via `map`/`collect`. If the goal is a side effect (writing to a log, sending an email), an enhanced-for loop usually reads better — it makes the side effect visible.

## Related

- Rule: [`prefer-toList-over-collectors-toList`](../../rules/functional/prefer-toList-over-collectors-toList.md) — replaces the verbose collector with the Java 16+ terminal.
- Lesson: [`lambda-expression-fundamentals`](../expressions/lambda-expression.md) — the syntax used at every pipeline stage.
- Lesson: [`method-reference-fundamentals`](../expressions/method-reference.md) — the compact form of one-call lambdas in pipelines.
- Lesson: [`collections-fundamentals`](../types-and-collections/collections.md) — the sources and targets of most stream pipelines.
