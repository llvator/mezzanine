---
id: text-block-fundamentals
language: java
applies-to: [text_block]
title: "Text blocks — multi-line string literals"
level: beginner
sources:
  - "Java_L Module 20 — Modern Features (TextBlocks)"
  - "JEP 378 — Text Blocks (Java 15)"
  - "JLS §3.10.6 — Text Blocks"
---

# Text blocks — multi-line string literals

A text block (`"""..."""`) is a Java 15+ literal for multi-line strings. The compiler figures out the common leading indentation and strips it, so embedded JSON, SQL, HTML, and similar payloads can sit naturally indented inside Java code without `\n`s and `+`s.

## Syntax

```java
// Single line of payload — same as a regular "...".
String greeting = """
        Hello, world!""";        // → "Hello, world!"

// Multi-line — the closing delimiter sets the indentation baseline.
String json = """
        {
            "id": 42,
            "status": "active"
        }
        """;
// Equivalent to:
//   "{\n    \"id\": 42,\n    \"status\": \"active\"\n}\n"

// Inline expressions still need concatenation or formatted().
String query = """
        SELECT *
        FROM customers
        WHERE id = %d
        """.formatted(id);
```

## Key ideas

**The closing delimiter's indentation determines the strip column.** The compiler measures the leading whitespace of every non-blank content line *and* the closing `"""` line, takes the minimum, and removes that prefix from every line. Putting the closing `"""` flush-left preserves all indentation; aligning it with the content lines strips them to column zero. This is what lets you indent the payload nicely without that indentation leaking into the string value.

**The opening `"""` must be followed by a line terminator.** `"""Hello"""` is not a text block — it's a syntax error. The first content character is always on the line *after* the opening delimiter, which is why a one-line text block typically reads as `"""\n    payload"""`.

**Escapes still work, plus two text-block-specific ones.** `\n`, `\t`, `\"`, `\\` behave as in regular strings. `\<line-terminator>` (Java 15+) is a line continuation that suppresses the implicit newline at the end of a line in the source. `\s` is a literal space that survives the trailing-whitespace trim the compiler applies to each line — useful when you genuinely need trailing spaces in your output.

**Use them for embedded payloads, not as a general string idiom.** Text blocks shine for JSON, SQL, HTML, regex, multi-line error messages, configuration snippets — anything where the *structure* of the string benefits from indentation. For one-line strings, a regular `"..."` is shorter and more familiar. For strings assembled from variables, `formatted(...)` or `String.format(...)` keeps the structure of the text block intact.

**No string interpolation — by design (as of Java 21).** Java doesn't have `${var}`-style interpolation; the language design has held the line that text blocks are literals and string composition is a separate concern. Use `formatted(...)` or `String.format(...)` for variable substitution; expect changes only if the long-discussed string templates JEP eventually ships.

## Related

- Lesson: [`declared-type-fundamentals`](../types-and-collections/declared-type.md) — text blocks have type `String`; everything String-related still applies.
