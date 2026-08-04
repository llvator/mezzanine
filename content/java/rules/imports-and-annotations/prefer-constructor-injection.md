---
id: prefer-constructor-injection
language: java
applies-to: [annotation]
match:
  name:
    in: [Resource, Autowired, Inject]
severity: warning
kind: structure
sources:
  - "Spring Framework Reference — \"Constructor-based or setter-based DI?\" (constructor injection is recommended)"
  - "Effective Java, Item 5 — Prefer dependency injection to hardwiring resources"
  - "Oliver Drotbohm, \"Field injection is not recommended\" — Spring team guidance"
---

# Prefer constructor injection to `@Resource` / `@Autowired` / `@Inject` on fields

## Bad

```java
import javax.annotation.Resource;

public class OrderService {

    @Resource
    private OrderRepository repository;

    @Resource
    private PaymentGateway gateway;

    public Receipt place(Order order) {
        Payment p = gateway.charge(order.total());
        return repository.save(new Receipt(order, p));
    }
}
```

## Good

```java
public class OrderService {

    private final OrderRepository repository;
    private final PaymentGateway gateway;

    public OrderService(OrderRepository repository, PaymentGateway gateway) {
        this.repository = repository;
        this.gateway = gateway;
    }

    public Receipt place(Order order) {
        Payment p = gateway.charge(order.total());
        return repository.save(new Receipt(order, p));
    }
}
```

## Why

Field injection hides a class's dependencies from its own constructor. Reading `OrderService`, you cannot tell — without scrolling the body and noticing the `@Resource` lines — what this class needs to function. The compiler cannot help you either: `new OrderService()` succeeds even when `repository` is null, and the `NullPointerException` only surfaces the first time `place()` runs in an environment that didn't wire the field.

Constructor injection inverts each of those costs. The dependency list is the constructor signature, which is the first thing every reader sees and every test must satisfy. Fields become `final`, so the compiler enforces "assigned exactly once" and the object is safely publishable across threads. The class is now instantiable in a plain `new …(…)` test without a framework runtime, reflection, or a mocking trick — the friction the field-injection variant has every time a unit test needs a fake collaborator.

The same critique applies more weakly to setter injection (`@Resource` on a setter), which at least keeps the dependency name on the public API but still leaves the field non-`final` and the object briefly invalid between construction and the setter call. Reserve setter or field injection for the narrow case of a genuinely optional collaborator that can be re-set at runtime — circular dependencies between Spring beans, lazy initialisers, hot-swappable strategies. Everything else should take its collaborators as constructor arguments.
