---
id: try-with-resources-opportunity
language: java
applies-to: [try_statement]
match:
  has_resource_spec: "false"
severity: info
kind: structure
sources:
  - "Effective Java, Item 9 — Prefer try-with-resources to try-finally"
  - "JLS §14.20.3 — try-with-resources"
---

# If the try opens a `Closeable`, use try-with-resources

## Bad

```java
public String firstLine(Path path) throws IOException {
    BufferedReader r = Files.newBufferedReader(path);
    try {
        return r.readLine();
    } finally {
        r.close();           // easy to forget; if readLine throws, close runs but the exception leaks
    }
}
```

## Good

```java
public String firstLine(Path path) throws IOException {
    try (BufferedReader r = Files.newBufferedReader(path)) {
        return r.readLine();
    }
}
```

## Why

Try-with-resources guarantees that every declared resource is closed in reverse order, even if the body throws. It also handles the subtle case where both the body *and* `close()` throw — the body's exception is preserved as the primary, with the close exception attached via `addSuppressed` rather than lost.

This rule fires on every plain `try { … }` block — most are legitimate (`try { … } catch (X e) { … }` with no resource at all). Read it as a reminder: *if anything you opened in this `try` implements `AutoCloseable`*, move it to the resource specification. The rule cannot tell from syntax alone whether the body opens a `Closeable`; you, looking at the code, can.
