---
id: legacy-thread-safe-collections
language: java
applies-to: [declared_type]
match:
  name:
    in: [Vector, Hashtable, Stack]
severity: warning
kind: gotcha
sources:
  - "Java Concurrency in Practice, §5.1 — Synchronized collections"
  - "java.util.Vector / Hashtable Javadoc — 'as of the Java 2 platform v1.2'"
---

# `Vector` / `Hashtable` / `Stack` are pre-Collections-framework

## Bad

```java
import java.util.Vector;
import java.util.Hashtable;
import java.util.Stack;

public class Cache {
    private final Vector<String> recent = new Vector<>();
    private final Hashtable<String, byte[]> store = new Hashtable<>();
    private final Stack<String> lifo = new Stack<>();
}
```

## Good

```java
import java.util.ArrayList;
import java.util.ArrayDeque;
import java.util.Collections;
import java.util.Deque;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

public class Cache {
    // Single-threaded → plain ArrayList/HashMap:
    private final List<String> recent = new ArrayList<>();
    private final Map<String, byte[]> store = new HashMap<>();

    // Multi-threaded → ConcurrentHashMap (much better than Hashtable):
    private final Map<String, byte[]> sharedStore = new ConcurrentHashMap<>();

    // LIFO → ArrayDeque, not Stack:
    private final Deque<String> lifo = new ArrayDeque<>();
}
```

## Why

`Vector`, `Hashtable`, and `Stack` predate the Collections framework (Java 1.0/1.1). They achieve thread-safety by `synchronized`-ing *every* method, which:

- Imposes a serialization cost on single-threaded callers (the common case), since the JVM can't elide a lock without proof of single-threaded access.
- Doesn't actually give you compound-atomic operations — `if (!v.contains(x)) v.add(x)` still races, and you have to lock externally just like an `ArrayList`.
- Doesn't scale: every reader contends with every writer on the same monitor.

`ConcurrentHashMap` solves all three for the concurrent case: per-bucket locking, atomic compound operations (`computeIfAbsent`, `putIfAbsent`), and read-mostly workloads scale linearly with cores. `ArrayDeque` is faster than `Stack` even single-threaded because `Stack` inherits its `Vector` synchronization.
