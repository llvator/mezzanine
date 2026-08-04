---
id: public-mutable-static-field
language: java
applies-to: [field_declaration]
match:
  visibility: public
  is_static: "true"
  is_final: "false"
severity: warning
kind: structure
sources:
  - "Effective Java, Item 15 — Minimize the accessibility of classes and members"
  - "Effective Java, Item 17 — Minimize mutability"
---

# `public static` non-`final` fields are global mutable state

## Bad

```java
public class Config {
    public static int maxRetries = 3;
}

// Anywhere in the codebase:
Config.maxRetries = -1;     // now every other reader of this field disagrees
```

## Good

```java
public class Config {
    public static final int DEFAULT_MAX_RETRIES = 3;
    private int maxRetries = DEFAULT_MAX_RETRIES;

    public int getMaxRetries() { return maxRetries; }
    public void setMaxRetries(int value) { this.maxRetries = value; }
}
```

## Why

A `public static` non-final field is reachable from anywhere and writable from anywhere. There is no audit trail, no thread-safety guarantee, no validation hook, and no way to introduce one later without a binary-incompatible refactor of every caller.

If the value is a constant, mark it `final` and SCREAMING_SNAKE_CASE it. If the value genuinely needs to mutate, make the field `private` and put a setter on it — even an unconditional setter is better than a public field, because the setter is the seam where validation, logging, or a thread-safety guard can later live without breaking the call sites.

The same advice applies to `public static final` fields holding mutable collections (`public static final List<String> NAMES = new ArrayList<>();` — `final` only freezes the *reference*, not the *contents*). Use `List.of(...)` / `Collections.unmodifiableList(...)` for those.
