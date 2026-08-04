---
id: equals-without-hashcode
language: java
applies-to: [class_declaration]
match:
  overrides_equals: "true"
  overrides_hashcode: "false"
severity: error
kind: structure
sources:
  - "Effective Java, Item 11 — Always override hashCode when you override equals"
  - "java.lang.Object.equals contract"
---

# Override `hashCode` when you override `equals`

## Bad

```java
public class Point {
    private final int x, y;

    @Override
    public boolean equals(Object o) {
        if (!(o instanceof Point)) return false;
        Point other = (Point) o;
        return x == other.x && y == other.y;
    }
    // hashCode left as Object's identity-based default
}
```

## Good

```java
public class Point {
    private final int x, y;

    @Override
    public boolean equals(Object o) {
        if (!(o instanceof Point)) return false;
        Point other = (Point) o;
        return x == other.x && y == other.y;
    }

    @Override
    public int hashCode() {
        return Objects.hash(x, y);
    }
}
```

## Why

The contract on `Object` requires that `a.equals(b)` implies `a.hashCode() == b.hashCode()`. Override one without the other and `HashMap`, `HashSet`, `Hashtable`, and `ConcurrentHashMap` all break silently: two "equal" points map to different buckets, so `set.add(p1); set.contains(p2)` returns `false` even though `p1.equals(p2)` is `true`.

The inverse (override `hashCode` only) is less dangerous but still suspicious — if the hash is identity-meaningful, `equals` likely should be too. Modern IDEs generate both together; if you wrote one by hand, write the other by hand too.
