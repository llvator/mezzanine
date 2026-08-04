---
id: default-package
language: java
applies-to: [class_declaration]
match:
  in_default_package: "true"
severity: warning
kind: structure
sources:
  - "Java_L coding guidelines — import.md (`Declare a package for your class. We don't want the class to fall into the default package`)"
  - "JLS §7.4.2 — Unnamed Packages"
---

# Always declare a `package`

## Bad

```java
// File: Service.java   ← no package statement at all
public class Service {
    // ...
}
```

## Good

```java
// File: com/example/orders/Service.java
package com.example.orders;

public class Service {
    // ...
}
```

## Why

A class without `package` lives in the JVM's *unnamed package*. Code in named packages cannot import from the unnamed package — there is no syntax for it — so the class becomes effectively unusable outside the same compilation unit. Reflection-driven frameworks (Spring, JPA, Jackson) also routinely fail on unnamed-package classes because their classloader assumptions break down.

The fix is mechanical: pick a reverse-DNS name (`com.<org>.<project>.<area>`) and put the file under a matching directory. Even one-off experiments are easier to grow when the package boundary is established from line one.
