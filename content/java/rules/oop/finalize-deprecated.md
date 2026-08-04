---
id: finalize-deprecated
language: java
applies-to: [method_declaration]
match:
  name: finalize
severity: warning
kind: gotcha
sources:
  - "Effective Java, Item 8 — Avoid finalizers and cleaners"
  - "JDK 9: java.lang.Object.finalize() marked @Deprecated"
---

# Don't override `finalize()`

## Bad

```java
public class Connection {
    private final Socket socket;

    @Override
    protected void finalize() throws Throwable {
        socket.close();
        super.finalize();
    }
}
```

## Good

```java
public class Connection implements AutoCloseable {
    private final Socket socket;

    @Override
    public void close() throws IOException {
        socket.close();
    }
}

// Caller:
try (Connection c = new Connection(...)) {
    c.use();
}
```

## Why

`Object.finalize()` was deprecated in JDK 9 and is scheduled for removal. Finalizers run on an unspecified thread at an unspecified time — sometimes never, if the JVM exits first. They are catastrophic for resource cleanup: a `Connection` whose finalizer eventually closes its socket can hold the OS handle open long enough to exhaust the file-descriptor limit under load.

The correct pattern is `AutoCloseable` + `try-with-resources` (or `Cleaner` for a last-line-of-defence fallback, but always with an explicit close path as the primary contract).
