//! Which type a written name carries, as far as one file can tell.
//!
//! `repo_->save(order)` names no type anywhere in it. Spelled from the
//! receiver's text the call target is `repo_::save`, which resolves to
//! nothing — so a class that reaches all of its collaborators through
//! fields draws as depending on none of them (§E2). But the file usually
//! *did* say: a member function states its class, a class body states
//! every field's type, a signature states every parameter's, and
//! `Repository* r = …` states its own. Following those hops turns
//! `repo_->save` into `Repository::save`, which the resolver can find.
//!
//! Two tables with different lifetimes. [`TypeIndex`] is built once per
//! file from its declarations; [`Locals`] is built per body from its
//! signature and its own declarations.
//!
//! Both stop at the file edge, and this costs more in C++ than it did in
//! Go: the class body usually lives in a header and the definitions in a
//! `.cpp`, so a translation unit alone often cannot say what `repo_`
//! holds. When it cannot, the receiver's text is left alone — a
//! receiver-shaped ghost is honest, and a guessed type is not (§E3).
//!
//! [`TypeIndex`] also answers a second question, which is C++-specific: a
//! bare `helper()` inside a member function is a call to `Class::helper`
//! only if `Class` has such a member, and a call to a free function
//! otherwise. Nothing in the call site distinguishes them, so
//! [`TypeIndex::declares_member`] is asked before a bare name is
//! qualified — the alternative, qualifying unconditionally the way the
//! Java parser can afford to, invents `Class::helper` for every free
//! function a method calls.

use super::super::helpers::{declared_name, function_declarator, qualified_parts};
use crate::parser::language_parser::node_text;
use std::collections::{HashMap, HashSet};
use tree_sitter::Node;

/// What a file's own declarations say about types and membership.
#[derive(Debug, Default)]
pub(in crate::parser::cpp) struct TypeIndex {
    /// `(class name, field name)` → the field's base type.
    fields: HashMap<(String, String), String>,
    /// `(class name, member name)` for every member this file declares or
    /// defines, data and functions alike.
    members: HashSet<(String, String)>,
    /// Namespace-scope variables → base type.
    globals: HashMap<String, String>,
}

impl TypeIndex {
    /// Read every class body and every out-of-line definition in one file.
    pub(in crate::parser::cpp) fn build(root: Node, source: &str) -> Self {
        let mut index = Self::default();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            match node.kind() {
                "class_specifier" | "struct_specifier" | "union_specifier" => {
                    index.record_class(&node, source)
                }
                "function_definition" => index.record_out_of_line(&node, source),
                "declaration" => index.record_global(&node, source),
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
    /// the class that holds it.
    pub(in crate::parser::cpp) fn field_type(&self, owner: &str, field: &str) -> Option<&str> {
        self.fields
            .get(&(owner.to_string(), field.to_string()))
            .map(String::as_str)
    }

    /// Whether this file says `owner` has a member called `name`.
    pub(in crate::parser::cpp) fn declares_member(&self, owner: &str, name: &str) -> bool {
        self.members.contains(&(owner.to_string(), name.to_string()))
    }

    /// The declared type of a namespace-scope name.
    pub(in crate::parser::cpp) fn global_type(&self, name: &str) -> Option<&str> {
        self.globals.get(name).map(String::as_str)
    }

    /// Record a namespace-scope `Type name = …;`, so a call through a
    /// file-level object resolves the same way a local does. A declaration
    /// inside a function body is a local, and [`Locals`] reads it with the
    /// scope it actually has.
    fn record_global(&mut self, node: &Node, source: &str) {
        if !is_namespace_scope(node) {
            return;
        }
        let Some(type_node) = node.child_by_field_name("type") else {
            return;
        };
        let type_name = base_type(node_text(&type_node, source)).to_string();
        let mut cursor = node.walk();
        for declarator in node.children_by_field_name("declarator", &mut cursor) {
            let Some(name_node) = declared_name(&declarator) else {
                continue;
            };
            self.globals
                .insert(node_text(&name_node, source).to_string(), type_name.clone());
        }
    }

    fn record_class(&mut self, node: &Node, source: &str) {
        let Some(owner) = node
            .child_by_field_name("name")
            .map(|n| class_name(&n, source))
        else {
            return;
        };
        let Some(body) = node.child_by_field_name("body") else {
            return;
        };
        let mut cursor = body.walk();
        for member in body.children(&mut cursor) {
            self.record_member(&owner, &member, source);
        }
    }

    /// One entry of a class body: a data member gets a type, a member
    /// function gets membership only.
    fn record_member(&mut self, owner: &str, node: &Node, source: &str) {
        if !matches!(node.kind(), "field_declaration" | "declaration" | "function_definition") {
            return;
        }
        let Some(declarator) = node.child_by_field_name("declarator") else {
            return;
        };
        let Some(name_node) = declared_name(&declarator) else {
            return;
        };
        let (_, name) = qualified_parts(&name_node, source);
        self.members.insert((owner.to_string(), name.clone()));
        if function_declarator(&declarator).is_some() {
            return;
        }
        let Some(type_node) = node.child_by_field_name("type") else {
            return;
        };
        self.fields.insert(
            (owner.to_string(), name),
            base_type(node_text(&type_node, source)).to_string(),
        );
    }

    /// `void Order::total() { … }` written outside the class body — the
    /// only evidence a `.cpp` file has that `Order` owns `total`.
    fn record_out_of_line(&mut self, node: &Node, source: &str) {
        let Some(declarator) = node.child_by_field_name("declarator") else {
            return;
        };
        let Some(name_node) = declared_name(&declarator) else {
            return;
        };
        let (Some(scope), name) = qualified_parts(&name_node, source) else {
            return;
        };
        // `app::run` names a namespace, not a class; the last segment is
        // the nearest enclosing scope either way, and a namespace that
        // never appears as a receiver costs nothing.
        let owner = scope.rsplit("::").next().unwrap_or(&scope).to_string();
        self.members.insert((owner, name));
    }
}

/// The names bound inside one body, and the type each carries.
#[derive(Debug, Default)]
pub(in crate::parser::cpp) struct Locals {
    by_name: HashMap<String, String>,
}

impl Locals {
    /// Collect what a callable's signature declares and what its body
    /// declares. Both halves are exact where C++ writes a type outright,
    /// and silent where it writes `auto` — chasing a return type through
    /// a call is the guess §E3 says not to make.
    pub(in crate::parser::cpp) fn for_body(declarator: &Node, body: &Node, source: &str) -> Self {
        let mut locals = Self::default();
        if let Some(function) = function_declarator(declarator) {
            if let Some(list) = function.child_by_field_name("parameters") {
                locals.record_declarations(&list, source);
            }
        }
        locals.record_declarations(body, source);
        locals
    }

    /// The type of a name in scope, or `None` when this file never said.
    pub(in crate::parser::cpp) fn type_of(&self, name: &str) -> Option<&str> {
        self.by_name.get(name).map(String::as_str)
    }

    /// Walk a subtree for the declaration forms that state a type.
    ///
    /// Lambda bodies are walked too, and their bindings land in the
    /// enclosing table. C++ scoping does not work that way, but a name
    /// bound in a lambda and reused for something else in the same
    /// function is a shadowing pattern review catches, while the cost of
    /// the alternative — losing every typed receiver inside every lambda
    /// — is paid on ordinary code.
    fn record_declarations(&mut self, root: &Node, source: &str) {
        let mut stack = vec![*root];
        while let Some(node) = stack.pop() {
            if matches!(
                node.kind(),
                "declaration" | "parameter_declaration" | "optional_parameter_declaration"
                    | "for_range_loop"
            ) {
                self.record_declaration(&node, source);
            }
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                stack.push(child);
            }
        }
    }

    fn record_declaration(&mut self, node: &Node, source: &str) {
        let Some(type_node) = node.child_by_field_name("type") else {
            return;
        };
        // `auto x = …` states nothing this pass can read.
        if type_node.kind() == "placeholder_type_specifier" {
            return;
        }
        let type_name = base_type(node_text(&type_node, source)).to_string();
        let mut cursor = node.walk();
        for declarator in node.children_by_field_name("declarator", &mut cursor) {
            let Some(name_node) = declared_name(&declarator) else {
                continue;
            };
            // A local function declaration binds no value.
            if function_declarator(&declarator).is_some() {
                continue;
            }
            self.by_name.insert(
                node_text(&name_node, source).to_string(),
                type_name.clone(),
            );
        }
    }
}

/// The name a type text resolves to once C++'s decoration is removed:
/// `const std::vector<Item>&` → `std::vector`, `Repository*` →
/// `Repository`. Qualifiers are kept, because `std::` is what tells the
/// standard library's `size()` from a project class's.
pub(in crate::parser::cpp) fn base_type(text: &str) -> &str {
    let text = text.trim();
    let text = text.split('<').next().unwrap_or(text);
    let mut out = text;
    for keyword in ["const ", "constexpr ", "volatile ", "mutable ", "static "] {
        out = out.trim_start().strip_prefix(keyword).unwrap_or(out);
    }
    out.trim().trim_end_matches(['*', '&', ' ']).trim()
}

/// Whether a declaration belongs to the file rather than to a body in it.
fn is_namespace_scope(node: &Node) -> bool {
    let mut current = node.parent();
    while let Some(parent) = current {
        match parent.kind() {
            "translation_unit" => return true,
            "compound_statement" | "function_definition" | "lambda_expression"
            | "field_declaration_list" | "for_statement" | "for_range_loop" => return false,
            _ => current = parent.parent(),
        }
    }
    false
}

/// The name of a class as written after `class` / `struct` — the plain
/// identifier, or the template's own name for `Repository<int>`.
fn class_name(node: &Node, source: &str) -> String {
    if node.kind() == "template_type" {
        if let Some(name) = node.child_by_field_name("name") {
            return node_text(&name, source).to_string();
        }
    }
    node_text(node, source).to_string()
}
