---
id: prefer-diamond-over-guava-lists
language: java
applies-to: [method_invocation]
match:
  call_target:
    in:
      - Lists.newArrayList
      - Lists.newLinkedList
      - Lists.newCopyOnWriteArrayList
      - Maps.newHashMap
      - Maps.newLinkedHashMap
      - Maps.newConcurrentMap
      - Sets.newHashSet
      - Sets.newLinkedHashSet
severity: info
kind: style
sources:
  - "Guava — `com.google.common.collect.Lists` Javadoc (`newArrayList` notes the diamond-operator alternative)"
  - "JLS §15.9 — Class Instance Creation Expressions (type inference, JEP 101)"
---

# Prefer `new ArrayList<>(…)` to Guava's `Lists.newArrayList(…)`

## Bad

```java
import com.google.common.collect.Lists;
import com.google.common.collect.Maps;
import com.google.common.collect.Sets;

public class Catalog {
    private final List<String> codes = Lists.newArrayList();
    private final Map<String, Product> byId = Maps.newHashMap();
    private final Set<String> tags = Sets.newHashSet("active", "promo");
}
```

## Good

```java
public class Catalog {
    private final List<String> codes = new ArrayList<>();
    private final Map<String, Product> byId = new HashMap<>();
    private final Set<String> tags = new HashSet<>(List.of("active", "promo"));
}
```

## Why

Guava's `Lists.newArrayList`, `Maps.newHashMap`, and `Sets.newHashSet` factories were a workaround for pre-Java 7 syntax: before the diamond operator (`<>`), `new ArrayList<String>()` repeated the type arguments on both sides of the assignment, and the factory let you write `Lists.<String>newArrayList()` or rely on inference. Java 7's diamond operator removed that pain — `new ArrayList<>()` infers the type from the assignment target without any helper.

Using the Guava factories today adds a `com.google.common.collect` dependency to a file that doesn't otherwise need it, hides which concrete class is being instantiated behind a static method, and reads as legacy code to anyone who has stopped seeing Guava as the default toolbox. There is no semantic difference — the factory returns exactly the same `ArrayList` / `HashMap` / `HashSet` you'd construct directly.

Keep Guava's collection helpers where they earn their dependency: `ImmutableList.of`, `Multimap`, `BiMap`, `Iterables.partition`, `Streams.zip` — the ones that don't have a one-line JDK equivalent.
