//! Java-specific construct-kind extraction.
//!
//! Each entry in [`JAVA_CONSTRUCT_KINDS`] documents what a rule author can
//! `applies-to:` and `match:` against; the [`extract`] function below produces
//! the runtime instances. The catalog and the extractor are pinned together
//! by `java_catalog_documents_every_extracted_kind` in the test module — if
//! a new kind appears in [`extract`] without a corresponding registry entry,
//! CI fails.

use super::catalog::{AttributeSpec, AttributeValues, ConstructKindSpec};
use super::predicate::Attrs;
use tree_sitter::Node;

/// Registry of every construct-kind the Java parser emits. See [`super::catalog`].
pub static JAVA_CONSTRUCT_KINDS: &[ConstructKindSpec] = &[
    ConstructKindSpec {
        kind: "synchronized_statement",
        description: "A `synchronized (expr) { … }` block. The lock expression is unwrapped one layer of parentheses so the inner shape — `this`, an identifier, a field access — is what shows up in attributes.",
        attributes: &[
            AttributeSpec {
                name: "lock_expr_kind",
                description: "Tree-sitter node kind of the lock expression (after peeling one set of parentheses).",
                values: AttributeValues::Enum(&[
                    "this",
                    "identifier",
                    "field_access",
                    "method_invocation",
                    "parenthesized_expression",
                ]),
            },
            AttributeSpec {
                name: "lock_expr_text",
                description: "Raw source text of the lock expression. Useful for rules that match a specific named lock by convention.",
                values: AttributeValues::Text,
            },
        ],
    },
    ConstructKindSpec {
        kind: "method_declaration",
        description: "A method declaration. Carries the method's name so ancestor-context rules can attach (e.g. `equals`/`hashCode` pairing), exposes `is_synchronized` and `is_default` for the modifier-based rules, and emits `throws_types` (only when present) so `checked-exception-over-use` can fire on methods that declare checked exceptions.",
        attributes: &[
            AttributeSpec {
                name: "name",
                description: "Method identifier as written in source.",
                values: AttributeValues::Identifier,
            },
            AttributeSpec {
                name: "is_synchronized",
                description: "`true` when the declaration carries the `synchronized` modifier (implicit lock on `this` for instance methods, on the `Class` for static methods).",
                values: AttributeValues::Boolean,
            },
            AttributeSpec {
                name: "is_default",
                description: "`true` when the declaration carries the `default` modifier — only legal on interface methods. Used by `default-method-in-interface` to discourage growing interfaces via default methods.",
                values: AttributeValues::Boolean,
            },
            AttributeSpec {
                name: "throws_types",
                description: "Comma-separated list of declared throws-types. Only emitted when the method has a `throws` clause — rules can `match: { throws_types: { present: true } }` for any-throws or check the text for specific exception names.",
                values: AttributeValues::Text,
            },
        ],
    },
    ConstructKindSpec {
        kind: "class_declaration",
        description: "A class declaration. Visible as the outermost ancestor when hovering anywhere inside a class body. The `overrides_*` booleans let class-level rules (`equals-without-hashcode`) check the equals/hashCode pairing without re-walking the body themselves, `in_default_package` flags top-level classes whose file has no `package` declaration, and the inheritance attributes (`is_abstract`, `is_final`, `is_sealed`, `extends_type`, `has_abstract_methods`) cover the modifier/superclass shape that rules in the inheritance space need.",
        attributes: &[
            AttributeSpec {
                name: "name",
                description: "Class identifier as written in source.",
                values: AttributeValues::Identifier,
            },
            AttributeSpec {
                name: "overrides_equals",
                description: "`true` when the class declares a method named `equals` (any signature — the heuristic doesn't check for `Object` arg type).",
                values: AttributeValues::Boolean,
            },
            AttributeSpec {
                name: "overrides_hashcode",
                description: "`true` when the class declares a method named `hashCode`.",
                values: AttributeValues::Boolean,
            },
            AttributeSpec {
                name: "in_default_package",
                description: "`true` when this class is declared in a file with no `package …;` line — the JVM's anonymous default package.",
                values: AttributeValues::Boolean,
            },
            AttributeSpec {
                name: "is_abstract",
                description: "`true` when the class is declared `abstract` — it cannot be instantiated directly and may declare abstract methods that subclasses must implement.",
                values: AttributeValues::Boolean,
            },
            AttributeSpec {
                name: "is_final",
                description: "`true` when the class is declared `final` — no subclass may extend it.",
                values: AttributeValues::Boolean,
            },
            AttributeSpec {
                name: "is_sealed",
                description: "`true` when the class is declared `sealed` (Java 17+) — only the types listed in `permits` may extend it.",
                values: AttributeValues::Boolean,
            },
            AttributeSpec {
                name: "extends_type",
                description: "When the class has an `extends` clause: the source text of the superclass type (e.g. `Employee`, `PaymentMethod`, `AbstractList<String>`). Absent when the class extends `Object` implicitly.",
                values: AttributeValues::Text,
            },
            AttributeSpec {
                name: "has_abstract_methods",
                description: "`true` when the class body declares one or more `abstract` methods. Used together with `is_abstract` to catch the `abstract class` with no abstract members anti-pattern.",
                values: AttributeValues::Boolean,
            },
        ],
    },
    ConstructKindSpec {
        kind: "equality_expression",
        description: "A `==` or `!=` comparison (the only `binary_expression` shapes that have a footgun-flavoured reading). Operand types are filled when the operand is a plain identifier referring to a local variable whose declared type the parser can see; cross-file/field-typed operands stay unset.",
        attributes: &[
            AttributeSpec {
                name: "op",
                description: "The operator — `==` or `!=`.",
                values: AttributeValues::Enum(&["==", "!="]),
            },
            AttributeSpec {
                name: "lhs_type",
                description: "Declared type of the left operand when it is a local variable. Absent for fields, method results, literals, and operands the parser can't resolve locally.",
                values: AttributeValues::Identifier,
            },
            AttributeSpec {
                name: "rhs_type",
                description: "Declared type of the right operand under the same conditions as `lhs_type`.",
                values: AttributeValues::Identifier,
            },
        ],
    },
    ConstructKindSpec {
        kind: "method_invocation",
        description: "A method call site. The `call_target` attribute combines the receiver text and method name for convenient `match:` use (`Arrays.asList`, `Collections.singletonList`).",
        attributes: &[
            AttributeSpec {
                name: "name",
                description: "Bare method name (the `name` field of the `method_invocation` node).",
                values: AttributeValues::Identifier,
            },
            AttributeSpec {
                name: "receiver",
                description: "Raw source text of the receiver (the `object` field) when present.",
                values: AttributeValues::Text,
            },
            AttributeSpec {
                name: "call_target",
                description: "Convenience: `<receiver>.<name>` when both exist, else just `<name>`. Targets like `Arrays.asList` are intended to be matched here.",
                values: AttributeValues::Text,
            },
        ],
    },
    ConstructKindSpec {
        kind: "declared_type",
        description: "A type identifier appearing in a declaration position (variable, field, parameter, return type). Used by `raw-types-warning` to flag commonly-generic types (`List`, `Map`, …) used without type arguments. When the source uses a parameterised form, `generic_args` is present; raw uses leave it absent.",
        attributes: &[
            AttributeSpec {
                name: "name",
                description: "Type identifier (e.g. `List`, `String`, `Account`).",
                values: AttributeValues::Identifier,
            },
            AttributeSpec {
                name: "generic_args",
                description: "Raw source text of the type-argument list (`<String>`, `<K, V>`) when present. Absent for raw uses — that's the signal `raw-types-warning` matches on.",
                values: AttributeValues::Text,
            },
        ],
    },
    ConstructKindSpec {
        kind: "field_declaration",
        description: "A field declaration. The visibility/static/final attributes let rules like `public-mutable-static-field` detect leaky global state without scanning bodies.",
        attributes: &[
            AttributeSpec {
                name: "visibility",
                description: "Declared visibility — `public`, `protected`, `private`, or `package` (when no modifier is present).",
                values: AttributeValues::Enum(&["public", "protected", "private", "package"]),
            },
            AttributeSpec {
                name: "is_static",
                description: "`true` when the field is declared `static`.",
                values: AttributeValues::Boolean,
            },
            AttributeSpec {
                name: "is_final",
                description: "`true` when the field is declared `final`.",
                values: AttributeValues::Boolean,
            },
            AttributeSpec {
                name: "type_name",
                description: "Top-level type identifier of the field (the part rules typically inspect — `String`, `Map`, …).",
                values: AttributeValues::Identifier,
            },
        ],
    },
    ConstructKindSpec {
        kind: "try_statement",
        description: "A `try` block. Covers both plain `try` and `try-with-resources` — the `has_resource_spec` attribute distinguishes them so `try-with-resources-opportunity` can teach the pattern without firing on legitimate catch-only blocks.",
        attributes: &[
            AttributeSpec {
                name: "has_resource_spec",
                description: "`true` for `try (Resource r = …) { … }` (try-with-resources), `false` for the plain `try { … }` form.",
                values: AttributeValues::Boolean,
            },
        ],
    },
    ConstructKindSpec {
        kind: "import_declaration",
        description: "A top-of-file `import …;`. Carries booleans for the import-style decisions rule authors care about (wildcard, static) plus the raw imported name for source-specific matches.",
        attributes: &[
            AttributeSpec {
                name: "is_wildcard",
                description: "`true` for `import java.util.*;` and `import static java.lang.Math.*;` — the `.*` form that pulls in every public symbol from the package or type.",
                values: AttributeValues::Boolean,
            },
            AttributeSpec {
                name: "is_static",
                description: "`true` for `import static foo.Bar.baz;` (and the wildcard form `import static foo.Bar.*;`).",
                values: AttributeValues::Boolean,
            },
            AttributeSpec {
                name: "imported_name",
                description: "The full dotted path being imported, including the trailing `.*` for wildcard imports.",
                values: AttributeValues::Text,
            },
        ],
    },
    ConstructKindSpec {
        kind: "annotation",
        description: "A Java annotation written at a declaration site (e.g. `@Override`, `@Resource`, `@SuppressWarnings(\"unchecked\")`). Covers both the no-argument *marker* form (`@Override`) and the form with arguments (`@Resource(name = \"foo\")`). Rules matching framework-injection annotations like `@Resource` / `@Autowired` / `@Inject` attach here.",
        attributes: &[
            AttributeSpec {
                name: "name",
                description: "Annotation type name as written in source, e.g. `Override`, `Deprecated`, `Resource`. For fully-qualified usages like `@javax.annotation.Resource`, only the unqualified tail is captured.",
                values: AttributeValues::Identifier,
            },
            AttributeSpec {
                name: "is_marker",
                description: "`true` when the annotation has no arguments (`@Override`, `@Deprecated`); `false` when it carries an argument list (`@SuppressWarnings(\"x\")`, `@Resource(name = \"y\")`).",
                values: AttributeValues::Boolean,
            },
        ],
    },
    ConstructKindSpec {
        kind: "local_variable_declaration",
        description: "A local variable declaration inside a method or block — the `Foo x = …;` line. Exposed so the sidebar can land on the line as a unit when the cursor is anywhere on it, even when no rule or lesson targets it specifically.",
        attributes: &[
            AttributeSpec {
                name: "type_name",
                description: "Top-level type of the declaration as written (`int`, `String`, `Map`, …). For generic types, only the bare head is captured.",
                values: AttributeValues::Identifier,
            },
            AttributeSpec {
                name: "is_final",
                description: "`true` when the declaration carries the `final` modifier.",
                values: AttributeValues::Boolean,
            },
        ],
    },
    ConstructKindSpec {
        kind: "primitive_type",
        description: "A Java primitive type token (`boolean`, `int`, `long`, `byte`, `short`, `float`, `double`, `char`, `void`). Surfaced so the cursor lands here when it's directly on the keyword — gives the reader a clear \"you're on a primitive type\" anchor even when no rule or lesson attaches.",
        attributes: &[
            AttributeSpec {
                name: "name",
                description: "The primitive keyword itself — one of `boolean`, `int`, `long`, `byte`, `short`, `float`, `double`, `char`, `void`.",
                values: AttributeValues::Enum(&[
                    "boolean", "int", "long", "byte", "short",
                    "float", "double", "char", "void",
                ]),
            },
        ],
    },
    ConstructKindSpec {
        kind: "lambda_expression",
        description: "A `->` lambda expression — anonymous function that targets a functional interface (`Runnable`, `Function`, `Comparator`, `Predicate`, …). Covers all three parameter forms (`x -> …`, `(a, b) -> …`, `(int a) -> …`) and both body shapes (expression body `x -> x + 1` and block body `x -> { … return x; }`). The `body_kind` attribute is the most useful signal for rules — block-body lambdas often indicate code that has outgrown the lambda form.",
        attributes: &[
            AttributeSpec {
                name: "body_kind",
                description: "`expression` when the body is a single expression (`x -> x + 1`); `block` when the body is a brace-delimited statement list (`x -> { return x + 1; }`).",
                values: AttributeValues::Enum(&["expression", "block"]),
            },
            AttributeSpec {
                name: "parameter_kind",
                description: "Shape of the parameter list. `identifier` — bare single parameter, no parens (`x -> …`). `inferred` — parenthesised list of parameter names with types inferred (`(a, b) -> …`). `typed` — explicitly typed parameters (`(int a) -> …`, the rarer form).",
                values: AttributeValues::Enum(&["identifier", "inferred", "typed"]),
            },
        ],
    },
    ConstructKindSpec {
        kind: "if_statement",
        description: "An `if` / `else if` / `else` chain. tree-sitter-java models `else if` as a nested `if_statement` inside the outer one's `alternative` field; this construct represents *one node* of that chain. The attributes capture whether the then- and else-branches use braces and what shape the else-branch takes, so rules can target the dangling-else footgun or pattern-matching opportunities.",
        attributes: &[
            AttributeSpec {
                name: "has_else",
                description: "`true` when the `if` has any `else` clause (chained `else if` counts).",
                values: AttributeValues::Boolean,
            },
            AttributeSpec {
                name: "consequence_is_block",
                description: "`true` when the then-branch is a brace-delimited block (`if (c) { … }`); `false` for the single-statement form (`if (c) foo();`). The single-statement form is the source of the dangling-else bug class.",
                values: AttributeValues::Boolean,
            },
            AttributeSpec {
                name: "else_kind",
                description: "Shape of the else clause. `none` — no else. `block` — `else { … }`. `if` — `else if (…) …` (a chained else-if). `single_statement` — `else foo();`, the unbraced form.",
                values: AttributeValues::Enum(&["none", "block", "if", "single_statement"]),
            },
            AttributeSpec {
                name: "then_kind",
                description: "Classification of the then-branch body, used by refactor rules. `solitary_if_no_else` — the then-branch is a single nested `if` with no else, so merging the conditions with `&&` is safe. `single_return` / `single_throw` — the then-branch is exactly one return / throw statement (with or without an enclosing block), so any explicit `else` after it is redundant. `other` — anything more complex.",
                values: AttributeValues::Enum(&[
                    "solitary_if_no_else",
                    "single_return",
                    "single_throw",
                    "other",
                ]),
            },
            AttributeSpec {
                name: "condition_complexity",
                description: "Bucketed count of short-circuit operators (`&&`, `||`) in the condition expression. `simple` — zero operators, the condition is a single predicate. `compound` — one or two operators, still readable in a glance. `long` — three or more operators, the threshold at which lifting the boolean into a named explaining variable usually clarifies intent.",
                values: AttributeValues::Enum(&["simple", "compound", "long"]),
            },
        ],
    },
    ConstructKindSpec {
        kind: "for_statement",
        description: "A `for` loop. Covers both Java forms — classic C-style (`for (init; cond; update) { … }`) and the enhanced for-each (`for (Type item : iterable) { … }`). The `style` attribute distinguishes them so rules can target one form without matching the other. For enhanced loops the element type, element name, and iterable expression are also captured for finer-grained matching.",
        attributes: &[
            AttributeSpec {
                name: "style",
                description: "`classic` for `for (init; cond; update) { … }`; `enhanced` for `for (Type x : iterable) { … }`.",
                values: AttributeValues::Enum(&["classic", "enhanced"]),
            },
            AttributeSpec {
                name: "element_type",
                description: "Enhanced-for only: source text of the element type (e.g. `ProductPromotionItemModel`, `String`, `int`). Absent for classic loops.",
                values: AttributeValues::Text,
            },
            AttributeSpec {
                name: "element_name",
                description: "Enhanced-for only: name of the loop variable (e.g. `promoItem`). Absent for classic loops.",
                values: AttributeValues::Identifier,
            },
            AttributeSpec {
                name: "iterable_text",
                description: "Enhanced-for only: source text of the expression being iterated (e.g. `promoItems`, `getItems()`). Absent for classic loops.",
                values: AttributeValues::Text,
            },
        ],
    },
    ConstructKindSpec {
        kind: "switch_statement",
        description: "A `switch` — covers both the classic statement form (`switch (x) { case A: … break; }`) and the Java 14+ expression form (`var y = switch (x) { case A -> …; };`). tree-sitter-java emits both under a single `switch_expression` node; the `style` attribute distinguishes them. The `case_style` attribute reports whether the cases use the colon (classic, fall-through) or arrow (modern, no fall-through) form — most rules care about that distinction more than the statement-vs-expression one.",
        attributes: &[
            AttributeSpec {
                name: "style",
                description: "`statement` for the side-effecting form (`switch (x) { … }` not in expression position); `expression` for the value-producing form (the result is assigned, returned, or passed).",
                values: AttributeValues::Enum(&["statement", "expression"]),
            },
            AttributeSpec {
                name: "case_style",
                description: "Shape of the case labels in the body. `colon` — every case uses the classic `case X:` (fall-through unless `break`). `arrow` — every case uses `case X -> …` (Java 14+, no fall-through). `mixed` — both styles appear. `none` — the body has no case labels (empty switch).",
                values: AttributeValues::Enum(&["colon", "arrow", "mixed", "none"]),
            },
            AttributeSpec {
                name: "has_default",
                description: "`true` when the switch body contains a `default` branch.",
                values: AttributeValues::Boolean,
            },
        ],
    },
    ConstructKindSpec {
        kind: "while_statement",
        description: "A `while (cond) { … }` loop — condition checked before each iteration, so the body may run zero times. The `condition_is_literal_true` attribute lets rules flag the common `while (true)` infinite-loop shape so they can suggest a verified exit path.",
        attributes: &[
            AttributeSpec {
                name: "condition_is_literal_true",
                description: "`true` when the condition is the literal `true` — i.e. the loop is unconditionally infinite and relies on an internal `break`/`return`/`throw` to terminate.",
                values: AttributeValues::Boolean,
            },
            AttributeSpec {
                name: "body_is_block",
                description: "`true` when the body is a brace-delimited block (`while (c) { … }`); `false` for the single-statement form (`while (c) foo();`). The single-statement form is the same dangling-statement footgun as braceless `if`.",
                values: AttributeValues::Boolean,
            },
        ],
    },
    ConstructKindSpec {
        kind: "do_statement",
        description: "A `do { … } while (cond);` loop — the body always runs at least once, then the condition decides whether to repeat. Rare in modern Java because the trailing condition reads poorly; most do-while loops can be expressed as a plain `while` once the first iteration is hoisted.",
        attributes: &[
            AttributeSpec {
                name: "condition_is_literal_true",
                description: "`true` when the trailing condition is the literal `true` — the loop relies on an internal `break`/`return`/`throw` to terminate.",
                values: AttributeValues::Boolean,
            },
        ],
    },
    ConstructKindSpec {
        kind: "array_creation_expression",
        description: "A `new T[…]` array allocation. Covers both the sized form (`new int[5]`) and the literal form (`new int[]{1, 2, 3}`). The `element_type` and `has_initializer` attributes let rules suggest `List.of(…)` / `Map.of(…)` when the allocation is really a fixed-size collection in disguise.",
        attributes: &[
            AttributeSpec {
                name: "element_type",
                description: "Top-level element type as written (`int`, `String`, `Account`). For multidimensional arrays only the innermost type is captured.",
                values: AttributeValues::Identifier,
            },
            AttributeSpec {
                name: "has_initializer",
                description: "`true` when the allocation provides an inline `{ … }` initializer (`new int[]{1, 2, 3}`); `false` for the sized-but-empty form (`new int[5]`).",
                values: AttributeValues::Boolean,
            },
        ],
    },
    ConstructKindSpec {
        kind: "constructor_declaration",
        description: "A `MyClass(…) { … }` constructor. Carries the class name (constructors share their declaring class's identifier), the declared visibility, and convenience booleans for whether the body's first statement delegates to another constructor (`this(…)` or `super(…)`). The `parameter_count` attribute is exposed for rules that flag too-many-args constructors as a builder-pattern smell.",
        attributes: &[
            AttributeSpec {
                name: "name",
                description: "Constructor name as written — the declaring class's identifier.",
                values: AttributeValues::Identifier,
            },
            AttributeSpec {
                name: "visibility",
                description: "Declared visibility — `public`, `protected`, `private`, or `package` (when no modifier is present).",
                values: AttributeValues::Enum(&["public", "protected", "private", "package"]),
            },
            AttributeSpec {
                name: "parameter_count",
                description: "Number of formal parameters as a decimal string (`0`, `1`, `2`, …). Stringified because the rule predicate vocabulary is string-typed; use `match: { parameter_count: { in: [\"5\", \"6\", \"7\"] } }` to catch wide constructors.",
                values: AttributeValues::Text,
            },
            AttributeSpec {
                name: "delegates_to",
                description: "Shape of the first body statement's delegation. `this` — the body opens with `this(…)`. `super` — opens with `super(…)`. `none` — no constructor-call delegation.",
                values: AttributeValues::Enum(&["this", "super", "none"]),
            },
        ],
    },
    ConstructKindSpec {
        kind: "interface_declaration",
        description: "An `interface Foo { … }` declaration. Visible as the outermost ancestor when hovering inside an interface body. The `has_default_methods` / `has_static_methods` booleans let rules detect interfaces that have accumulated implementation as a structural smell.",
        attributes: &[
            AttributeSpec {
                name: "name",
                description: "Interface identifier as written in source.",
                values: AttributeValues::Identifier,
            },
            AttributeSpec {
                name: "has_default_methods",
                description: "`true` when the interface declares one or more `default` methods. Heavy default-method use can indicate the interface is being asked to grow like a class.",
                values: AttributeValues::Boolean,
            },
            AttributeSpec {
                name: "has_static_methods",
                description: "`true` when the interface declares one or more `static` methods.",
                values: AttributeValues::Boolean,
            },
            AttributeSpec {
                name: "is_sealed",
                description: "`true` when the interface is declared `sealed` (Java 17+).",
                values: AttributeValues::Boolean,
            },
        ],
    },
    ConstructKindSpec {
        kind: "instanceof_expression",
        description: "An `x instanceof T` runtime type check. Java 16+ added the pattern form `x instanceof T name` which binds the cast in one step — the `is_pattern` attribute distinguishes it from the classic form that still needs a separate cast. `target_type` is the type tested against.",
        attributes: &[
            AttributeSpec {
                name: "target_type",
                description: "Source text of the type being tested against (e.g. `String`, `EmailNotification`, `List<String>`).",
                values: AttributeValues::Text,
            },
            AttributeSpec {
                name: "is_pattern",
                description: "`true` for the Java 16+ pattern form `x instanceof T name` (binds the cast result to `name`); `false` for the classic form `x instanceof T` that requires a separate cast.",
                values: AttributeValues::Boolean,
            },
        ],
    },
    ConstructKindSpec {
        kind: "throw_statement",
        description: "A `throw …;` statement. The `exception_kind` attribute reports the shape of the thrown expression — most throws are `new SomeException(...)`, and rules typically match on the type name there (`exception_type`).",
        attributes: &[
            AttributeSpec {
                name: "exception_kind",
                description: "Shape of the thrown expression. `object_creation` — `throw new Foo(...)`, the common case. `identifier` — `throw e;`, re-raising a caught exception. `method_invocation` — `throw factory.build();`, a thrown value from a call. `other` — anything more exotic.",
                values: AttributeValues::Enum(&["object_creation", "identifier", "method_invocation", "other"]),
            },
            AttributeSpec {
                name: "exception_type",
                description: "When `exception_kind` is `object_creation`: the type name being instantiated (e.g. `IllegalArgumentException`, `RuntimeException`, `Exception`). Absent for the other shapes.",
                values: AttributeValues::Identifier,
            },
        ],
    },
    ConstructKindSpec {
        kind: "text_block",
        description: "A `\"\"\" … \"\"\"` text block (Java 13+ preview, Java 15 standard). tree-sitter-java doesn't have a dedicated node for them — they surface as `string_literal` whose children include `multiline_string_fragment`. We normalise to one construct kind so authors can teach the indentation rules and write rules that nudge multi-line string concatenations toward this form. `line_count` is the rendered line count after the closing-delimiter rule trims leading whitespace.",
        attributes: &[
            AttributeSpec {
                name: "line_count",
                description: "Number of lines in the literal as written, stringified. Counts newline characters in the literal's source range plus one — so a single-line text block reports `1`, a three-line block reports `3`.",
                values: AttributeValues::Text,
            },
        ],
    },
    ConstructKindSpec {
        kind: "record_declaration",
        description: "A `record Point(int x, int y) { … }` declaration (Java 16+). Carries the type name, the component count (the items in the header parens), and `has_instance_field` — `true` when the body declares any non-`static` field on top of the components. Instance fields beyond the components defeat the record's whole point: state goes through the components.",
        attributes: &[
            AttributeSpec {
                name: "name",
                description: "Record identifier as written in source.",
                values: AttributeValues::Identifier,
            },
            AttributeSpec {
                name: "component_count",
                description: "Number of record components — the parameters in the record header `Foo(a, b, c)`, stringified.",
                values: AttributeValues::Text,
            },
            AttributeSpec {
                name: "has_instance_field",
                description: "`true` when the body declares any non-`static` field. The canonical record holds state exclusively through its components; an instance field is a sign the type wants to be a regular class.",
                values: AttributeValues::Boolean,
            },
        ],
    },
    ConstructKindSpec {
        kind: "method_reference",
        description: "A `::` method reference — `String::length`, `customer::getName`, `Account::new`. Equivalent to a lambda that calls one named target; rules typically nudge the lambda form *toward* this when the lambda body is a single call. The `reference_kind` attribute classifies the four shapes (constructor / static-or-unbound / bound / super).",
        attributes: &[
            AttributeSpec {
                name: "reference_kind",
                description: "`constructor` when the target is `new` (e.g. `Account::new`). `bound` when the receiver is a value expression (e.g. `customer::getName`). `static_or_unbound` when the receiver is a type (e.g. `String::length`, `Math::abs`) — distinguishing static-method-on-class from instance-method-via-class requires symbol resolution we don't do, so they share this label.",
                values: AttributeValues::Enum(&["constructor", "bound", "static_or_unbound", "super"]),
            },
            AttributeSpec {
                name: "target_text",
                description: "Source text after `::` — the method name being referenced, or `new` for constructor references.",
                values: AttributeValues::Text,
            },
        ],
    },
    ConstructKindSpec {
        kind: "enum_declaration",
        description: "An `enum Foo { A, B, C; … }` declaration. Carries the type name, the constant count, and `has_mutable_field` — `true` when the enum body declares any non-`final` instance field, the canonical anti-pattern for enums (their whole point is that the named constants are singletons with fixed state).",
        attributes: &[
            AttributeSpec {
                name: "name",
                description: "Enum identifier as written in source.",
                values: AttributeValues::Identifier,
            },
            AttributeSpec {
                name: "constant_count",
                description: "Number of declared `enum_constant` entries, stringified for `match: { constant_count: \"0\" }` use. An enum with zero constants is unusual and usually a typo.",
                values: AttributeValues::Text,
            },
            AttributeSpec {
                name: "has_mutable_field",
                description: "`true` when the enum body declares any non-`final`, non-`static` field. Enum instances are JVM-singletons whose identity is the constant — mutable instance state breaks the assumption that two references to `Color.RED` describe the same value.",
                values: AttributeValues::Boolean,
            },
        ],
    },
    ConstructKindSpec {
        kind: "type_parameters",
        description: "The `<T>`, `<K, V>`, `<T extends Comparable<T>>` clause on a generic class, interface, or method. Carries each parameter name plus a boolean for whether any of them violates the single-uppercase-letter convention (`T`, `E`, `K`, `V`, `R`, `T1`, `T2`, `K1`/`V1`) — Sun's original generics naming guideline and the one most style guides still follow.",
        attributes: &[
            AttributeSpec {
                name: "names",
                description: "Comma-joined list of the type-parameter identifiers as written in source (e.g. `T`, `K, V`, `T1, T2, R`).",
                values: AttributeValues::Text,
            },
            AttributeSpec {
                name: "count",
                description: "Number of type parameters, stringified for `match: { count: \"1\" }` use.",
                values: AttributeValues::Text,
            },
            AttributeSpec {
                name: "has_non_conventional_name",
                description: "`true` when any parameter name doesn't fit the conventional shape — a single uppercase letter, or a single uppercase letter followed by a small digit. Lowercase names (`t`), full-word names (`Element`), and long identifiers (`ResultType`) all flip this true.",
                values: AttributeValues::Boolean,
            },
            AttributeSpec {
                name: "has_bounds",
                description: "`true` when any parameter has an `extends` clause (`<T extends Number>`, `<T extends Comparable<T>>`).",
                values: AttributeValues::Boolean,
            },
        ],
    },
    ConstructKindSpec {
        kind: "ternary_expression",
        description: "A `cond ? a : b` expression — the only ternary operator in Java and the only conditional that has a *value* rather than driving control flow. Lives alongside `if` (which is a statement, not an expression) and the modern arrow `switch` expression. The `is_nested` attribute fires when either branch is itself a ternary, which is the readability cliff most style guides flag.",
        attributes: &[
            AttributeSpec {
                name: "is_nested",
                description: "`true` when the `consequence` or `alternative` branch is itself a `ternary_expression` — i.e. the source has chained ternaries like `a ? x : b ? y : z`. Used by `nested-ternary-discouraged` to flag the readability cliff.",
                values: AttributeValues::Boolean,
            },
        ],
    },
    ConstructKindSpec {
        kind: "catch_clause",
        description: "A `catch (… e) { … }` block attached to a `try`. The `is_multi_catch` attribute flags the Java 7+ `catch (A | B e)` shape; `is_empty` flags a catch with an empty body — the canonical \"swallowed exception\" anti-pattern.",
        attributes: &[
            AttributeSpec {
                name: "exception_types",
                description: "Source text of the caught exception type(s). For multi-catch this is the full pipe-separated text (`IOException | SQLException`); for single-type catches it's the bare type name (`IOException`).",
                values: AttributeValues::Text,
            },
            AttributeSpec {
                name: "is_multi_catch",
                description: "`true` for `catch (A | B e) { … }` (Java 7+ union catch); `false` for the single-type form.",
                values: AttributeValues::Boolean,
            },
            AttributeSpec {
                name: "is_empty",
                description: "`true` when the catch body contains no statements — the canonical exception-swallowing anti-pattern that hides failures from callers and from logs.",
                values: AttributeValues::Boolean,
            },
        ],
    },
];

/// A construct instance produced for one AST node.
pub struct Extracted {
    pub kind: &'static str,
    pub attrs: Attrs,
}

/// Inspect a tree-sitter node and return an `Extracted` instance when the node
/// corresponds to a construct-kind rules can attach to. Unknown node kinds
/// return `None` — those nodes still appear in the AST walk but produce no
/// position-stack entry.
pub fn extract(node: &Node, source: &str) -> Option<Extracted> {
    match node.kind() {
        "synchronized_statement" => Some(Extracted {
            kind: "synchronized_statement",
            attrs: synchronized_attrs(node, source),
        }),
        "method_declaration" => Some(Extracted {
            kind: "method_declaration",
            attrs: method_attrs(node, source),
        }),
        "class_declaration" => Some(Extracted {
            kind: "class_declaration",
            attrs: class_attrs(node, source),
        }),
        "binary_expression" => equality_expression(node, source),
        "method_invocation" => Some(Extracted {
            kind: "method_invocation",
            attrs: method_invocation_attrs(node, source),
        }),
        "type_identifier" | "generic_type" => declared_type(node, source),
        "field_declaration" => Some(Extracted {
            kind: "field_declaration",
            attrs: field_attrs(node, source),
        }),
        "try_statement" | "try_with_resources_statement" => Some(Extracted {
            kind: "try_statement",
            attrs: try_attrs(node),
        }),
        "import_declaration" => Some(Extracted {
            kind: "import_declaration",
            attrs: import_attrs(node, source),
        }),
        "annotation" | "marker_annotation" => Some(Extracted {
            kind: "annotation",
            attrs: annotation_attrs(node, source),
        }),
        "lambda_expression" => Some(Extracted {
            kind: "lambda_expression",
            attrs: lambda_attrs(node),
        }),
        "local_variable_declaration" => Some(Extracted {
            kind: "local_variable_declaration",
            attrs: local_variable_attrs(node, source),
        }),
        // tree-sitter-java emits each primitive keyword as its own node kind
        // (boolean_type, int_type, …) rather than a single `primitive_type`.
        // We normalise them to one construct kind for rule/lesson authors.
        "boolean_type" | "void_type" | "integral_type" | "floating_point_type"
        | "byte_type" | "short_type" | "int_type" | "long_type" | "char_type"
        | "float_type" | "double_type" => Some(Extracted {
            kind: "primitive_type",
            attrs: primitive_type_attrs(node, source),
        }),
        // tree-sitter-java has two separate node kinds for the two `for` flavours.
        // We normalise to one construct kind (`for_statement`) and use a `style`
        // attribute to distinguish classic from enhanced, so rule/lesson authors
        // can target either form without learning two construct names.
        "for_statement" | "enhanced_for_statement" => Some(Extracted {
            kind: "for_statement",
            attrs: for_statement_attrs(node, source),
        }),
        "if_statement" => Some(Extracted {
            kind: "if_statement",
            attrs: if_statement_attrs(node),
        }),
        // tree-sitter-java emits both `switch (x) { … }` and `var y = switch (x) { … }`
        // as a single `switch_expression` node; the difference is positional
        // (statement context vs expression context).
        "switch_expression" => Some(Extracted {
            kind: "switch_statement",
            attrs: switch_statement_attrs(node, source),
        }),
        "while_statement" => Some(Extracted {
            kind: "while_statement",
            attrs: while_statement_attrs(node, source),
        }),
        "do_statement" => Some(Extracted {
            kind: "do_statement",
            attrs: do_statement_attrs(node, source),
        }),
        "array_creation_expression" => Some(Extracted {
            kind: "array_creation_expression",
            attrs: array_creation_attrs(node, source),
        }),
        "constructor_declaration" => Some(Extracted {
            kind: "constructor_declaration",
            attrs: constructor_attrs(node, source),
        }),
        "interface_declaration" => Some(Extracted {
            kind: "interface_declaration",
            attrs: interface_attrs(node, source),
        }),
        "instanceof_expression" => Some(Extracted {
            kind: "instanceof_expression",
            attrs: instanceof_attrs(node, source),
        }),
        "throw_statement" => Some(Extracted {
            kind: "throw_statement",
            attrs: throw_attrs(node, source),
        }),
        "catch_clause" => Some(Extracted {
            kind: "catch_clause",
            attrs: catch_attrs(node, source),
        }),
        "ternary_expression" => Some(Extracted {
            kind: "ternary_expression",
            attrs: ternary_attrs(node),
        }),
        "type_parameters" => Some(Extracted {
            kind: "type_parameters",
            attrs: type_parameters_attrs(node, source),
        }),
        "enum_declaration" => Some(Extracted {
            kind: "enum_declaration",
            attrs: enum_attrs(node, source),
        }),
        "method_reference" => Some(Extracted {
            kind: "method_reference",
            attrs: method_reference_attrs(node, source),
        }),
        "record_declaration" => Some(Extracted {
            kind: "record_declaration",
            attrs: record_attrs(node, source),
        }),
        "string_literal" => text_block_or_none(node, source),
        _ => None,
    }
}

/// Attributes for `synchronized_statement`. The grammar field `body` holds the
/// block; the lock expression is the first non-keyword child (typically
/// `parenthesized_expression` containing the actual expression).
fn synchronized_attrs(node: &Node, source: &str) -> Attrs {
    let mut attrs = Attrs::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let kind = child.kind();
        if kind == "synchronized" || kind == "{" || kind == "}" {
            continue;
        }
        if kind == "block" {
            continue;
        }
        let inner = if kind == "parenthesized_expression" {
            first_non_punct_child(&child).unwrap_or(child)
        } else {
            child
        };
        attrs.insert("lock_expr_kind".to_string(), inner.kind().to_string());
        if let Ok(text) = inner.utf8_text(source.as_bytes()) {
            attrs.insert("lock_expr_text".to_string(), text.to_string());
        }
        break;
    }
    attrs
}

/// First child whose kind is not bare punctuation. Used to peel a
/// `parenthesized_expression` down to its real inner expression.
fn first_non_punct_child<'a>(node: &Node<'a>) -> Option<Node<'a>> {
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            if !matches!(child.kind(), "(" | ")" | "{" | "}" | "," | ";") {
                return Some(child);
            }
        }
    }
    None
}

fn named_decl_attrs(node: &Node, source: &str) -> Attrs {
    let mut attrs = Attrs::new();
    if let Some(name) = node.child_by_field_name("name") {
        if let Ok(text) = name.utf8_text(source.as_bytes()) {
            attrs.insert("name".to_string(), text.to_string());
        }
    }
    attrs
}

fn method_attrs(node: &Node, source: &str) -> Attrs {
    let mut attrs = named_decl_attrs(node, source);
    attrs.insert(
        "is_synchronized".to_string(),
        if has_modifier(node, source, "synchronized") {
            "true".to_string()
        } else {
            "false".to_string()
        },
    );
    attrs.insert(
        "is_default".to_string(),
        if has_modifier(node, source, "default") {
            "true".to_string()
        } else {
            "false".to_string()
        },
    );
    if let Some(throws) = throws_types_text(node, source) {
        attrs.insert("throws_types".to_string(), throws);
    }
    attrs
}

/// Extract the `throws X, Y, Z` clause as a comma-joined string of type names.
/// Returns `None` when the method has no `throws` clause so rules can match
/// on `present: true`.
fn throws_types_text(node: &Node, source: &str) -> Option<String> {
    for i in 0..node.child_count() {
        let Some(child) = node.child(i) else { continue };
        if child.kind() != "throws" {
            continue;
        }
        let mut names = Vec::new();
        for j in 0..child.child_count() {
            let Some(c) = child.child(j) else { continue };
            if matches!(c.kind(), "throws" | ",") {
                continue;
            }
            if let Ok(text) = c.utf8_text(source.as_bytes()) {
                names.push(text.to_string());
            }
        }
        if names.is_empty() {
            return None;
        }
        return Some(names.join(", "));
    }
    None
}

fn class_attrs(node: &Node, source: &str) -> Attrs {
    let mut attrs = named_decl_attrs(node, source);
    let (eq, hc) = class_method_overrides(node, source);
    attrs.insert(
        "overrides_equals".to_string(),
        if eq { "true".into() } else { "false".into() },
    );
    attrs.insert(
        "overrides_hashcode".to_string(),
        if hc { "true".into() } else { "false".into() },
    );
    attrs.insert(
        "in_default_package".to_string(),
        if file_has_package(node) { "false".into() } else { "true".into() },
    );
    attrs.insert(
        "is_abstract".to_string(),
        if has_modifier(node, source, "abstract") { "true".into() } else { "false".into() },
    );
    attrs.insert(
        "is_final".to_string(),
        if has_modifier(node, source, "final") { "true".into() } else { "false".into() },
    );
    attrs.insert(
        "is_sealed".to_string(),
        if has_modifier(node, source, "sealed") { "true".into() } else { "false".into() },
    );
    if let Some(sc) = node.child_by_field_name("superclass") {
        // `superclass` wraps the actual parent type as a child; the wrapper's
        // text starts with `extends `, so we either peel the keyword off the
        // text or read the inner type node directly.
        let inner_text = first_non_punct_child(&sc)
            .and_then(|n| n.utf8_text(source.as_bytes()).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| s != "extends" && !s.is_empty());
        let text = inner_text.or_else(|| {
            sc.utf8_text(source.as_bytes()).ok().map(|t| {
                t.trim().trim_start_matches("extends").trim().to_string()
            })
        });
        if let Some(t) = text {
            if !t.is_empty() {
                attrs.insert("extends_type".to_string(), t);
            }
        }
    }
    attrs.insert(
        "has_abstract_methods".to_string(),
        if class_has_abstract_methods(node, source) { "true".into() } else { "false".into() },
    );
    attrs
}

/// True when the class body declares any `abstract` method. Only the direct
/// body is inspected — methods on nested classes don't count toward the
/// enclosing class's "is this an abstract-class-without-abstract-members"
/// rule.
fn class_has_abstract_methods(node: &Node, source: &str) -> bool {
    let Some(body) = node.child_by_field_name("body") else { return false };
    for i in 0..body.child_count() {
        let Some(child) = body.child(i) else { continue };
        if child.kind() != "method_declaration" {
            continue;
        }
        if has_modifier(&child, source, "abstract") {
            return true;
        }
    }
    false
}

/// Walk up to the program root and look for any `package_declaration` child.
/// `in_default_package` flips when the file lacks one entirely.
fn file_has_package(node: &Node) -> bool {
    let mut current = Some(*node);
    while let Some(n) = current {
        if let Some(parent) = n.parent() {
            current = Some(parent);
        } else {
            // n is the root program node
            for i in 0..n.child_count() {
                if let Some(child) = n.child(i) {
                    if child.kind() == "package_declaration" {
                        return true;
                    }
                }
            }
            return false;
        }
    }
    false
}

fn import_attrs(node: &Node, source: &str) -> Attrs {
    let mut attrs = Attrs::new();
    let text = node.utf8_text(source.as_bytes()).unwrap_or("").trim();
    // Strip leading `import` keyword and trailing `;` for the `imported_name` attribute.
    let body = text
        .trim_start_matches("import")
        .trim_start()
        .trim_end_matches(';')
        .trim();
    let is_static = body.starts_with("static ");
    let name = if is_static {
        body.trim_start_matches("static").trim().to_string()
    } else {
        body.to_string()
    };
    let is_wildcard = name.ends_with(".*");
    attrs.insert(
        "is_wildcard".to_string(),
        if is_wildcard { "true".into() } else { "false".into() },
    );
    attrs.insert(
        "is_static".to_string(),
        if is_static { "true".into() } else { "false".into() },
    );
    attrs.insert("imported_name".to_string(), name);
    attrs
}

/// Inspect the class body for declared `equals` / `hashCode` methods. Only
/// looks at the direct body — methods in nested classes don't count, and
/// inherited overrides we can't see across files don't count either.
fn class_method_overrides(node: &Node, source: &str) -> (bool, bool) {
    let body = node.child_by_field_name("body");
    let Some(body) = body else {
        return (false, false);
    };
    let mut has_equals = false;
    let mut has_hashcode = false;
    for i in 0..body.child_count() {
        let Some(child) = body.child(i) else { continue };
        if child.kind() != "method_declaration" {
            continue;
        }
        let Some(name_node) = child.child_by_field_name("name") else { continue };
        let Ok(name) = name_node.utf8_text(source.as_bytes()) else { continue };
        match name {
            "equals" => has_equals = true,
            "hashCode" => has_hashcode = true,
            _ => {}
        }
    }
    (has_equals, has_hashcode)
}

fn field_attrs(node: &Node, source: &str) -> Attrs {
    let mut attrs = Attrs::new();
    let visibility = field_visibility(node, source);
    attrs.insert("visibility".to_string(), visibility);
    attrs.insert(
        "is_static".to_string(),
        if has_modifier(node, source, "static") {
            "true".into()
        } else {
            "false".into()
        },
    );
    attrs.insert(
        "is_final".to_string(),
        if has_modifier(node, source, "final") {
            "true".into()
        } else {
            "false".into()
        },
    );
    if let Some(type_node) = node.child_by_field_name("type") {
        let name = if type_node.kind() == "generic_type" {
            type_node
                .child(0)
                .and_then(|c| c.utf8_text(source.as_bytes()).ok())
                .map(|s| s.to_string())
        } else {
            type_node.utf8_text(source.as_bytes()).ok().map(|s| s.to_string())
        };
        if let Some(n) = name {
            attrs.insert("type_name".to_string(), n);
        }
    }
    attrs
}

fn field_visibility(node: &Node, source: &str) -> String {
    if has_modifier(node, source, "public") {
        "public".into()
    } else if has_modifier(node, source, "protected") {
        "protected".into()
    } else if has_modifier(node, source, "private") {
        "private".into()
    } else {
        "package".into()
    }
}

fn try_attrs(node: &Node) -> Attrs {
    let mut attrs = Attrs::new();
    let has_spec = node.kind() == "try_with_resources_statement";
    attrs.insert(
        "has_resource_spec".to_string(),
        if has_spec { "true".into() } else { "false".into() },
    );
    attrs
}

/// Attributes for `annotation` / `marker_annotation`. tree-sitter-java splits
/// `@Foo` (marker, no args) from `@Foo(args)` (full annotation), so we accept
/// both and normalise to one construct kind. The `name` field is the
/// annotation type identifier; for fully-qualified usages
/// (`@javax.annotation.Resource`) the trailing component is what we keep —
/// rule authors match on simple names like `Resource` / `Autowired` / `Inject`.
fn annotation_attrs(node: &Node, source: &str) -> Attrs {
    let mut attrs = Attrs::new();
    let is_marker = node.kind() == "marker_annotation";
    attrs.insert(
        "is_marker".to_string(),
        if is_marker { "true".into() } else { "false".into() },
    );
    if let Some(name_node) = node.child_by_field_name("name") {
        if let Ok(text) = name_node.utf8_text(source.as_bytes()) {
            let simple = text.rsplit('.').next().unwrap_or(text);
            attrs.insert("name".to_string(), simple.to_string());
        }
    }
    attrs
}

/// Attributes for `lambda_expression`. tree-sitter-java places the parameter
/// list in the `parameters` field and the body in the `body` field; the
/// parameters node's kind tells us which of the three syntactic forms is
/// used, and the body's kind separates expression-bodies from block-bodies.
fn lambda_attrs(node: &Node) -> Attrs {
    let mut attrs = Attrs::new();
    let body_kind = match node.child_by_field_name("body").map(|n| n.kind()) {
        Some("block") => "block",
        _ => "expression",
    };
    attrs.insert("body_kind".to_string(), body_kind.to_string());
    let parameter_kind = match node.child_by_field_name("parameters").map(|n| n.kind()) {
        Some("identifier") => "identifier",
        Some("inferred_parameters") => "inferred",
        Some("formal_parameters") => "typed",
        _ => "inferred",
    };
    attrs.insert("parameter_kind".to_string(), parameter_kind.to_string());
    attrs
}

/// Attributes for `local_variable_declaration` (`final boolean wawiActive = …`).
/// The `type` field is the declared type — for `generic_type` we take the head
/// identifier so rule authors match on `Map` rather than `Map<String, Integer>`.
fn local_variable_attrs(node: &Node, source: &str) -> Attrs {
    let mut attrs = Attrs::new();
    if let Some(type_node) = node.child_by_field_name("type") {
        let name = if type_node.kind() == "generic_type" {
            type_node
                .child(0)
                .and_then(|c| c.utf8_text(source.as_bytes()).ok())
                .map(|s| s.to_string())
        } else {
            type_node.utf8_text(source.as_bytes()).ok().map(|s| s.to_string())
        };
        if let Some(n) = name {
            attrs.insert("type_name".to_string(), n);
        }
    }
    attrs.insert(
        "is_final".to_string(),
        if has_modifier(node, source, "final") { "true".into() } else { "false".into() },
    );
    attrs
}

/// Attributes for the primitive-type tokens. The `name` is the keyword text
/// (`boolean`, `int`, …). tree-sitter-java sometimes nests the actual keyword
/// inside `integral_type` / `floating_point_type` wrappers, so we use the
/// raw source span — that always matches the keyword regardless of nesting.
fn primitive_type_attrs(node: &Node, source: &str) -> Attrs {
    let mut attrs = Attrs::new();
    if let Ok(text) = node.utf8_text(source.as_bytes()) {
        attrs.insert("name".to_string(), text.trim().to_string());
    }
    attrs
}

/// Attributes for `if_statement`. Reads the `consequence` and `alternative`
/// fields (tree-sitter-java's names for then-branch and else-branch) and
/// reports their shape. `else if` chains appear as nested `if_statement` in
/// the `alternative` field, so `else_kind: if` captures that case
/// distinctly from a plain `else { … }` block.
fn if_statement_attrs(node: &Node) -> Attrs {
    let mut attrs = Attrs::new();

    let consequence = node.child_by_field_name("consequence");
    let consequence_is_block = consequence
        .map(|c| c.kind() == "block")
        .unwrap_or(false);
    attrs.insert(
        "consequence_is_block".to_string(),
        if consequence_is_block { "true".into() } else { "false".into() },
    );

    let alternative = node.child_by_field_name("alternative");
    attrs.insert(
        "has_else".to_string(),
        if alternative.is_some() { "true".into() } else { "false".into() },
    );
    let else_kind = match alternative.map(|a| a.kind()) {
        None => "none",
        Some("block") => "block",
        Some("if_statement") => "if",
        Some(_) => "single_statement",
    };
    attrs.insert("else_kind".to_string(), else_kind.to_string());

    attrs.insert(
        "then_kind".to_string(),
        classify_then_kind(consequence).to_string(),
    );

    let condition = node.child_by_field_name("condition");
    attrs.insert(
        "condition_complexity".to_string(),
        classify_condition_complexity(condition).to_string(),
    );

    attrs
}

/// Count short-circuit operators (`&&`, `||`) anywhere inside the condition
/// expression and bucket the result. The threshold of 3+ lines up with the
/// `lift-boolean-into-explaining-variable` rule — a condition with four or
/// more operands is the readability cliff Fowler's "Introduce Explaining
/// Variable" refactor is meant to address.
fn classify_condition_complexity(condition: Option<Node>) -> &'static str {
    let Some(node) = condition else { return "simple" };
    let mut count: u32 = 0;
    let mut stack: Vec<Node> = vec![node];
    while let Some(n) = stack.pop() {
        if n.kind() == "binary_expression" {
            let mut cursor = n.walk();
            for c in n.children(&mut cursor) {
                let t = c.kind();
                if t == "&&" || t == "||" {
                    count = count.saturating_add(1);
                }
            }
        }
        let mut cursor = n.walk();
        for child in n.children(&mut cursor) {
            stack.push(child);
        }
    }
    match count {
        0 => "simple",
        1 | 2 => "compound",
        _ => "long",
    }
}

/// Inspect the then-branch and classify its shape. Refactor rules
/// (`nested-if-could-be-and`, `redundant-else-after-return`) match on the
/// resulting category. A block wrapping exactly one statement is treated as
/// equivalent to that statement, so `if (x) return y;` and
/// `if (x) { return y; }` classify the same way.
fn classify_then_kind(consequence: Option<Node>) -> &'static str {
    let Some(node) = consequence else { return "other" };
    // Unwrap a block whose only meaningful child is one statement. Comments
    // and braces (`{`, `}`) are not named children, so child_count over
    // named_children gives the count of statements.
    let inner = if node.kind() == "block" {
        match single_named_child(&node) {
            Some(child) => child,
            None => return "other",
        }
    } else {
        node
    };
    match inner.kind() {
        "if_statement" => {
            // Safe to merge with `&&` only when the inner if has no else.
            if inner.child_by_field_name("alternative").is_none() {
                "solitary_if_no_else"
            } else {
                "other"
            }
        }
        "return_statement" => "single_return",
        "throw_statement" => "single_throw",
        _ => "other",
    }
}

/// Return the single named child of `node`, or `None` if there are zero or
/// more than one. Named children skip punctuation tokens (`{`, `}`, `;`).
fn single_named_child<'a>(node: &Node<'a>) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    let mut iter = node.named_children(&mut cursor);
    let first = iter.next()?;
    if iter.next().is_some() {
        return None;
    }
    Some(first)
}

/// Attributes for `for_statement`. Distinguishes the two Java forms — classic
/// (`for (init; cond; update) { … }`) from enhanced for-each (`for (Type x :
/// iterable) { … }`) — via the `style` attribute. For enhanced loops we also
/// extract the element type, element name, and the iterable expression so
/// rule authors can match on specific collection types.
fn for_statement_attrs(node: &Node, source: &str) -> Attrs {
    let mut attrs = Attrs::new();
    if node.kind() == "enhanced_for_statement" {
        attrs.insert("style".to_string(), "enhanced".to_string());
        if let Some(type_node) = node.child_by_field_name("type") {
            if let Ok(text) = type_node.utf8_text(source.as_bytes()) {
                attrs.insert("element_type".to_string(), text.to_string());
            }
        }
        if let Some(name_node) = node.child_by_field_name("name") {
            if let Ok(text) = name_node.utf8_text(source.as_bytes()) {
                attrs.insert("element_name".to_string(), text.to_string());
            }
        }
        if let Some(value_node) = node.child_by_field_name("value") {
            if let Ok(text) = value_node.utf8_text(source.as_bytes()) {
                attrs.insert("iterable_text".to_string(), text.to_string());
            }
        }
    } else {
        // `for_statement` — the classic three-clause form.
        attrs.insert("style".to_string(), "classic".to_string());
    }
    attrs
}

/// True if the declaration's modifier list contains the given keyword (used for
/// `synchronized`, but trivially extends to `static`, `final`, …).
fn has_modifier(node: &Node, source: &str, keyword: &str) -> bool {
    for i in 0..node.child_count() {
        let Some(child) = node.child(i) else { continue };
        if child.kind() != "modifiers" {
            continue;
        }
        for j in 0..child.child_count() {
            if let Some(mod_node) = child.child(j) {
                if mod_node.kind() == keyword {
                    return true;
                }
                if let Ok(text) = mod_node.utf8_text(source.as_bytes()) {
                    if text == keyword {
                        return true;
                    }
                }
            }
        }
    }
    false
}

fn equality_expression(node: &Node, source: &str) -> Option<Extracted> {
    let op = node.child_by_field_name("operator")?;
    let op_text = op.utf8_text(source.as_bytes()).ok()?;
    if !matches!(op_text, "==" | "!=") {
        return None;
    }
    let mut attrs = Attrs::new();
    attrs.insert("op".to_string(), op_text.to_string());
    if let Some(lhs) = node.child_by_field_name("left") {
        if let Some(t) = infer_local_var_type(&lhs, source) {
            attrs.insert("lhs_type".to_string(), t);
        }
    }
    if let Some(rhs) = node.child_by_field_name("right") {
        if let Some(t) = infer_local_var_type(&rhs, source) {
            attrs.insert("rhs_type".to_string(), t);
        }
    }
    Some(Extracted {
        kind: "equality_expression",
        attrs,
    })
}

/// Best-effort local-variable type inference. Returns the declared type when
/// the operand is a bare identifier resolved to a `local_variable_declaration`
/// inside the enclosing method/constructor. Cross-file and field-typed
/// operands deliberately stay unresolved — false negatives are acceptable,
/// false positives are not (per design Q6).
fn infer_local_var_type(operand: &Node, source: &str) -> Option<String> {
    if operand.kind() != "identifier" {
        return None;
    }
    let name = operand.utf8_text(source.as_bytes()).ok()?;
    let mut cursor = operand.parent();
    while let Some(parent) = cursor {
        if matches!(
            parent.kind(),
            "method_declaration" | "constructor_declaration" | "lambda_expression"
        ) {
            return find_local_type(&parent, name, source);
        }
        cursor = parent.parent();
    }
    None
}

fn find_local_type(scope: &Node, name: &str, source: &str) -> Option<String> {
    if scope.kind() == "local_variable_declaration" {
        if let Some(t) = type_for_name_in_declaration(scope, name, source) {
            return Some(t);
        }
    }
    for i in 0..scope.child_count() {
        let Some(child) = scope.child(i) else { continue };
        if let Some(t) = find_local_type(&child, name, source) {
            return Some(t);
        }
    }
    None
}

fn type_for_name_in_declaration(node: &Node, name: &str, source: &str) -> Option<String> {
    let type_node = node.child_by_field_name("type")?;
    let type_text = type_node.utf8_text(source.as_bytes()).ok()?;
    for i in 0..node.child_count() {
        let Some(child) = node.child(i) else { continue };
        if child.kind() != "variable_declarator" {
            continue;
        }
        if let Some(id) = child.child_by_field_name("name") {
            if id.utf8_text(source.as_bytes()).ok() == Some(name) {
                return Some(type_text.to_string());
            }
        }
    }
    None
}

fn method_invocation_attrs(node: &Node, source: &str) -> Attrs {
    let mut attrs = Attrs::new();
    let name_text = node
        .child_by_field_name("name")
        .and_then(|n| n.utf8_text(source.as_bytes()).ok())
        .map(|s| s.to_string());
    let receiver_text = node
        .child_by_field_name("object")
        .and_then(|n| n.utf8_text(source.as_bytes()).ok())
        .map(|s| s.to_string());
    if let Some(ref n) = name_text {
        attrs.insert("name".to_string(), n.clone());
    }
    if let Some(ref r) = receiver_text {
        attrs.insert("receiver".to_string(), r.clone());
    }
    let call_target = match (&receiver_text, &name_text) {
        (Some(r), Some(n)) => Some(format!("{}.{}", r, n)),
        (None, Some(n)) => Some(n.clone()),
        _ => None,
    };
    if let Some(t) = call_target {
        attrs.insert("call_target".to_string(), t);
    }
    attrs
}

/// `declared_type` is only emitted in declaration positions (variable, field,
/// parameter, return type). Filtering by parent kind here keeps type
/// identifiers in expression contexts (`String.format(...)`, `MyType.class`)
/// from drowning every hover in noise.
fn declared_type(node: &Node, source: &str) -> Option<Extracted> {
    let parent = node.parent()?;
    let parent_is_type_position = matches!(
        parent.kind(),
        "local_variable_declaration"
            | "field_declaration"
            | "constant_declaration"
            | "formal_parameter"
            | "method_declaration"
            | "type_arguments"
            | "array_type"
    );
    // For `generic_type` we also accept that the inner type identifier
    // matters; for the outer `generic_type` we emit a single construct.
    if !parent_is_type_position {
        // Special case: `generic_type` whose parent is itself a type position
        // is fine; the inner `type_identifier`'s parent is `generic_type` and
        // we want to skip it (we emit on the outer `generic_type` instead).
        return None;
    }
    let (name, generic_args) = match node.kind() {
        "type_identifier" => {
            // If our direct parent is a `generic_type`, that outer node will
            // emit instead — avoid double-counting.
            if parent.kind() == "type_arguments" {
                return None;
            }
            let name = node.utf8_text(source.as_bytes()).ok()?.to_string();
            (name, None)
        }
        "generic_type" => {
            let first = node.child(0)?;
            if first.kind() != "type_identifier" {
                return None;
            }
            let name = first.utf8_text(source.as_bytes()).ok()?.to_string();
            let args = node
                .child_by_field_name("type_arguments")
                .or_else(|| {
                    // tree-sitter-java may not name the field on every version;
                    // fall back to the first `type_arguments` child.
                    (0..node.child_count())
                        .filter_map(|i| node.child(i))
                        .find(|c| c.kind() == "type_arguments")
                })
                .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                .map(|s| s.to_string());
            (name, args)
        }
        _ => return None,
    };
    let mut attrs = Attrs::new();
    attrs.insert("name".to_string(), name);
    if let Some(args) = generic_args {
        attrs.insert("generic_args".to_string(), args);
    }
    Some(Extracted {
        kind: "declared_type",
        attrs,
    })
}

/// Attributes for `switch_statement` (tree-sitter-java: `switch_expression`,
/// which models both the classic statement and the modern expression).
/// `style` is decided by the parent: expression-position uses (assignment,
/// argument, return) get `expression`; everything else is `statement`.
/// `case_style` walks the switch body and classifies labels by punctuation.
fn switch_statement_attrs(node: &Node, source: &str) -> Attrs {
    let mut attrs = Attrs::new();
    let style = match node.parent().map(|p| p.kind()) {
        Some("variable_declarator")
        | Some("assignment_expression")
        | Some("argument_list")
        | Some("return_statement")
        | Some("expression_statement") => {
            // expression_statement wraps a switch used purely for side effects —
            // that's still a "statement" position in spirit.
            if matches!(node.parent().map(|p| p.kind()), Some("expression_statement")) {
                "statement"
            } else {
                "expression"
            }
        }
        _ => "statement",
    };
    attrs.insert("style".to_string(), style.to_string());

    let mut seen_colon = false;
    let mut seen_arrow = false;
    let mut has_default = false;
    let body = node.child_by_field_name("body");
    if let Some(body) = body {
        walk_switch_body(&body, source, &mut seen_colon, &mut seen_arrow, &mut has_default);
    }
    let case_style = match (seen_colon, seen_arrow) {
        (true, true) => "mixed",
        (true, false) => "colon",
        (false, true) => "arrow",
        (false, false) => "none",
    };
    attrs.insert("case_style".to_string(), case_style.to_string());
    attrs.insert(
        "has_default".to_string(),
        if has_default { "true".into() } else { "false".into() },
    );
    attrs
}

/// Recursively classify the switch body's labels. tree-sitter-java may use
/// either `switch_block_statement_group` (classic, colon-style) or
/// `switch_rule` (arrow-style), and the body itself is `switch_block`.
fn walk_switch_body(
    node: &Node,
    source: &str,
    seen_colon: &mut bool,
    seen_arrow: &mut bool,
    has_default: &mut bool,
) {
    let kind = node.kind();
    if kind == "switch_block_statement_group" {
        *seen_colon = true;
        if has_default_label(node, source) {
            *has_default = true;
        }
    } else if kind == "switch_rule" {
        *seen_arrow = true;
        if has_default_label(node, source) {
            *has_default = true;
        }
    }
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            walk_switch_body(&child, source, seen_colon, seen_arrow, has_default);
        }
    }
}

fn has_default_label(node: &Node, source: &str) -> bool {
    for i in 0..node.child_count() {
        let Some(child) = node.child(i) else { continue };
        if child.kind() != "switch_label" {
            continue;
        }
        if let Ok(text) = child.utf8_text(source.as_bytes()) {
            if text.trim_start().starts_with("default") {
                return true;
            }
        }
    }
    false
}

/// Attributes for `while_statement`. tree-sitter-java exposes `condition` and
/// `body` as named fields; we read the raw text of the condition to detect
/// the literal `true` form.
fn while_statement_attrs(node: &Node, source: &str) -> Attrs {
    let mut attrs = Attrs::new();
    attrs.insert(
        "condition_is_literal_true".to_string(),
        if condition_is_literal_true(node, source) { "true".into() } else { "false".into() },
    );
    let body_is_block = node
        .child_by_field_name("body")
        .map(|b| b.kind() == "block")
        .unwrap_or(false);
    attrs.insert(
        "body_is_block".to_string(),
        if body_is_block { "true".into() } else { "false".into() },
    );
    attrs
}

/// True when the loop's condition is the literal `true`. The condition is
/// usually wrapped in a `parenthesized_expression`, so we peel one layer
/// before checking the text.
fn condition_is_literal_true(node: &Node, source: &str) -> bool {
    let Some(cond) = node.child_by_field_name("condition") else { return false };
    let inner = if cond.kind() == "parenthesized_expression" {
        first_non_punct_child(&cond).unwrap_or(cond)
    } else {
        cond
    };
    inner
        .utf8_text(source.as_bytes())
        .map(|t| t.trim() == "true")
        .unwrap_or(false)
}

fn do_statement_attrs(node: &Node, source: &str) -> Attrs {
    let mut attrs = Attrs::new();
    attrs.insert(
        "condition_is_literal_true".to_string(),
        if condition_is_literal_true(node, source) { "true".into() } else { "false".into() },
    );
    attrs
}

/// Attributes for `array_creation_expression` (`new int[5]` / `new int[]{1, 2, 3}`).
/// The `type` field is the element type; `value` (when present) is the inline
/// initializer block.
fn array_creation_attrs(node: &Node, source: &str) -> Attrs {
    let mut attrs = Attrs::new();
    if let Some(type_node) = node.child_by_field_name("type") {
        let name = type_node
            .utf8_text(source.as_bytes())
            .ok()
            .map(|s| s.trim().to_string());
        if let Some(n) = name {
            attrs.insert("element_type".to_string(), n);
        }
    }
    let has_initializer = node.child_by_field_name("value").is_some();
    attrs.insert(
        "has_initializer".to_string(),
        if has_initializer { "true".into() } else { "false".into() },
    );
    attrs
}

/// Attributes for `constructor_declaration`. tree-sitter-java reports the
/// declaring class's identifier in the `name` field, the parameter list in
/// `parameters` (a `formal_parameters` node), and the body in `body`.
fn constructor_attrs(node: &Node, source: &str) -> Attrs {
    let mut attrs = named_decl_attrs(node, source);
    attrs.insert("visibility".to_string(), field_visibility(node, source));

    let param_count = node
        .child_by_field_name("parameters")
        .map(|p| {
            let mut n = 0;
            for i in 0..p.child_count() {
                if let Some(c) = p.child(i) {
                    if c.kind() == "formal_parameter" || c.kind() == "spread_parameter" {
                        n += 1;
                    }
                }
            }
            n
        })
        .unwrap_or(0);
    attrs.insert("parameter_count".to_string(), param_count.to_string());

    attrs.insert(
        "delegates_to".to_string(),
        constructor_delegation(node).to_string(),
    );
    attrs
}

/// Inspect the constructor body's first statement to detect a delegating
/// constructor call (`this(...)` / `super(...)`). tree-sitter-java models
/// these as `explicit_constructor_invocation` whose first token is `this`
/// or `super`.
fn constructor_delegation(node: &Node) -> &'static str {
    let Some(body) = node.child_by_field_name("body") else { return "none" };
    let mut cursor = body.walk();
    for child in body.named_children(&mut cursor) {
        if child.kind() != "explicit_constructor_invocation" {
            // Skip whitespace/comments by ignoring non-named or other-kind
            // children; the *first* explicit_constructor_invocation is the
            // delegation we care about, and it must be the first statement
            // in the body — so once we see another statement kind first,
            // there's no delegation.
            continue;
        }
        // Found it — peek at the first token.
        for i in 0..child.child_count() {
            if let Some(tok) = child.child(i) {
                if tok.kind() == "this" {
                    return "this";
                }
                if tok.kind() == "super" {
                    return "super";
                }
            }
        }
        return "none";
    }
    "none"
}

/// Attributes for `interface_declaration`. The body is a `interface_body`
/// containing method/field/constant declarations; we walk it once to set
/// the modifier-based booleans without re-walking per attribute.
fn interface_attrs(node: &Node, source: &str) -> Attrs {
    let mut attrs = named_decl_attrs(node, source);

    let mut has_default = false;
    let mut has_static = false;
    if let Some(body) = node.child_by_field_name("body") {
        for i in 0..body.child_count() {
            let Some(child) = body.child(i) else { continue };
            if child.kind() != "method_declaration" {
                continue;
            }
            if has_modifier(&child, source, "default") {
                has_default = true;
            }
            if has_modifier(&child, source, "static") {
                has_static = true;
            }
        }
    }
    attrs.insert(
        "has_default_methods".to_string(),
        if has_default { "true".into() } else { "false".into() },
    );
    attrs.insert(
        "has_static_methods".to_string(),
        if has_static { "true".into() } else { "false".into() },
    );
    attrs.insert(
        "is_sealed".to_string(),
        if has_modifier(node, source, "sealed") { "true".into() } else { "false".into() },
    );
    attrs
}

/// Attributes for `instanceof_expression`. The Java 16+ pattern form
/// `x instanceof T name` adds a `name` field to the node; the classic form
/// has only `left` and `right`.
fn instanceof_attrs(node: &Node, source: &str) -> Attrs {
    let mut attrs = Attrs::new();
    if let Some(right) = node.child_by_field_name("right") {
        if let Ok(text) = right.utf8_text(source.as_bytes()) {
            attrs.insert("target_type".to_string(), text.to_string());
        }
    }
    let is_pattern = node.child_by_field_name("name").is_some();
    attrs.insert(
        "is_pattern".to_string(),
        if is_pattern { "true".into() } else { "false".into() },
    );
    attrs
}

/// Attributes for `throw_statement`. The thrown expression is the first (and
/// only) non-keyword, non-punctuation named child. We special-case the
/// `object_creation_expression` shape — overwhelmingly the common one — to
/// expose its type name.
fn throw_attrs(node: &Node, source: &str) -> Attrs {
    let mut attrs = Attrs::new();
    let mut cursor = node.walk();
    let thrown: Option<Node> = node.named_children(&mut cursor).next();
    let (kind_label, type_name) = match thrown {
        None => ("other", None),
        Some(t) => match t.kind() {
            "object_creation_expression" => {
                let type_node = t.child_by_field_name("type");
                let name = type_node
                    .and_then(|n| {
                        // Peel `generic_type` to its head identifier.
                        if n.kind() == "generic_type" {
                            n.child(0)
                        } else {
                            Some(n)
                        }
                    })
                    .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                    .map(|s| s.to_string());
                ("object_creation", name)
            }
            "identifier" => ("identifier", None),
            "method_invocation" => ("method_invocation", None),
            _ => ("other", None),
        },
    };
    attrs.insert("exception_kind".to_string(), kind_label.to_string());
    if let Some(n) = type_name {
        attrs.insert("exception_type".to_string(), n);
    }
    attrs
}

/// tree-sitter-java emits text blocks as `string_literal` nodes whose
/// children include `multiline_string_fragment`. Only those become the
/// `text_block` construct; regular `"…"` strings stay anonymous (they're
/// noise in the position stack).
fn text_block_or_none(node: &Node, source: &str) -> Option<Extracted> {
    let mut is_text_block = false;
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            if child.kind() == "multiline_string_fragment" {
                is_text_block = true;
                break;
            }
        }
    }
    if !is_text_block {
        return None;
    }
    let mut attrs = Attrs::new();
    let line_count = node
        .utf8_text(source.as_bytes())
        .map(|t| t.matches('\n').count() + 1)
        .unwrap_or(0);
    attrs.insert("line_count".to_string(), line_count.to_string());
    Some(Extracted {
        kind: "text_block",
        attrs,
    })
}

/// Attributes for `record_declaration`. Counts components from the
/// `parameters` field (a `formal_parameters` node containing
/// `formal_parameter` children) and inspects the `body` (a `class_body`)
/// for any non-`static` field_declaration — the "extra instance field"
/// shape `record-with-instance-field` matches on.
fn record_attrs(node: &Node, source: &str) -> Attrs {
    let mut attrs = named_decl_attrs(node, source);
    let component_count = node
        .child_by_field_name("parameters")
        .map(|p| {
            let mut n = 0;
            for i in 0..p.child_count() {
                if let Some(c) = p.child(i) {
                    if c.kind() == "formal_parameter" {
                        n += 1;
                    }
                }
            }
            n
        })
        .unwrap_or(0);
    attrs.insert("component_count".to_string(), component_count.to_string());

    let mut has_instance_field = false;
    if let Some(body) = node.child_by_field_name("body") {
        for i in 0..body.child_count() {
            let Some(child) = body.child(i) else { continue };
            if child.kind() != "field_declaration" {
                continue;
            }
            if !has_modifier(&child, source, "static") {
                has_instance_field = true;
            }
        }
    }
    attrs.insert(
        "has_instance_field".to_string(),
        if has_instance_field { "true".into() } else { "false".into() },
    );
    attrs
}

/// Attributes for `method_reference`. tree-sitter-java doesn't expose
/// fields on this node, and it does NOT specialise `String::length` into a
/// `type_identifier` receiver — `String` and `customer` both arrive as
/// `identifier`, so we can't classify static_or_unbound vs bound from the
/// node kind alone. We fall back to the Java naming convention: a receiver
/// whose first character is uppercase is treated as a class name
/// (`static_or_unbound`); everything else is a value (`bound`). `super` and
/// the constructor form keep their own categories.
fn method_reference_attrs(node: &Node, source: &str) -> Attrs {
    let mut attrs = Attrs::new();
    let mut receiver_text: Option<String> = None;
    let mut receiver_is_super = false;
    let mut last_text: Option<String> = None;
    for i in 0..node.child_count() {
        let Some(child) = node.child(i) else { continue };
        let kind = child.kind();
        if kind == "::" || kind == "type_arguments" {
            continue;
        }
        let text = child.utf8_text(source.as_bytes()).ok().map(|s| s.trim().to_string());
        if receiver_text.is_none() {
            if kind == "super" {
                receiver_is_super = true;
            }
            receiver_text = text.clone();
        }
        if let Some(t) = text {
            last_text = Some(t);
        }
    }
    let target_text = last_text;
    let reference_kind = match (&target_text, receiver_is_super, &receiver_text) {
        (Some(t), _, _) if t == "new" => "constructor",
        (_, true, _) => "super",
        (_, _, Some(r)) if r.chars().next().map(|c| c.is_ascii_uppercase()).unwrap_or(false) => {
            "static_or_unbound"
        }
        _ => "bound",
    };
    attrs.insert("reference_kind".to_string(), reference_kind.to_string());
    if let Some(t) = target_text {
        attrs.insert("target_text".to_string(), t);
    }
    attrs
}

/// Attributes for `enum_declaration`. Walks the `enum_body` for
/// `enum_constant` count and into `enum_body_declarations` for any
/// non-final non-static `field_declaration` (the mutable-instance-state
/// anti-pattern).
fn enum_attrs(node: &Node, source: &str) -> Attrs {
    let mut attrs = named_decl_attrs(node, source);
    let mut constant_count = 0usize;
    let mut has_mutable_field = false;
    if let Some(body) = node.child_by_field_name("body") {
        for i in 0..body.child_count() {
            let Some(child) = body.child(i) else { continue };
            match child.kind() {
                "enum_constant" => constant_count += 1,
                "enum_body_declarations" => {
                    for j in 0..child.child_count() {
                        let Some(decl) = child.child(j) else { continue };
                        if decl.kind() != "field_declaration" {
                            continue;
                        }
                        let is_static = has_modifier(&decl, source, "static");
                        let is_final = has_modifier(&decl, source, "final");
                        if !is_static && !is_final {
                            has_mutable_field = true;
                        }
                    }
                }
                _ => {}
            }
        }
    }
    attrs.insert("constant_count".to_string(), constant_count.to_string());
    attrs.insert(
        "has_mutable_field".to_string(),
        if has_mutable_field { "true".into() } else { "false".into() },
    );
    attrs
}

/// Attributes for `type_parameters`. Walks each `type_parameter` child for
/// its identifier and (optionally) its `type_bound`, then computes the
/// summary booleans rules match on.
fn type_parameters_attrs(node: &Node, source: &str) -> Attrs {
    let mut attrs = Attrs::new();
    let mut names: Vec<String> = Vec::new();
    let mut has_bounds = false;
    let mut cursor = node.walk();
    for tp in node.named_children(&mut cursor) {
        if tp.kind() != "type_parameter" {
            continue;
        }
        // First identifier child is the parameter name; type_bound is a sibling.
        for i in 0..tp.child_count() {
            let Some(child) = tp.child(i) else { continue };
            match child.kind() {
                "type_identifier" => {
                    if let Ok(text) = child.utf8_text(source.as_bytes()) {
                        names.push(text.to_string());
                    }
                }
                "type_bound" => has_bounds = true,
                _ => {}
            }
        }
    }
    attrs.insert("count".to_string(), names.len().to_string());
    let has_non_conventional = names.iter().any(|n| !is_conventional_type_param(n));
    attrs.insert(
        "has_non_conventional_name".to_string(),
        if has_non_conventional { "true".into() } else { "false".into() },
    );
    attrs.insert(
        "has_bounds".to_string(),
        if has_bounds { "true".into() } else { "false".into() },
    );
    attrs.insert("names".to_string(), names.join(", "));
    attrs
}

/// The Sun-era convention: a single uppercase letter, optionally followed
/// by a single digit. `T`, `E`, `K`, `V`, `R`, `T1`, `K2`, `U` all pass;
/// `t`, `Type`, `Element`, `ResultType` all fail.
fn is_conventional_type_param(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else { return false };
    if !first.is_ascii_uppercase() {
        return false;
    }
    match chars.next() {
        None => true,
        Some(c) if c.is_ascii_digit() => chars.next().is_none(),
        _ => false,
    }
}

/// Attributes for `ternary_expression`. We peel one layer of
/// `parenthesized_expression` off each branch before checking the kind, so
/// `a ? (b ? c : d) : e` still counts as nested even though the inner
/// ternary is parenthesised. We deliberately do not chase further — a rule
/// that wants the depth count can read the source text.
fn ternary_attrs(node: &Node) -> Attrs {
    let mut attrs = Attrs::new();
    let is_nested = branch_is_ternary(node.child_by_field_name("consequence"))
        || branch_is_ternary(node.child_by_field_name("alternative"));
    attrs.insert(
        "is_nested".to_string(),
        if is_nested { "true".into() } else { "false".into() },
    );
    attrs
}

fn branch_is_ternary(branch: Option<Node>) -> bool {
    let Some(branch) = branch else { return false };
    let inner = if branch.kind() == "parenthesized_expression" {
        first_non_punct_child(&branch).unwrap_or(branch)
    } else {
        branch
    };
    inner.kind() == "ternary_expression"
}

/// Attributes for `catch_clause`. tree-sitter-java models the
/// `(IOException | SQLException e)` parameter as a `catch_formal_parameter`
/// whose `catch_type` child holds the pipe-separated type list. An empty
/// catch body (zero named statements inside the `block`) flips
/// `is_empty: true`.
fn catch_attrs(node: &Node, source: &str) -> Attrs {
    let mut attrs = Attrs::new();
    let mut is_multi = false;
    let mut types_text: Option<String> = None;
    for i in 0..node.child_count() {
        let Some(child) = node.child(i) else { continue };
        if child.kind() != "catch_formal_parameter" {
            continue;
        }
        // catch_formal_parameter has no `type` field; the union of types is a
        // direct child of kind `catch_type` (which itself contains the
        // pipe-separated identifiers).
        for j in 0..child.child_count() {
            let Some(grandchild) = child.child(j) else { continue };
            if grandchild.kind() != "catch_type" {
                continue;
            }
            if let Ok(text) = grandchild.utf8_text(source.as_bytes()) {
                let trimmed = text.trim().to_string();
                is_multi = trimmed.contains('|');
                types_text = Some(trimmed);
            }
            break;
        }
        break;
    }
    if let Some(t) = types_text {
        attrs.insert("exception_types".to_string(), t);
    }
    attrs.insert(
        "is_multi_catch".to_string(),
        if is_multi { "true".into() } else { "false".into() },
    );

    let is_empty = node
        .child_by_field_name("body")
        .map(|b| {
            let mut cursor = b.walk();
            let empty = b.named_children(&mut cursor).next().is_none();
            empty
        })
        .unwrap_or(false);
    attrs.insert(
        "is_empty".to_string(),
        if is_empty { "true".into() } else { "false".into() },
    );
    attrs
}
