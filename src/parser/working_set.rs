//! The working set: how many distinct names a reader has to hold to follow
//! one callable body.
//!
//! Every other per-callable metric mezz computes is a measure of *control
//! flow* — branches, nesting depth, span. A function with no branches at all
//! can still be unreadable, and the analyzer scored one such body (twelve
//! locals, three mixed altitudes, a mutated global) at `cyclomatic 1,
//! cognitive 0, composite 0.17` — the healthy band. What that body was long
//! on is names in view, which nothing here measured.
//!
//! So this counts them: parameters, plus the bindings the body introduces,
//! plus the instance fields it reaches for. The threshold is Miller's
//! 7±2 (`Thresholds::working_set`), and crossing the red line is the
//! `OverfullHead` smell.
//!
//! **The count is a floor, not a total.** Two things hold it below the true
//! number, both deliberate:
//!
//! * *Implicit field access.* A field is counted only where the language
//!   spells the receiver — `self.` in Rust and Python, `this.` in
//!   TypeScript, Dart and Java, `this->` in C++. Java, Kotlin, Groovy and
//!   C++ all let a method say `total` for `this.total`, and Go's receiver
//!   is named by the author, so fields go uncounted there. A floor that
//!   never over-reports is the right error to make for a metric that
//!   triggers refactoring.
//! * *Unresolved grammars.* A language whose declaration kind is not in
//!   [`BINDING_FIELDS`] contributes parameters only.
//!
//! Nested *named* functions are not descended into: they are separate
//! entities with their own working set, and folding their locals into the
//! parent would charge one reader for two views. Closures and lambdas *are*
//! descended into — their bindings are in view exactly where you are
//! reading.

use crate::models::CodeEntity;
use std::collections::HashSet;
use tree_sitter::Node;

/// What one body asks a reader to hold, split so the renderer can say which
/// part is heavy. Parameters are counted by the caller — they live on the
/// declaration, not in the body — and added in
/// [`EntityMetrics::working_set`](crate::models::entity::EntityMetrics).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BodyNames {
    /// Distinct names bound inside the body, counted once however many
    /// times they are rebound or shadowed.
    pub locals: u32,
    /// Distinct instance fields the body reaches for through an explicit
    /// receiver. A floor — see the module docs.
    pub fields: u32,
}

/// Declaration node kinds across the grammars mezz parses, each paired with
/// the field holding the name(s) it binds.
///
/// A table rather than a `match` so that adding a language costs one row and
/// no complexity: the walker's own cyclomatic count does not grow with the
/// number of languages supported.
///
/// The second element is the tree-sitter field name; where a grammar gives
/// the binding no field, `""` means "take the first named child".
const BINDING_FIELDS: &[(&str, &str)] = &[
    // Rust
    ("let_declaration", "pattern"),
    ("const_item", "name"),
    ("static_item", "name"),
    // TypeScript / JavaScript / Svelte / Java / Groovy
    ("variable_declarator", "name"),
    // Go
    ("short_var_declaration", "left"),
    ("var_spec", "name"),
    ("const_spec", "name"),
    // C / C++. `declaration` covers `Order o;` and `int i = 0;` alike, and
    // `init_declarator` catches the ones nested under a `for` header;
    // `for_range_loop` binds the loop variable of `for (auto& x : xs)`.
    ("declaration", "declarator"),
    ("init_declarator", "declarator"),
    ("for_range_loop", "declarator"),
    // Kotlin
    ("variable_declaration", ""),
    ("multi_variable_declaration", ""),
    // Dart
    ("initialized_variable_definition", ""),
    ("initialized_identifier", ""),
    // Python — no declarations, so first binding is the declaration. Distinct
    // counting is what makes this sound: `x = 1` three times is one name.
    ("assignment", "left"),
    ("for_in_clause", "left"),
    ("as_pattern_target", ""),
];

/// Kinds whose subtree belongs to *another* entity's working set.
///
/// Shared with [`super::loops`], which draws the same line for the same
/// reason: a nested named function is its own entity, and folding its body
/// into the parent's measurement charges one reader for two views.
pub(super) const NESTED_ENTITY_KINDS: &[&str] = &[
    "function_item",
    "function_declaration",
    "function_definition",
    "method_declaration",
    "method_definition",
    "class_declaration",
    "class_definition",
    "class_body",
    "class_specifier",
    "struct_specifier",
    "union_specifier",
    "impl_item",
    "struct_item",
    "enum_item",
    "trait_item",
];

/// Leaf kinds that spell a bound name.
const NAME_LEAVES: &[&str] = &[
    "identifier",
    "simple_identifier",
    "shorthand_field_identifier",
    "field_identifier",
];

/// Subtrees under a binding that name *types*, not values.
const TYPE_KINDS: &[&str] = &[
    "type_identifier",
    "type_annotation",
    "type_arguments",
    "type_parameters",
    "generic_type",
    "scoped_type_identifier",
    "primitive_type",
    "user_type",
];

/// Receivers that mark an instance-field access rather than a local read.
const SELF_RECEIVERS: &[&str] = &["self", "this", "cls"];

/// Node kinds that *are* a member access. Both readers below need this list,
/// for opposite reasons: [`touched_field`] counts only these, and
/// [`collect_names`] refuses to descend into them.
///
/// Without the first restriction `const alias = this;` reads the declarator's
/// own `name` field as a touched field. Without the second, Python's
/// `self.total = 1` — an `assignment`, the same node kind as `x = 1` — has
/// its target walked for identifiers and books `self` and `total` as two new
/// locals, when it declares none.
const ACCESS_KINDS: &[&str] = &[
    "field_expression",
    "member_expression",
    "attribute",
    "field_access",
    "selector_expression",
    "navigation_expression",
    "subscript",
    "subscript_expression",
    "index_expression",
];

/// Walk one callable body and count the distinct names it puts in view.
///
/// Iterative, like [`compute_complexity`](crate::parser::rust::complexity),
/// and single-pass: the same traversal finds bindings and field accesses.
pub fn count_body_names(body: &Node, source: &str) -> BodyNames {
    let mut locals: HashSet<String> = HashSet::new();
    let mut fields: HashSet<String> = HashSet::new();
    let mut stack: Vec<Node> = vec![*body];

    while let Some(node) = stack.pop() {
        if let Some(binding) = binding_node(&node) {
            collect_names(&binding, source, &mut locals);
        }
        if let Some(field) = touched_field(&node, source) {
            fields.insert(field);
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            // The body's own node can be a nested-entity kind (a Python
            // `block` under a `function_definition` is not, but Go hands us
            // the declaration itself in places); only *children* are skipped.
            if NESTED_ENTITY_KINDS.contains(&child.kind()) {
                continue;
            }
            stack.push(child);
        }
    }

    BodyNames {
        locals: locals.len() as u32,
        fields: fields.len() as u32,
    }
}

/// The subtree holding the names a declaration binds, or `None` when the node
/// declares nothing.
fn binding_node<'a>(node: &Node<'a>) -> Option<Node<'a>> {
    let (_, field) = BINDING_FIELDS.iter().find(|(k, _)| *k == node.kind())?;
    if field.is_empty() {
        return node.named_child(0);
    }
    node.child_by_field_name(field)
}

/// Collect every value name spelled inside a binding, skipping type
/// annotations. Destructuring counts each name it introduces, which is the
/// honest answer: `let (head, tail) = …` puts two names in view.
///
/// The subtree a binding hands over is not always only the names — see
/// [`push_bound_children`] for the part of it that is skipped.
fn collect_names(binding: &Node, source: &str, out: &mut HashSet<String>) {
    let mut stack: Vec<Node> = vec![*binding];
    while let Some(node) = stack.pop() {
        let kind = node.kind();
        // A type annotation names no value, and an assignment into a member
        // (`self.total = 1`, `items[0] = x`) declares nothing — both would
        // otherwise contribute identifiers that are not locals.
        if TYPE_KINDS.contains(&kind) || ACCESS_KINDS.contains(&kind) {
            continue;
        }
        if NAME_LEAVES.contains(&kind) && node.child_count() == 0 {
            if let Ok(text) = node.utf8_text(source.as_bytes()) {
                out.insert(text.to_string());
            }
            continue;
        }
        push_bound_children(&node, &mut stack);
    }
}

/// Push the children of a binding subtree that can still name something.
///
/// The value a binding is initialised with is not one of them. C++ spells
/// `double sum = a + b;` as a `declaration` whose `declarator` is an
/// `init_declarator` holding *both* the name and the expression it is
/// initialised from, so a walk that took every identifier under it would
/// book `a` and `b` as two more locals when the statement declares one.
/// No grammar here puts a bound name in a `value` field, so skipping it
/// costs nothing anywhere else.
fn push_bound_children<'t>(node: &Node<'t>, stack: &mut Vec<Node<'t>>) {
    let mut cursor = node.walk();
    for (index, child) in node.children(&mut cursor).enumerate() {
        if node.field_name_for_child(index as u32) == Some("value") {
            continue;
        }
        stack.push(child);
    }
}

/// The receiver half of a member access, under whichever field name the
/// grammar in hand gives it: Rust says `value`, TypeScript and Java say
/// `object`, Kotlin says `operand`, C++ says `argument`.
fn access_receiver<'t>(node: &Node<'t>) -> Option<Node<'t>> {
    node.child_by_field_name("value")
        .or_else(|| node.child_by_field_name("object"))
        .or_else(|| node.child_by_field_name("operand"))
        .or_else(|| node.child_by_field_name("argument"))
}

/// The field name behind an explicit-receiver access — `self.total`,
/// `this.total` — or `None` for anything else, including a call through the
/// same receiver (`self.render()` is a dependency, already counted as
/// fan-out, and reading it costs a name only if the result is bound).
fn touched_field(node: &Node, source: &str) -> Option<String> {
    if !ACCESS_KINDS.contains(&node.kind()) {
        return None;
    }
    let receiver = access_receiver(node)?;
    let receiver_text = receiver.utf8_text(source.as_bytes()).ok()?;
    if !SELF_RECEIVERS.contains(&receiver_text) {
        return None;
    }
    let field = node
        .child_by_field_name("field")
        .or_else(|| node.child_by_field_name("property"))
        .or_else(|| node.child_by_field_name("attribute"))
        .or_else(|| node.child_by_field_name("name"))?;
    // A method call reaches here as `self.render` inside a call expression;
    // the parent decides. Only bare accesses count.
    if node
        .parent()
        .is_some_and(|p| p.kind().contains("call") && p.child_by_field_name("function") == Some(*node))
    {
        return None;
    }
    field.utf8_text(source.as_bytes()).ok().map(str::to_string)
}

/// Fill `local_count` and `working_set` on an entity whose `param_count` is
/// already set. One call per language, placed beside `compute_complexity`.
///
/// A bodyless declaration — an interface method, an `abstract` member, a
/// trait signature — gets its parameter count and nothing else, matching how
/// the complexity metrics treat the same case: the signature is real, the
/// body simply has no names to add.
pub fn populate(entity: &mut CodeEntity, body: Option<&Node>, source: &str) {
    let params = entity.metrics.param_count.unwrap_or(0);
    let names = match body {
        Some(body) => count_body_names(body, source),
        None => BodyNames::default(),
    };
    entity.metrics.local_count = Some(names.locals);
    entity.metrics.working_set = Some(params + names.locals + names.fields);
}

#[cfg(test)]
mod tests {
    use crate::models::{CodeEntity, EntityKind};
    use crate::parser::language_parser::LanguageParser;
    use std::path::Path;

    /// `(local_count, working_set)` for one callable, taken through the real
    /// parser for the language rather than through a hand-built tree — the
    /// node kinds in [`super::BINDING_FIELDS`] are a claim about eight
    /// grammars, and only a real parse can check it.
    fn ws(parser: &dyn LanguageParser, file: &str, src: &str, name: &str) -> (u32, u32) {
        let result = parser.parse(Path::new(file), src).expect("parse");
        let e: &CodeEntity = result
            .entities
            .iter()
            .find(|e| {
                e.name == name && matches!(e.kind, EntityKind::Function | EntityKind::Method)
            })
            .unwrap_or_else(|| panic!("callable `{name}` not found in {file}"));
        (
            e.metrics.local_count.expect("local_count"),
            e.metrics.working_set.expect("working_set"),
        )
    }

    #[test]
    fn rust_counts_params_locals_and_self_fields() {
        let src = r#"
            struct S { total: u32 }
            impl S {
                fn run(&self, a: u32, b: u32) -> u32 {
                    let x = a + b;
                    let y = x * 2;
                    self.total + y
                }
            }
        "#;
        // 2 params + 2 locals + 1 field
        assert_eq!(ws(&crate::parser::RustParser::new(), "a.rs", src, "run"), (2, 5));
    }

    /// Shadowing is one name in view, not two. Without this the metric would
    /// punish the idiom Rust uses to *avoid* holding two names.
    #[test]
    fn rust_shadowing_counts_once() {
        let src = r#"
            fn f(input: String) -> String {
                let value = input.trim();
                let value = value.to_lowercase();
                let value = value.replace(' ', "-");
                value
            }
        "#;
        assert_eq!(ws(&crate::parser::RustParser::new(), "a.rs", src, "f"), (1, 2));
    }

    /// Destructuring introduces every name it binds — two names to hold, and
    /// the count says two.
    #[test]
    fn rust_destructuring_counts_each_name() {
        let src = r#"
            fn f(pair: (u32, u32)) -> u32 {
                let (head, tail) = pair;
                head + tail
            }
        "#;
        assert_eq!(ws(&crate::parser::RustParser::new(), "a.rs", src, "f"), (2, 3));
    }

    /// A nested `fn` is its own entity with its own working set; folding its
    /// locals into the parent would charge one reader for two views.
    #[test]
    fn rust_nested_fn_does_not_inflate_its_parent() {
        let src = r#"
            fn outer(a: u32) -> u32 {
                fn inner(b: u32) -> u32 {
                    let p = b + 1;
                    let q = p + 1;
                    q
                }
                let r = inner(a);
                r
            }
        "#;
        assert_eq!(ws(&crate::parser::RustParser::new(), "a.rs", src, "outer"), (1, 2));
    }

    /// Python rebinds rather than declares, so the same name assigned three
    /// times is one name. This is the case that makes distinct counting
    /// load-bearing rather than a refinement.
    #[test]
    fn python_rebinding_counts_once() {
        let src = "
def f(seed):
    acc = seed
    acc = acc + 1
    acc = acc * 2
    other = acc
    return other
";
        assert_eq!(ws(&crate::parser::PythonParser::new(), "a.py", src, "f"), (2, 3));
    }

    #[test]
    fn python_counts_self_attributes() {
        let src = "
class C:
    def run(self, a):
        x = a
        return self.total + self.rate + x
";
        // 1 param (self is dropped) + 1 local + 2 attributes
        assert_eq!(ws(&crate::parser::PythonParser::new(), "a.py", src, "run"), (1, 4));
    }

    #[test]
    fn typescript_counts_declarators_and_this_fields() {
        let src = "
class C {
  run(a: number, b: number): number {
    const x = a + b;
    let y = x * 2;
    return this.total + y;
  }
}
";
        assert_eq!(ws(&crate::parser::TypeScriptParser::new(), "a.ts", src, "run"), (2, 5));
    }

    #[test]
    fn java_counts_declarators() {
        let src = "
class A {
    int m(int a) {
        int x = a + 1;
        int y = x + 1;
        String s = \"z\";
        return x + y;
    }
}
";
        assert_eq!(ws(&crate::parser::JavaParser::new(), "A.java", src, "m"), (3, 4));
    }

    #[test]
    fn go_counts_short_and_var_declarations() {
        let src = "
package p

func f(a int) int {
    x := a + 1
    var y int = x + 1
    z, w := y, x
    return z + w
}
";
        assert_eq!(ws(&crate::parser::GoParser::new(), "a.go", src, "f"), (4, 5));
    }

    #[test]
    fn kotlin_counts_val_and_var() {
        let src = "
class C {
    fun run(a: Int): Int {
        val x = a + 1
        var y = x + 1
        return x + y
    }
}
";
        assert_eq!(ws(&crate::parser::KotlinParser::new(), "a.kt", src, "run"), (2, 3));
    }

    #[test]
    fn dart_counts_local_definitions() {
        let src = "
class C {
  int run(int a) {
    var x = a + 1;
    final y = x + 1;
    return x + y;
  }
}
";
        assert_eq!(ws(&crate::parser::DartParser::new(), "a.dart", src, "run"), (2, 3));
    }

    #[test]
    fn groovy_counts_local_definitions() {
        let src = "
class C {
    int run(int a) {
        def x = a + 1
        int y = x + 1
        return x + y
    }
}
";
        assert_eq!(ws(&crate::parser::GroovyParser::new(), "a.groovy", src, "run"), (2, 3));
    }

    /// `self.total = 1` is an `assignment` — the same node kind Python uses
    /// to declare a local — but it declares nothing. Counting its target for
    /// identifiers booked `self` and `total` as two new names.
    #[test]
    fn python_assigning_into_an_attribute_declares_no_local() {
        let src = "
class C:
    def run(self, a):
        self.total = a
        self.total = self.total + 1
        return self.total
";
        // 1 param, 0 locals, 1 field
        assert_eq!(ws(&crate::parser::PythonParser::new(), "a.py", src, "run"), (0, 2));
    }

    /// Aliasing the receiver reads the declarator's own `name` field as a
    /// member access unless the field reader is restricted to access kinds.
    #[test]
    fn typescript_aliasing_this_is_a_local_not_a_field() {
        let src = "
class C {
  run(): number {
    const alias = this;
    return 1;
  }
}
";
        assert_eq!(ws(&crate::parser::TypeScriptParser::new(), "a.ts", src, "run"), (1, 1));
    }

    /// A signature with no body has its parameters in view and nothing else.
    /// Reporting `None` instead would drop the entity out of `mezz quality`,
    /// which is the mistake the complexity metrics already learned not to
    /// make (TS-003, KT-002).
    #[test]
    fn a_bodyless_declaration_is_measured_as_its_parameters() {
        let src = "
interface I {
  run(a: number, b: number): number;
}
";
        assert_eq!(ws(&crate::parser::TypeScriptParser::new(), "a.ts", src, "run"), (0, 2));
    }
}
