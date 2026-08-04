---
id: legacy-date-time
language: java
applies-to: [declared_type]
match:
  name:
    in: [Date, Calendar, GregorianCalendar, SimpleDateFormat, TimeZone]
severity: warning
kind: gotcha
sources:
  - "Effective Java, Item 36 — Use enum sets and date/time API instead of int constants and legacy classes (paraphrased)"
  - "JSR-310 — Java SE 8 Date and Time"
---

# Use `java.time`, not `java.util.Date` / `Calendar`

## Bad

```java
import java.util.Date;
import java.util.Calendar;
import java.text.SimpleDateFormat;

public class Schedule {
    private Date created = new Date();   // mutable; any caller can mutate it

    public String formatted() {
        SimpleDateFormat fmt = new SimpleDateFormat("yyyy-MM-dd");   // not thread-safe
        return fmt.format(created);
    }

    public boolean inJanuary() {
        Calendar c = Calendar.getInstance();
        c.setTime(created);
        return c.get(Calendar.MONTH) == 0;    // months are 0-indexed; January is 0
    }
}
```

## Good

```java
import java.time.Instant;
import java.time.LocalDate;
import java.time.Month;
import java.time.format.DateTimeFormatter;

public class Schedule {
    private final Instant created = Instant.now();   // immutable

    public String formatted() {
        return DateTimeFormatter.ISO_LOCAL_DATE.format(
            LocalDate.ofInstant(created, ZoneId.systemDefault()));
    }

    public boolean inJanuary() {
        return LocalDate.ofInstant(created, ZoneId.systemDefault())
            .getMonth() == Month.JANUARY;
    }
}
```

## Why

`java.util.Date`, `Calendar`, and `SimpleDateFormat` predate the language understanding what a value type should look like. They are mutable (so every getter must defensively copy), not thread-safe (so `SimpleDateFormat` *cannot be shared across threads* and several production outages have traced back to that), and `Calendar.MONTH` is zero-indexed (so `January == 0`, which has bitten everyone reading the legacy code at least once).

`java.time.*` (JSR-310, shipped in Java 8) is the modern replacement: immutable, thread-safe, sensibly named, with explicit modelling of `Instant` vs `LocalDateTime` vs `ZonedDateTime` so timezone bugs become type errors rather than silent corruption. There is no reason to write new code with the legacy types unless you're constrained by an API contract (JDBC's `java.sql.Date`, certain XML libraries).
