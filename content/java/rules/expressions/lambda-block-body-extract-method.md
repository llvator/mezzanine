---
id: lambda-block-body-extract-method
language: java
applies-to: [lambda_expression]
match:
  body_kind: block
severity: info
kind: structure
sources:
  - "Effective Java, Item 42 — Prefer lambdas to anonymous classes (with the corollary: don't overgrow a lambda)"
  - "Effective Java, Item 43 — Prefer method references to lambdas"
  - "Java_L Module 16 — Lambdas and Method References"
---

# Block-body lambdas have outgrown the lambda form — extract a method

## Bad

```java
List<Order> processed = orders.stream()
    .map(order -> {
        Order updated = order.withStatus(Status.PROCESSING);
        notifyCustomer(updated);
        auditLog.record(updated);
        return inventoryService.reserve(updated);
    })
    .collect(Collectors.toList());
```

## Good

```java
List<Order> processed = orders.stream()
    .map(this::beginProcessing)
    .collect(Collectors.toList());

private Order beginProcessing(Order order) {
    Order updated = order.withStatus(Status.PROCESSING);
    notifyCustomer(updated);
    auditLog.record(updated);
    return inventoryService.reserve(updated);
}
```

## Why

The lambda form is at its best when the body fits on the same line as `->` and reads as a one-liner expression: `x -> x * 2`, `order -> order.getId()`, `(a, b) -> a.compareTo(b)`. The grammar lets you grow that into a `{ … }` block with multiple statements, an explicit `return`, and arbitrary control flow — but at that point the lambda has stopped being a *value* (the thing you pass to `map`) and become a *piece of logic* that happens to live inline.

Two things go wrong as block-body lambdas grow:

1. **The lambda no longer reads where it lives.** A `.map(…)` call should communicate "for each order, get its processed form." When the lambda body is six lines of logic, the reader has to expand the chain mentally to see the structure of the pipeline. Extracting to a named method (`this::beginProcessing`) makes the *what* visible at the call site and pushes the *how* into a method whose name explains the intent.
2. **You lose the testability of the operation.** A method can be unit-tested directly with mock orders. A lambda buried inside `.stream().map(…)` is only reachable through the surrounding pipeline.

The rule fires on every block-body lambda regardless of length — it's a *hint*, not an error. Short blocks (two or three statements with a single early-`return` for the common shape) are fine to leave; the value is in the prompt, not the auto-extraction. When you do extract, prefer a method reference (`this::name`) over wrapping the call in another lambda — that's *Effective Java*'s Item 43.

Expression-body lambdas (`x -> x + 1`) are not flagged. The rule's signal is specifically the `{ … }` syntax, which the parser exposes as `body_kind: block` on `lambda_expression`.
