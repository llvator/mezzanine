# Java Educator — Authoring Style

_Synthesized from the 11 hand-authored exemplars in [`rules/`](rules/). Every
observation below is backed by at least two rule files; the cited `id`s are
what you should read to feel the pattern before writing a new rule._

This guide is **descriptive**, not prescriptive. If you have a good reason to
deviate, deviate — and please update this file when you do, so future authors
(human or LLM) see the new shape as valid.

## Voice

- **Declarative first, imperative second.** Bodies open by stating the
  mechanism (*what is actually happening at the JVM level*) and only then
  prescribe ("Use X / Prefer Y / Reserve Z for…").
  → see [`boxed-equality`](rules/expressions/boxed-equality.md),
  [`raw-types-warning`](rules/types-and-collections/raw-types-warning.md).
- **Third-person, no "we".** None of the 11 rules use "we" or "I"; "you"
  appears only inside imperative instructions (*Use `Objects.equals(a, b)` if…*).
  → see [`finalize-deprecated`](rules/oop/finalize-deprecated.md),
  [`public-mutable-static-field`](rules/oop/public-mutable-static-field.md).
- **Name the concrete consequence before the abstract reason.** Bodies lead
  with what actually breaks ("doubles allocation per iteration", "the OS
  handle stays open under load") and only then explain the why
  ("`String` is immutable", "finalizers run on an unspecified thread").
  → see [`finalize-deprecated`](rules/oop/finalize-deprecated.md),
  [`system-out-println`](rules/logging/system-out-println.md).

## Length

- **`## Bad` and `## Good`: 5–15 lines each.** Long enough to show the full
  context, short enough to read in one glance. Both blocks compile in
  isolation — no `// ... rest of class` ellipses.
  → see [`synchronized-on-this`](rules/concurrency/synchronized-on-this.md),
  [`equals-without-hashcode`](rules/oop/equals-without-hashcode.md).
- **`## Why`: 1–3 paragraphs.** One paragraph if the issue is straightforward
  (style); two or three when the mechanism is non-obvious or when there are
  caveats worth surfacing.
  → see [`arrays-aslist-mutability`](rules/types-and-collections/arrays-aslist-mutability.md) (one
  long paragraph, one short follow-up),
  [`checked-exception-over-use`](rules/exceptions/checked-exception-over-use.md) (three
  paragraphs because the rule is intentionally a nudge, not a verdict).

## Example shape

- **Both blocks model the same scenario, parallel structure.** The reader's
  eye should bounce between `## Bad` and `## Good` and spot exactly which
  line moved — not parse two unrelated programs.
  → see [`synchronized-on-this`](rules/concurrency/synchronized-on-this.md) (the field
  declaration is the only line difference),
  [`boxed-equality`](rules/expressions/boxed-equality.md) (`==` → `.equals` is the only
  edit).
- **Use full class declarations, not bare snippets.** Helps the reader place
  the code in context and verifies the example compiles.
  → see [`public-mutable-static-field`](rules/oop/public-mutable-static-field.md),
  [`try-with-resources-opportunity`](rules/exceptions/try-with-resources-opportunity.md).
- **Show multiple "good" forms when more than one is right.** Some rules
  have a primary fix plus an alternative that's better in a different
  shape.
  → see [`arrays-aslist-mutability`](rules/types-and-collections/arrays-aslist-mutability.md) (both
  `new ArrayList<>(...)` and `List.of(...)` are shown),
  [`checked-exception-over-use`](rules/exceptions/checked-exception-over-use.md)
  (`Optional` return *and* an unchecked-throw alternative).

## Severity

The corpus uses all three levels. The pattern:

- **`error`** — the program is wrong, not just suspect. Data corruption,
  contract violation, definite undefined behaviour. **Used for 1 rule out
  of 11.** → [`equals-without-hashcode`](rules/oop/equals-without-hashcode.md)
  (silently breaks `HashMap`/`HashSet`).
- **`warning`** — a real bug or strong smell waiting for the wrong input
  to detonate. Most footgun rules sit here. **Used for 6 rules.**
  → [`synchronized-on-this`](rules/concurrency/synchronized-on-this.md) (deadlock under
  contention), [`boxed-equality`](rules/expressions/boxed-equality.md) (intermittent
  past the `Integer` cache boundary).
- **`info`** — style or design reminder; the code may be fine, the rule is
  here to prompt a reconsideration. **Used for 3 rules.**
  → [`checked-exception-over-use`](rules/exceptions/checked-exception-over-use.md)
  (intentionally a nudge), [`system-out-println`](rules/logging/system-out-println.md)
  (style, no semantic issue).

Decision heuristic: if a reasonable Java engineer would say "that's not
broken, it's a choice" — it's `info`. If they'd say "that's a smell" —
`warning`. If they'd say "that's a bug" — `error`.

## Kind

- **`gotcha`** — language semantics will surprise the reader. The bad
  example *looks* like it should work. **6 rules.**
  → [`synchronized-on-this`](rules/concurrency/synchronized-on-this.md),
  [`arrays-aslist-mutability`](rules/types-and-collections/arrays-aslist-mutability.md).
- **`structure`** — design / code-shape decision affecting maintainability
  or correctness at a higher level than language semantics. **4 rules.**
  → [`equals-without-hashcode`](rules/oop/equals-without-hashcode.md),
  [`public-mutable-static-field`](rules/oop/public-mutable-static-field.md).
- **`style`** — convention or preference with no semantic surprise; the
  code works but reads or maintains worse. **1 rule.**
  → [`system-out-println`](rules/logging/system-out-println.md).

`naming` is in the schema but unused in the corpus so far. Adding the first
naming rule is a chance to refine the bucket boundary.

## `match:` vs no `match:`

- **The current corpus is all-Specific.** Every rule has a `match:`
  predicate; nothing fires General-only. **All 11 rules use `match:`.**
- This is a gap, not a prescription. The hover UI partitions matched rules
  into "Specific" (prominent) and unmatched-attached rules into "General"
  (secondary). A General rule would be a fundamentals-of-construct teaching
  card that fires every time the user hovers the construct, even on
  correctly-written code — useful for onboarding, not for catching bugs.
  Worth writing one when the corpus has a clear candidate.

## Source citation

- **`Source, locator — topic`.** Page references where possible.
  → [`finalize-deprecated`](rules/oop/finalize-deprecated.md) cites
  *"Effective Java, Item 8 — Avoid finalizers and cleaners"*;
  [`synchronized-on-this`](rules/concurrency/synchronized-on-this.md) cites
  *"Java Concurrency in Practice, §4.2.1 — The Java monitor pattern"*.
- **One to three sources per rule.** More than three suggests the rule is
  too broad — consider splitting it.
- **Never paste verbatim from a copyrighted source.** The Educator content
  is your-words-via-LLM, citing the source for attribution. Body text is
  always paraphrased.

## What this guide is not

- Not a checklist a rule must pass to be merged. Use it as a baseline; the
  exemplars themselves are the contract.
- Not an enforcement document. There is no linter that validates rule
  bodies against this style guide (and there shouldn't be — the value of
  the corpus is in the human judgment behind each rule, not in mechanical
  conformity).
- Not stable. If a future rule sets a different style and we keep it, this
  guide should grow to describe both — not pick one and reject the other.

---

## Drafting prompt template

Use this when feeding an LLM the corpus to draft new rules. Replace the
bracketed parts before sending.

```
You are drafting a new educator rule for the Nao project. Read the schema
contract, the style guide, and 3 existing rules as exemplars. Produce a
new rule file (frontmatter + markdown body) that follows the same shape.

Schema:  content/java/construct-kinds.md           (paste contents)
Style:   content/java/STYLE.md                     (paste contents)
Exemplar 1: content/java/rules/[NEAREST-RULE-1].md (paste contents)
Exemplar 2: content/java/rules/[NEAREST-RULE-2].md (paste contents)
Exemplar 3: content/java/rules/[NEAREST-RULE-3].md (paste contents)

Source material (your own paraphrase of the rule, not a copyrighted quote):
[YOUR DESCRIPTION OF THE RULE — 2-4 SENTENCES]

Constraints:
- Output a single markdown file content, ready to commit.
- Choose `applies-to`, `match`, `severity`, `kind` from the schema; explain
  any unusual choice in a comment at the top.
- Body has `# Title`, `## Bad`, `## Good`, `## Why`. Bad and Good are
  parallel-structure examples that compile in isolation.
- Cite at least one source in the frontmatter (book + item, JLS section,
  reputable blog) — paraphrase, never quote.
- Voice: declarative first, imperative second; third-person; concrete
  consequence before abstract reason.
```

Pick the three nearest exemplars by construct-kind: `synchronized_*` /
`equality_expression` / `method_invocation` / `declared_type` /
`method_declaration` / `class_declaration` / `field_declaration` /
`try_statement`. If no exemplar matches the new rule's construct, pick
three rules with similar severity/kind instead.
