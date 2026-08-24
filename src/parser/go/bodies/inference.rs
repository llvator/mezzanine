//! Which type a written name carries, as far as one file can tell.
//!
//! This exists because of how a Go call reads. `s.repo.Find(id)` names no
//! type anywhere in it, and without one the only honest target is the text
//! `s.repo.Find` — a ghost node named after an expression, which is worse
//! than no edge at all. But Go declares more than it looks like it does:
//! a method states its receiver's type, a signature states every
//! parameter's, a struct states every field's, and `x := &Order{}` states
//! its own. Following those three hops — receiver to type, type to field,
//! field to type — turns `s.repo.Find` into `Repository.Find`, which is an
//! entity the resolver can actually find.
//!
//! Two tables, because they have different lifetimes. [`TypeIndex`] is
//! built once per file from its declarations; [`Locals`] is built per
//! function body from its signature and its own assignments.
//!
//! Both stop at the file edge. A field whose type is declared in another
//! file of the same package is not resolved here, and the call through it
//! is dropped rather than guessed at — the parser sees one file at a time,
//! and inventing the rest is how a graph starts lying.
//!
//! Types are recorded with their package attached (`sync.WaitGroup`, not
//! `WaitGroup`). The package is what tells a project's own `WaitGroup`
//! from the standard library's, and dropping it here is what made
//! `wg.Add(1)` draw an edge to a method nobody in the project wrote.

use super::super::helpers::strip_decoration;
use crate::parser::language_parser::node_text;
use std::collections::HashMap;
use tree_sitter::Node;

/// What a file's own declarations say about the types of things.
#[derive(Debug, Default)]
pub(in crate::parser::go) struct TypeIndex {
    /// `(struct name, field name)` → the field's base type name.
    fields: HashMap<(String, String), String>,
    /// Package-level `var` and `const` names → base type name.
    globals: HashMap<String, String>,
}

impl TypeIndex {
    /// Read every struct's fields and every package-level variable's type
    /// out of one parsed file.
    pub(in crate::parser::go) fn build(root: Node, source: &str) -> Self {
        let mut index = Self::default();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            match node.kind() {
                "type_spec" => index.record_struct(&node, source),
                "var_spec" | "const_spec" => index.record_globals(&node, source),
                _ => {}
            }
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                stack.push(child);
            }
        }
        index
    }

    /// The declared type of `field` on `owner`, when this file declares
    /// the struct.
    pub(in crate::parser::go) fn field_type(&self, owner: &str, field: &str) -> Option<&str> {
        self.fields
            .get(&(owner.to_string(), field.to_string()))
            .map(String::as_str)
    }

    /// The declared type of a package-level name.
    pub(in crate::parser::go) fn global_type(&self, name: &str) -> Option<&str> {
        self.globals.get(name).map(String::as_str)
    }

    fn record_struct(&mut self, spec: &Node, source: &str) {
        let (Some(name), Some(type_node)) = (
            spec.child_by_field_name("name"),
            spec.child_by_field_name("type"),
        ) else {
            return;
        };
        if type_node.kind() != "struct_type" {
            return;
        }
        let owner = node_text(&name, source).to_string();
        for (field, type_name) in struct_fields(&type_node, source) {
            self.fields.insert((owner.clone(), field), type_name);
        }
    }

    fn record_globals(&mut self, spec: &Node, source: &str) {
        let Some(type_node) = spec.child_by_field_name("type") else {
            return;
        };
        // Only package-level specs belong here. One nested in a function
        // body is a local, and `Locals` reads it with the right scope.
        if !is_package_level(spec) {
            return;
        }
        let type_name = strip_decoration(node_text(&type_node, source)).to_string();
        let mut cursor = spec.walk();
        for name in spec.children_by_field_name("name", &mut cursor) {
            self.globals
                .insert(node_text(&name, source).to_string(), type_name.clone());
        }
    }
}

/// The names bound inside one function body, and the type each carries.
#[derive(Debug, Default)]
pub(in crate::parser::go) struct Locals {
    by_name: HashMap<String, String>,
}

impl Locals {
    /// Collect what `decl`'s signature declares and what its body binds.
    ///
    /// The signature half is exact — Go writes the type of every receiver,
    /// parameter and named result. The body half is deliberately partial:
    /// it takes the two forms that state a type outright, `x := &Order{}`
    /// and `var x *Order`, and leaves `x := load()` alone rather than
    /// chasing a return type through a call.
    pub(in crate::parser::go) fn for_body(decl: &Node, body: &Node, source: &str) -> Self {
        let mut locals = Self::default();
        for field in ["receiver", "parameters", "result"] {
            if let Some(list) = decl.child_by_field_name(field) {
                locals.record_parameter_list(&list, source);
            }
        }
        locals.record_body(body, source);
        locals
    }

    /// The type of a name in scope, or `None` when this file never said.
    pub(in crate::parser::go) fn type_of(&self, name: &str) -> Option<&str> {
        self.by_name.get(name).map(String::as_str)
    }

    /// Every named entry of a `parameter_list`, including a receiver's, and
    /// including the `x, y int` shape that shares one type across names.
    fn record_parameter_list(&mut self, list: &Node, source: &str) {
        if list.kind() != "parameter_list" {
            return;
        }
        let mut cursor = list.walk();
        for decl in list.children(&mut cursor) {
            if !matches!(
                decl.kind(),
                "parameter_declaration" | "variadic_parameter_declaration"
            ) {
                continue;
            }
            let Some(type_node) = decl.child_by_field_name("type") else {
                continue;
            };
            let type_name = strip_decoration(node_text(&type_node, source)).to_string();
            let mut names = decl.walk();
            for name in decl.children_by_field_name("name", &mut names) {
                self.by_name
                    .insert(node_text(&name, source).to_string(), type_name.clone());
            }
        }
    }

    /// Walk the body for the two statement shapes that state a type.
    ///
    /// Nested function literals are walked too. Their bindings leak into
    /// the enclosing scope's table, which Go's scoping does not do — but a
    /// name bound in a closure and reused for something else in the same
    /// function is a shadowing pattern the compiler discourages and code
    /// review catches, and the cost of pretending otherwise (losing every
    /// edge inside every closure) is much higher than the cost of being
    /// wrong in that case.
    fn record_body(&mut self, body: &Node, source: &str) {
        let mut stack = vec![*body];
        while let Some(node) = stack.pop() {
            match node.kind() {
                "var_spec" => self.record_var_spec(&node, source),
                "short_var_declaration" => self.record_short_var(&node, source),
                _ => {}
            }
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                stack.push(child);
            }
        }
    }

    fn record_var_spec(&mut self, spec: &Node, source: &str) {
        let Some(type_node) = spec.child_by_field_name("type") else {
            return;
        };
        let type_name = strip_decoration(node_text(&type_node, source)).to_string();
        let mut cursor = spec.walk();
        for name in spec.children_by_field_name("name", &mut cursor) {
            self.by_name
                .insert(node_text(&name, source).to_string(), type_name.clone());
        }
    }

    /// `x := &Order{}`, `x := Order{}`, `x := new(Order)`.
    ///
    /// Only the one-name-one-value form is read. `a, b := f()` states no
    /// type for either name, and guessing which of two names took which
    /// half of a return would be fiction.
    fn record_short_var(&mut self, node: &Node, source: &str) {
        let (Some(left), Some(right)) = (
            node.child_by_field_name("left"),
            node.child_by_field_name("right"),
        ) else {
            return;
        };
        let (Some(name), Some(value)) = (only_child(&left), only_child(&right)) else {
            return;
        };
        if name.kind() != "identifier" {
            return;
        }
        let Some(type_name) = constructed_type(&value, source) else {
            return;
        };
        self.by_name
            .insert(node_text(&name, source).to_string(), type_name);
    }
}

/// The type a value expression constructs, when it constructs one outright.
pub(in crate::parser::go) fn constructed_type(value: &Node, source: &str) -> Option<String> {
    match value.kind() {
        // `&Order{}` — the address of a composite literal.
        "unary_expression" => constructed_type(&value.child_by_field_name("operand")?, source),
        "composite_literal" => Some(
            strip_decoration(node_text(&value.child_by_field_name("type")?, source)).to_string(),
        ),
        // `new(Order)`, whose sole argument is a type rather than a value.
        "call_expression" => {
            let function = value.child_by_field_name("function")?;
            if node_text(&function, source) != "new" {
                return None;
            }
            let arguments = value.child_by_field_name("arguments")?;
            let mut cursor = arguments.walk();
            let first = arguments.children(&mut cursor).find(|c| c.is_named())?;
            Some(strip_decoration(node_text(&first, source)).to_string())
        }
        _ => None,
    }
}

/// The one named child of a node, or `None` when there are none or several.
fn only_child<'a>(node: &Node<'a>) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    let mut named = node.children(&mut cursor).filter(|c| c.is_named());
    let first = named.next()?;
    named.next().is_none().then_some(first)
}

/// Whether a spec belongs to the file rather than to some function in it.
fn is_package_level(spec: &Node) -> bool {
    let mut current = spec.parent();
    while let Some(node) = current {
        match node.kind() {
            "source_file" => return true,
            "function_declaration" | "method_declaration" | "func_literal" | "block" => {
                return false
            }
            _ => current = node.parent(),
        }
    }
    false
}

/// `(field name, base type name)` for every named field of a struct type.
/// Embedded fields declare no name of their own; the type *is* the name, so
/// `s.Mutex` reaches `Mutex` the same way a named field would.
fn struct_fields(struct_type: &Node, source: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut cursor = struct_type.walk();
    for list in struct_type.children(&mut cursor) {
        if list.kind() != "field_declaration_list" {
            continue;
        }
        let mut fields = list.walk();
        for field in list.children(&mut fields) {
            if field.kind() != "field_declaration" {
                continue;
            }
            let Some(type_node) = field.child_by_field_name("type") else {
                continue;
            };
            let type_name = strip_decoration(node_text(&type_node, source)).to_string();
            let mut names = field.walk();
            let named: Vec<_> = field
                .children_by_field_name("name", &mut names)
                .map(|n| node_text(&n, source).to_string())
                .collect();
            if named.is_empty() {
                out.push((type_name.clone(), type_name));
            } else {
                for name in named {
                    out.push((name, type_name.clone()));
                }
            }
        }
    }
    out
}
