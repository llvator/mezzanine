---
id: wait-notify-low-level
language: java
applies-to: [method_invocation]
match:
  name:
    in: [wait, notify, notifyAll]
severity: warning
kind: gotcha
sources:
  - "Effective Java, Item 81 — Prefer concurrency utilities to wait and notify"
  - "Java Concurrency in Practice, §14 — Building custom synchronizers"
---

# Don't use `wait` / `notify` — use a concurrency utility

## Bad

```java
private final Object signal = new Object();
private volatile boolean ready = false;

public void waitForReady() throws InterruptedException {
    synchronized (signal) {
        while (!ready) {
            signal.wait();         // easy to forget the while loop → spurious wakeup
        }
    }
}

public void markReady() {
    synchronized (signal) {
        ready = true;
        signal.notifyAll();
    }
}
```

## Good

```java
import java.util.concurrent.CountDownLatch;

private final CountDownLatch ready = new CountDownLatch(1);

public void waitForReady() throws InterruptedException {
    ready.await();
}

public void markReady() {
    ready.countDown();
}
```

## Why

`Object.wait`/`notify`/`notifyAll` are the lowest-level building blocks of Java's monitor pattern, and the rules for using them correctly are unforgiving: the wait must be inside a `while` loop (not `if`) to defend against spurious wakeups, the lock must be held when calling them (any caller forgetting and you get `IllegalMonitorStateException` at runtime), and `notify` vs `notifyAll` is a thread-safety contract decision that's invisible at the call site.

The `java.util.concurrent` package — `CountDownLatch`, `Semaphore`, `BlockingQueue`, `CyclicBarrier`, `Phaser`, the `*Future` types — encapsulates these patterns correctly *once*, so your code expresses the intent (`await`, `acquire`, `take`) instead of re-deriving the monitor mechanics every time. Reach for `wait`/`notify` only if you're implementing a new synchronizer that doesn't already exist in `java.util.concurrent` — which, given the breadth of that package, is rare in application code.

This rule will fire on a few false positives — most notably `Thread.currentThread().wait(...)` is technically distinct from monitor-`wait` but the method name is the same. If the hover shows on a non-monitor call, ignore it.
