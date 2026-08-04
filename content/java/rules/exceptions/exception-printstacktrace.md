---
id: exception-printstacktrace
language: java
applies-to: [method_invocation]
match:
  name: printStackTrace
severity: info
kind: style
sources:
  - "Effective Java, Item 75 — Include failure-capture information in detail messages (related)"
  - "SLF4J — Why a logging facade beats System.err"
---

# Don't `e.printStackTrace()` — log it

## Bad

```java
public Order load(String id) {
    try {
        return repository.findById(id);
    } catch (RepositoryException e) {
        e.printStackTrace();    // goes to System.err, unconfigurable
        return null;
    }
}
```

## Good

```java
private static final Logger LOG = LoggerFactory.getLogger(OrderService.class);

public Order load(String id) {
    try {
        return repository.findById(id);
    } catch (RepositoryException e) {
        LOG.error("failed to load order {}", id, e);   // logger handles the stack trace
        return null;
    }
}
```

## Why

`printStackTrace()` writes directly to `System.err`. That stream cannot be rate-limited, redirected per-request, tagged with a level or a correlation id, or filtered by package — every other concern your logging infrastructure handles, you forfeit. In a container, `System.err` typically lands in the same stream `stdout` does, so production logs become an indistinguishable mix of intended output and forgotten stack traces.

Pass the exception as the *last* argument to the logger (`LOG.error("…", id, e)`) — every common framework (SLF4J, Log4j2, JUL) detects the trailing `Throwable` and renders the stack trace with the level and structured fields. The cost of doing this right is one line of code at the catch site; the cost of `printStackTrace` is paid on every incident going forward.
