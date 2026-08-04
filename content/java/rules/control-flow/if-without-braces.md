---
id: if-without-braces
language: java
applies-to: [if_statement]
match:
  consequence_is_block: "false"
severity: warning
kind: style
sources:
  - "Google Java Style Guide §4.1.1 — Use of optional braces"
  - "Effective Java, Item 67 (paraphrased) — Keep control flow unambiguous"
  - "Java_L Module 05 — Control Flow (IfElse, common pitfalls)"
---

# Always brace `if` bodies — even one-liners

## Bad

```java
public int divide(int n, int d) {
    if (d == 0)
        throw new ArithmeticException("divide by zero");
    return n / d;
}

// Six months later, someone adds a log line:
public int divide(int n, int d) {
    if (d == 0)
        log.error("bad divide");
        throw new ArithmeticException("divide by zero");   // now ALWAYS runs
    return n / d;                                          // now unreachable
}
```

## Good

```java
public int divide(int n, int d) {
    if (d == 0) {
        throw new ArithmeticException("divide by zero");
    }
    return n / d;
}
```

## Why

Java's grammar lets the body of an `if` be a single statement *or* a braced block. The brace-less form looks fine until someone adds a second statement under the same indentation — at which point indentation lies about what the compiler sees. Java only honours the *first* statement as the body; everything underneath runs unconditionally. This is the same shape as the [Apple `goto fail;` bug](https://www.imperialviolet.org/2014/02/22/applebug.html) — semantically devastating, visually invisible.

The fix is one pair of braces. Modern formatters (`google-java-format`, `spotless`) add them automatically, and every mainstream style guide — Google, Sun, Oracle, Apache Commons — mandates them. Treat the brace-less form as a non-feature: never worth the byte saved.

The same logic applies to `else`, `for`, `while`, and `do-while` bodies. This rule fires only on the `if`'s then-branch, but the same discipline should hold across all control-flow bodies.
