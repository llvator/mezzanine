---
id: static-import
language: java
applies-to: [import_declaration]
match:
  is_static: "true"
severity: info
kind: style
sources:
  - "Java_L coding guidelines — import.md (`avoid static imports`)"
  - "Effective Java, Item 22 — Use interfaces only to define types (related: don't pollute namespace)"
---

# Avoid `import static`

## Bad

```java
import static java.lang.Math.PI;
import static java.lang.Math.cos;
import static java.util.Collections.emptyList;

double area(double r) {
    return PI * r * r;
}

List<String> empty() {
    return emptyList();
}
```

## Good

```java
import java.util.Collections;

double area(double r) {
    return Math.PI * r * r;
}

List<String> empty() {
    return Collections.emptyList();
}
```

## Why

Static imports trade two characters of typing (the `Math.` prefix) for two readability costs that hit every future reader of the file: the call site no longer says where the symbol comes from, and the symbol can shadow or collide with locals of the same name.

The narrow legitimate uses — JUnit assertions (`assertEquals(…)`), Mockito (`when(…)`), DSLs explicitly designed to read better without qualification — are the exception, not the default. If you reach for `import static` outside a test or a DSL, prefer the explicit qualifier.
