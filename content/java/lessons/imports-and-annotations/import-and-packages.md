---
id: import-and-packages
language: java
applies-to: [import_declaration]
title: Imports — bringing other packages into scope
level: beginner
sources:
  - "Java_L Module 01 — Getting Started (program structure, packages)"
  - "Java_L coding_guidelines/import.md"
  - "JLS §7.5 — Import Declarations"
---

# Imports — bringing other packages into scope

Java organises code into *packages* (directory-shaped namespaces, written `com.example.orders`). To use a type that lives in a different package than the current file, you import it.

## Syntax

```java
package com.example.orders;            // this file's package

import java.util.List;                  // a single type
import java.util.ArrayList;             // another single type
import java.util.*;                     // wildcard (use sparingly)
import static java.lang.Math.PI;        // a static member of another type

public class OrderService {
    private final List<String> ids = new ArrayList<>();
}
```

## Key ideas

**Same-package types are auto-imported.** Anything in `com.example.orders` is visible to any other file declaring `package com.example.orders;` — no `import` needed. Same goes for `java.lang.*` (so `String`, `Object`, `Math` work without an import line).

**One-type imports beat wildcards.** `import java.util.*` pulls in every public type from `java.util`. That looks tidy but has two costs: when two packages declare types with the same name (`java.util.Date` vs `java.sql.Date`, `java.awt.List` vs `java.util.List`) the resolution becomes ambiguous, and the import line stops being a free record of which dependencies the file uses.

**Static imports import members, not types.** `import static java.lang.Math.PI;` lets you write `PI` instead of `Math.PI`. Useful in tests (`assertEquals(…)`, Mockito's `when(…)`) and DSLs; in production code it usually makes call sites less clear about where the symbol comes from.

**Declare a package, even for small files.** A file with no `package` declaration lives in the unnamed default package. Named-package code cannot import from it, frameworks that scan packages (Spring, JPA) routinely fail on it, and the boundary between "internal experiment" and "real code" never gets drawn.

## Related

- Rule: [`wildcard-import`](../../rules/imports-and-annotations/wildcard-import.md) — flags `import java.util.*`.
- Rule: [`static-import`](../../rules/imports-and-annotations/static-import.md) — when `import static` clutters the call site.
- Rule: [`default-package`](../../rules/oop/default-package.md) — why every file needs a `package`.
