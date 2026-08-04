---
id: checked-exception-over-use
language: java
applies-to: [method_declaration]
match:
  throws_types:
    present: true
severity: info
kind: structure
sources:
  - "Effective Java, Item 71 — Avoid unnecessary use of checked exceptions"
  - "Effective Java, Item 72 — Favor the use of standard exceptions"
---

# Reconsider before adding a `throws` clause

## Bad

```java
public int parseLine(String line) throws InvalidLineException {
    if (line.isEmpty()) {
        throw new InvalidLineException("empty line");
    }
    return Integer.parseInt(line);
}
```

## Good

```java
// Caller-recoverable case → return an Optional or a result type:
public Optional<Integer> parseLine(String line) {
    if (line.isEmpty()) return Optional.empty();
    try {
        return Optional.of(Integer.parseInt(line));
    } catch (NumberFormatException e) {
        return Optional.empty();
    }
}

// Genuinely exceptional → throw unchecked:
public int parseLine(String line) {
    if (line.isEmpty()) {
        throw new IllegalArgumentException("line must not be empty");
    }
    return Integer.parseInt(line);
}
```

## Why

A `throws` clause is part of the method's *type signature* — adding or removing one is a binary-incompatible change for every caller. Checked exceptions also tend to climb the call stack: the moment you can't handle one at the call site, every method between you and the handler has to declare it (or wrap it in a runtime exception, which loses information).

This rule is intentionally a nudge, not a verdict. `IOException`, `SQLException`, `InterruptedException` and a handful of others have legitimate boundary-of-system reasons to exist. Use the rule to ask: *can this caller actually recover from this exception?* If the answer is "no, it's always programmer error" — make it unchecked. If the answer is "yes, but the recovery is empty/`Optional.empty`/sentinel value" — make it a return-typed result. Reserve checked exceptions for the cases where the caller really has a useful recovery path.
