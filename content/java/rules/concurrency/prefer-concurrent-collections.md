---
id: prefer-concurrent-collections
language: java
applies-to: [method_invocation]
match:
  call_target:
    in: [Collections.synchronizedMap, Collections.synchronizedList, Collections.synchronizedSet, Collections.synchronizedSortedMap, Collections.synchronizedSortedSet]
severity: info
kind: gotcha
sources:
  - "Effective Java, Item 81 — Prefer concurrency utilities to wait and notify"
  - "Java Concurrency in Practice, §5.2 — Concurrent collections"
---

# Prefer `ConcurrentHashMap` / `CopyOnWriteArrayList` over `Collections.synchronized…`

## Bad

```java
import java.util.Collections;
import java.util.HashMap;
import java.util.Map;

public class Cache {
    private final Map<String, byte[]> store =
        Collections.synchronizedMap(new HashMap<>());

    public byte[] getOrCompute(String key) {
        synchronized (store) {                  // still required for compound ops
            byte[] v = store.get(key);
            if (v == null) {
                v = compute(key);
                store.put(key, v);
            }
            return v;
        }
    }
}
```

## Good

```java
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

public class Cache {
    private final Map<String, byte[]> store = new ConcurrentHashMap<>();

    public byte[] getOrCompute(String key) {
        return store.computeIfAbsent(key, this::compute);   // atomic, no external lock
    }
}
```

## Why

`Collections.synchronizedMap(map)` wraps every individual operation in a `synchronized` block on the wrapper's intrinsic lock. That's enough for single-call thread-safety but *not* for compound operations — `if (!map.containsKey(k)) map.put(k, v)` is a classic race, and the caller has to take an external lock on the wrapper itself to fix it (manually).

`ConcurrentHashMap` ships compound operations as atomic primitives — `putIfAbsent`, `computeIfAbsent`, `compute`, `merge`, `replace(k, oldV, newV)` — that the wrapper map can't offer. It also avoids the global-monitor bottleneck: reads scale linearly with cores, and writers contend only at the bucket level.

The same upgrade exists for `synchronizedList` (→ `CopyOnWriteArrayList` for read-mostly workloads, or `Collections.synchronizedList` only when read/write ratios favour serialized access), `synchronizedSet` (→ `ConcurrentHashMap.newKeySet()` or `CopyOnWriteArraySet`), and `synchronizedSortedMap` (→ `ConcurrentSkipListMap`).

If your data is single-threaded, drop the wrapper entirely — `Collections.synchronizedMap` on a never-shared map is just overhead.
