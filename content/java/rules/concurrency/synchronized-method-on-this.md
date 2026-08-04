---
id: synchronized-method-on-this
language: java
applies-to: [method_declaration]
match:
  is_synchronized: "true"
severity: warning
kind: gotcha
sources:
  - "Effective Java, Item 82 — Document thread safety"
  - "Java Concurrency in Practice, §4.2.1 — The Java monitor pattern"
---

# A `synchronized` method locks on `this` — same footgun as `synchronized(this)`

## Bad

```java
public class Account {
    private int balance;

    public synchronized void withdraw(int amount) {
        balance -= amount;
    }
}
```

## Good

```java
public class Account {
    private final Object lock = new Object();
    private int balance;

    public void withdraw(int amount) {
        synchronized (lock) {
            balance -= amount;
        }
    }
}
```

## Why

A `synchronized` instance method is identical to wrapping the body in `synchronized (this) { … }`. The lock is publicly addressable — any code holding a reference to the `Account` can `synchronized (account) { … }` and contend for, or deadlock against, the same monitor your class relies on.

Static `synchronized` methods are the same problem with `MyClass.class` as the lock, which is even more public. A `private final Object lock` is the canonical fix: the locking discipline becomes a class invariant rather than a contract you have to defend against external callers.
