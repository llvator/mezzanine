---
id: synchronized-on-this
language: java
applies-to: [synchronized_statement]
match:
  lock_expr_kind: this
severity: warning
kind: gotcha
sources:
  - "Effective Java, Item 82 — Document thread safety"
  - "Java Concurrency in Practice, §4.2.1 — The Java monitor pattern"
---

# Don't synchronize on `this`

## Bad

```java
public class Account {
    private int balance;

    public void withdraw(int amount) {
        synchronized (this) {
            balance -= amount;
        }
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

The intrinsic lock of an object is *public*. Any code that holds a reference to your `Account` can write `synchronized (account) { … }` and contend for — or deadlock against — the same lock you use internally. A private final lock object is not reachable from outside the class, so the locking discipline stays a class invariant rather than a contract you have to defend.

The same argument applies to `synchronized` *methods*: they implicitly lock on `this` (instance methods) or on the `Class` object (static methods). Both are publicly addressable. Prefer an explicit private lock unless the class is documented as a thread-safe monitor and the public lock is part of the contract.
