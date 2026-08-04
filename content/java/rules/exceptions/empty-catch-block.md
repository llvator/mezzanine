---
id: empty-catch-block
language: java
applies-to: [catch_clause]
match:
  is_empty: "true"
severity: warning
kind: gotcha
sources:
  - "Effective Java, Item 77 — Don't ignore exceptions"
  - "Java_L Module 12 — Exceptions (TryCatch)"
---

# Empty `catch` blocks silently swallow failures

## Bad

```java
public Config loadConfig(Path file) {
    try {
        return parse(Files.readString(file));
    } catch (IOException e) {
    }
    return Config.defaults();
}
```

## Good

```java
public Config loadConfig(Path file) {
    try {
        return parse(Files.readString(file));
    } catch (IOException e) {
        log.warn("could not read config from {}, using defaults", file, e);
        return Config.defaults();
    }
}
```

## Why

An empty `catch` block tells the JVM "I have considered this failure and chosen to do nothing." It logs nothing, alerts nothing, leaves no trace in the stack trace, and lets the surrounding code run on with whatever broken state the failed operation left behind. Anyone debugging "why does this return the default config in prod?" will not find the answer in the logs — the answer was thrown away the moment the catch ran.

If the exception genuinely is expected and recoverable — `NumberFormatException` from a parse that you guarded against — leave a one-line comment in the catch body explaining *why* the failure is safe to ignore. *Effective Java*'s Item 77 makes the comment mandatory: a deliberate decision to ignore a failure is fine, but it must be visible to the next reader.

When the catch body is empty because the recovery is "log and continue," write the log line. When it's empty because the recovery is "fall through to a default value," compute that value here. The thing that empty must never mean is "I don't know what to do, so I'll pretend it didn't happen" — that's how outages start, and they're hardest to investigate because the only evidence is missing.
