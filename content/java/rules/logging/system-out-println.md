---
id: system-out-println
language: java
applies-to: [method_invocation]
match:
  call_target:
    in: [System.out.println, System.out.print, System.out.printf, System.err.println, System.err.print, System.err.printf]
severity: info
kind: style
sources:
  - "SLF4J — Why use a logging facade"
---

# Don't `System.out.println` in shipped code

## Bad

```java
public Order place(Cart cart) {
    System.out.println("placing order for " + cart);
    return repository.save(new Order(cart));
}
```

## Good

```java
private static final Logger LOG = LoggerFactory.getLogger(OrderService.class);

public Order place(Cart cart) {
    LOG.info("placing order for cart={}", cart);
    return repository.save(new Order(cart));
}
```

## Why

`System.out` and `System.err` are unconfigurable global sinks. They can't be silenced, redirected, rate-limited, tagged with a level, or correlated with the request id without rewriting every call site. In a container, both streams typically land in the same stream the orchestrator polls — production logs end up a mix of intended output and forgotten debug prints.

A logger (`java.util.logging`, SLF4J, Log4j2) gives you the level, the structured fields, the configurable output, and the ability to turn whole packages off without touching code. Use `println` for one-off `main()` demos and quick local debugging — and grep for it before opening the PR.
