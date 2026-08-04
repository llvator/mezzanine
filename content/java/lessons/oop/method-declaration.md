---
id: method-declaration-fundamentals
language: java
applies-to: [method_declaration]
title: Methods — the building blocks of behaviour
level: beginner
sources:
  - "Java_L Module 07 — Methods (MethodBasics, MethodOverloading, PassByValue)"
  - "JLS §8.4 — Method Declarations"
---

# Methods — the building blocks of behaviour

A *method* is a named block of code attached to a class. Other code calls it by name (optionally on an instance), passes in arguments, and receives a return value (or nothing, if the return type is `void`).

## Syntax

```java
accessModifier returnType methodName(ParamType paramName, …) {
    // body
    return value;     // omit when returnType is void
}
```

Each piece carries meaning:

- **Access modifier** (`public` / `protected` / `private` / package-default) — who is allowed to call this method.
- **Return type** — what the method gives back. `void` means "nothing"; any other type (primitive or reference) is the type the caller will receive.
- **Method name** — by convention `camelCase`, verb-shaped (`calculateTotal`, `findById`).
- **Parameters** — typed inputs. Java is statically typed, so each parameter declares its type.
- **Body** — the statements that run when the method is called.

## Key ideas

**Return on every path.** If a method declares a non-`void` return type, the compiler checks that every reachable code path returns a value. You cannot accidentally fall off the end of a non-void method.

**Java is pass-by-value, always.** When you call `foo(x)`, Java copies `x` into the parameter. For primitives this is obvious: the callee can't mutate the caller's variable. For object references, the *reference itself* is copied — the callee can mutate the object's state, but cannot make the caller's variable point at a different object.

**Overloading.** Multiple methods can share a name as long as their *parameter lists* differ. The compiler picks the most-specific matching overload at the call site. Return type alone does not differentiate overloads.

**Varargs.** A trailing `Type... param` parameter accepts zero or more arguments and is received as an array. Only one varargs slot per method, and it must be last.

## Related

- Rule: [`checked-exception-over-use`](../../rules/exceptions/checked-exception-over-use.md) — when methods over-rely on `throws`.
- Rule: [`finalize-deprecated`](../../rules/oop/finalize-deprecated.md) — one specific method name to avoid.
- Rule: [`synchronized-method-on-this`](../../rules/concurrency/synchronized-method-on-this.md) — when the `synchronized` modifier is the wrong tool.
