---
id: do-while-discouraged
language: java
applies-to: [do_statement]
severity: info
kind: style
sources:
  - "Java_L Module 05 — Control Flow (WhileLoops)"
  - "Google Java Style Guide §4.8 — Variable declarations (related: prefer the clearer of equivalent forms)"
---

# Prefer `while` over `do-while`

## Bad

```java
public int sumPositive(int[] xs) {
    int total = 0;
    int i = 0;
    do {
        if (xs[i] > 0) total += xs[i];
        i++;
    } while (i < xs.length);
    return total;
}
```

## Good

```java
public int sumPositive(int[] xs) {
    int total = 0;
    for (int x : xs) {
        if (x > 0) total += x;
    }
    return total;
}
```

## Why

`do-while` reverses the natural reading order: you scan the body before learning when it ends, which forces a second pass through the code to understand the loop's shape. In the bad example, the body unconditionally indexes `xs[i]` — fine for non-empty arrays but a guaranteed `ArrayIndexOutOfBoundsException` on `xs.length == 0`, because the body runs once before the condition is checked.

The "at least one iteration" guarantee that motivates `do-while` is rarely worth that cost. Most do-while loops can be rewritten as a plain `while` (with the condition restructured to be true on entry) or as a `for` / for-each (when the iteration count is computable). Both forms put the exit criterion *before* the body, which matches how readers expect loops to read.

`do-while` is still defensible for true "run, then maybe repeat" cases — interactive prompts that always show once, parser loops where the first read seeds the condition. When you use it, leave the reader a brief comment about *why* the first iteration must run unconditionally.
