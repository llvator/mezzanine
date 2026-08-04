---
id: interface-default-method-heavy
language: java
applies-to: [interface_declaration]
match:
  has_default_methods: "true"
severity: info
kind: structure
sources:
  - "Effective Java, Item 21 — Design interfaces for posterity"
  - "Effective Java, Item 20 — Prefer interfaces to abstract classes"
  - "Java_L Module 10 — Interfaces (DefaultMethods)"
---

# Interfaces with `default` methods — is it really an interface?

## Bad

```java
public interface Notifier {
    void send(Notification n);

    default void sendBatch(List<Notification> batch) {
        for (Notification n : batch) send(n);
    }
    default void sendBatchAsync(List<Notification> batch) {
        executor().submit(() -> sendBatch(batch));
    }
    default ExecutorService executor() {
        return ForkJoinPool.commonPool();
    }
    default Notification wrap(String message, String recipient) {
        return new Notification(message, recipient);
    }
}
```

## Good

```java
public interface Notifier {
    void send(Notification n);
}

public abstract class AbstractNotifier implements Notifier {
    public void sendBatch(List<Notification> batch) {
        for (Notification n : batch) send(n);
    }
    public void sendBatchAsync(List<Notification> batch) {
        executor().submit(() -> sendBatch(batch));
    }
    protected ExecutorService executor() {
        return ForkJoinPool.commonPool();
    }
    public Notification wrap(String message, String recipient) {
        return new Notification(message, recipient);
    }
}
```

## Why

`default` methods were added in Java 8 for a specific reason: letting a library add a method to an existing interface without breaking the classes that already implement it. Used that way (one or two utility methods on `List`, `Map`, `Stream`), they're a clean evolution tool.

When `default` methods accumulate, the interface is doing the job of an abstract class but without its tools — no stored fields, no `protected` helpers, no `super` calls. Implementors can't customise the shared logic without reimplementing every default they care about, and the type stops being a contract ("here is what you can do") and becomes a partial implementation ("here is what you get for free").

The cleaner shape is to keep the interface minimal and put the shared logic in an `abstract class` (or an explicit "skeletal implementation" — `AbstractList` is the canonical example). That gives implementors `extends AbstractFoo` for the easy case and `implements Foo` for the bespoke one, without the implementation leaking into the contract.

This rule fires whenever the interface has any `default` methods — calibrate to your codebase. One or two for API evolution is fine; five is the signal to refactor.
