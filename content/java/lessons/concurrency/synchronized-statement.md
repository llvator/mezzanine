---
id: synchronized-statement-fundamentals
language: java
applies-to: [synchronized_statement]
title: "`synchronized` — Java's intrinsic locks"
level: intermediate
sources:
  - "JLS §14.19 — The synchronized Statement"
  - "JLS §17.4 — Memory Model"
  - "Java Concurrency in Practice, §2 — Thread Safety"
  - "Effective Java, Item 78 — Synchronize access to shared mutable data"
  - "Effective Java, Item 82 — Document thread safety"
---

# `synchronized` — Java's intrinsic locks

`synchronized (lock) { body }` does two things at once: it grants the running thread exclusive access to the lock object's *intrinsic monitor* for the duration of the body, and it crosses a memory barrier so writes made under the lock are visible to the next thread that acquires it. Both halves matter — `synchronized` is not just about mutual exclusion.

## Syntax

```java
public class Account {
    private final Object lock = new Object();
    private int balanceCents;

    public void withdraw(int cents) {
        synchronized (lock) {
            balanceCents -= cents;
        }
    }

    public int balanceCents() {
        // The read also synchronises — otherwise a caller could see a stale value
        // from before another thread's withdraw, even after it returned.
        synchronized (lock) {
            return balanceCents;
        }
    }
}
```

## Key ideas

**Every Java object carries an intrinsic monitor.** The block `synchronized (x) { … }` acquires `x`'s monitor, runs the body, then releases the monitor — even if the body throws. Two threads that try to enter blocks synchronised on *the same object* serialize; threads synchronised on different objects do not interact. The monitor is re-entrant: the same thread can re-enter a block it already holds without deadlocking itself.

**Memory visibility comes with the lock, not just mutual exclusion.** Without synchronisation, the JVM is free to cache writes per thread, reorder them, and skip publishing them to other threads forever. Entering a `synchronized` block forces a read-from-shared-memory; leaving forces a write-to-shared-memory. That's why both *writers* and *readers* of shared state must synchronise — a read without the lock can see arbitrarily stale values even when every writer locks correctly.

**The lock object is part of your public contract.** Locking on `this` or on the class object means external code can see and contend for your lock — including locking *first* and deadlocking you. *Effective Java*'s Item 82 spells out the rule: unless the class is explicitly documented as a thread-safe monitor (rare), prefer a `private final Object lock = new Object();` field. The intent is opaque to callers, and the discipline stays inside the class.

**Hold the lock only as long as you must.** Long critical sections turn into contention bottlenecks; calls out to "alien" code (callbacks, untrusted listeners) under the lock can deadlock if those callbacks try to take other locks. Keep the body small — read the inputs, do the mutation, leave.

**Prefer the `java.util.concurrent` tools when they fit.** `AtomicInteger` / `AtomicReference` give lock-free updates for single variables. `ReentrantLock` adds timeouts and interruption (`tryLock(…, timeout, …)`) that intrinsic locks can't express. `ConcurrentHashMap` and `CopyOnWriteArrayList` provide thread-safe collections without externally-visible locks. `synchronized` is still the right answer when you need to keep *multiple* fields consistent — but the bar for reaching for it has moved up.

## Related

- Rule: [`synchronized-on-this`](../../rules/concurrency/synchronized-on-this.md) — flags the publicly-addressable `synchronized (this)` form.
- Rule: [`synchronized-method-on-this`](../../rules/concurrency/synchronized-method-on-this.md) — same hazard via the `synchronized` modifier on methods.
- Rule: [`wait-notify-low-level`](../../rules/concurrency/wait-notify-low-level.md) — the older intrinsic-monitor primitives most code should avoid.
- Rule: [`prefer-concurrent-collections`](../../rules/concurrency/prefer-concurrent-collections.md) — when the shared mutable state is a collection, reach for `ConcurrentHashMap` before `synchronized`.
