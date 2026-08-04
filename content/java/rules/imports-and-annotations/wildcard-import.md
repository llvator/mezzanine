---
id: wildcard-import
language: java
applies-to: [import_declaration]
match:
  is_wildcard: "true"
severity: warning
kind: style
sources:
  - "Java_L coding guidelines — import.md (`privilege importing only the specific class you need. No wildcard import using *`)"
  - "Google Java Style Guide §3.3.1 — No wildcard imports"
---

# Don't use wildcard imports

## Bad

```java
import java.util.*;
import java.io.*;

public class Service {
    private List<String> names = new ArrayList<>();
}
```

## Good

```java
import java.util.ArrayList;
import java.util.List;
import java.io.IOException;

public class Service {
    private List<String> names = new ArrayList<>();
}
```

## Why

Wildcard imports hide which class is being used. Two packages can declare types with the same name (`java.util.Date` vs `java.sql.Date`, `java.awt.List` vs `java.util.List`), and `import x.*` makes the resolution depend on what other classes exist in the package today — adding a new class upstream can silently change which `Foo` your code resolves to.

Explicit imports are also a free record of dependencies: a reviewer can see at a glance whether a class is meant to depend on `java.sql` without scrolling through the body. Every IDE worth using will add specific imports on demand, so the friction argument doesn't survive a single keystroke.
