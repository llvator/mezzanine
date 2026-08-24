//! Lightweight local-variable type inference for Rust function bodies.
//!
//! Feeds the call-extraction pass so that `walker.do_thing()` can resolve
//! `walker` to its declared type and qualify the call as `FileWalker::do_thing`.

use crate::parser::language_parser::node_text;
use crate::parser::rust_type_names::{base_type_name, strip_generics};
use std::collections::HashMap;
use tree_sitter::Node;

/// Everything the call extractor knows about types in one function body:
/// its bindings, the file's struct fields, and the enclosing impl's type.
pub(super) struct TypeEnv<'a> {
    /// Variable → type, from parameters and `let` bindings. Locals shadow
    /// parameters because they are inserted last.
    pub locals: HashMap<String, String>,
    /// Struct name → (field → type), for walking dotted receivers.
    pub fields: &'a HashMap<String, HashMap<String, String>>,
    /// The enclosing `impl` block's type name, if any.
    pub self_type: Option<&'a str>,
}

/// How far a dotted receiver could be typed from declarations in *this* file.
///
/// The split exists because struct fields are collected per file while the
/// structs themselves are spread across the tree: `entity.kind.is_callable()`
/// types `entity` as `CodeEntity` from its parameter, then stalls, because
/// `CodeEntity`'s fields are declared in `src/models/entity.rs`. Carrying the
/// stall point out of the parser lets the analyzer finish the walk against the
/// whole tree's structs (AN-012) instead of dropping the edge.
pub(super) struct ReceiverPath {
    /// Type of the last segment that could be read off a declaration.
    pub base: String,
    /// Field segments still unwalked, in order. Empty when the receiver is
    /// fully typed here.
    pub rest: Vec<String>,
}

impl TypeEnv<'_> {
    /// Type of a receiver expression, walking a dotted path (AN-010).
    ///
    /// `ctx.result` resolves `ctx` to its parameter type `ExtractCtx`, then
    /// follows the `result` field to `ParseResult` — so `ctx.result.add_entity()`
    /// becomes `ParseResult::add_entity` instead of the unmatched
    /// `ctx.result::add_entity`. This is the general form of the receiver
    /// problem; `self.<field>` is just the case where the head is `self`.
    ///
    /// Every step is a *declared* type, so a resolved path is exact. A head
    /// with no declaration to read, or a call or index inside the path, yields
    /// `None` rather than a guess. A field this file's structs cannot type is
    /// the one recoverable case: it comes back in `rest` for the analyzer to
    /// finish, and stays unresolved if the analyzer cannot either.
    pub(super) fn resolve_receiver(&self, receiver: &str) -> Option<ReceiverPath> {
        let mut segments = receiver.split('.');
        let mut base = self.head_type(segments.next()?.trim())?;
        let mut rest: Vec<String> = Vec::new();
        for segment in segments {
            // A field access wraps onto the next line often enough to matter
            // (`ctx.result\n    .entities`), and the indentation rides along
            // in the receiver's source text. A field name never contains
            // whitespace, so trimming can only turn a miss into a match.
            let segment = segment.trim();
            // A call or index in the path (`a.b().c`, `a.b[0].c`) is an
            // expression whose type we cannot read off a declaration.
            if segment.contains('(') || segment.contains('[') {
                return None;
            }
            // Once one hop is deferred every later hop is too — the type it
            // would be looked up on isn't known here either.
            let known = rest
                .is_empty()
                .then(|| self.fields.get(&base).and_then(|f| f.get(segment)))
                .flatten();
            match known {
                Some(ty) => base = ty.clone(),
                None => rest.push(segment.to_string()),
            }
        }
        Some(ReceiverPath { base, rest })
    }

    /// Type of the first segment of a receiver: `self`, or a variable bound
    /// by a parameter or a `let`.
    fn head_type(&self, head: &str) -> Option<String> {
        if head == "self" {
            return self.self_type.map(str::to_string);
        }
        match self.locals.get(head)?.as_str() {
            // `let mut g = Self::new()` binds the literal `Self`.
            "Self" => self.self_type.map(str::to_string),
            bound => Some(bound.to_string()),
        }
    }
}

/// Types of a function's parameters, read off the signature (AN-009).
///
/// Parameters are the single largest remaining source of unresolved call
/// edges: `fn run(client: &mut LspClient)` followed by `client.shutdown()`
/// used to emit the callee `client::shutdown`, which matches nothing, so
/// `impact` never reported the dependency. The type is *written down* right
/// there — reading it is exact, not a guess.
///
/// `self` is deliberately absent: the call extractor already qualifies
/// `self.foo()` through `self_type`.
pub(super) fn infer_param_types(fn_node: &Node, source: &str) -> HashMap<String, String> {
    let mut types = HashMap::new();
    let Some(params) = fn_node.child_by_field_name("parameters") else {
        return types;
    };
    let mut cursor = params.walk();
    for child in params.named_children(&mut cursor) {
        if child.kind() != "parameter" {
            continue;
        }
        let (Some(pattern), Some(type_node)) = (
            child.child_by_field_name("pattern"),
            child.child_by_field_name("type"),
        ) else {
            continue;
        };
        let name = match pattern.kind() {
            "identifier" => node_text(&pattern, source).to_string(),
            // `fn f(mut buf: Vec<u8>)` — same shape as `let mut`.
            "mut_pattern" => match pattern.named_child(0) {
                Some(inner) if inner.kind() == "identifier" => {
                    node_text(&inner, source).to_string()
                }
                _ => continue,
            },
            // Destructuring patterns bind no single name we can attribute a
            // type to. Skipped rather than guessed.
            _ => continue,
        };
        if let Some(base) = base_type_name(node_text(&type_node, source)) {
            types.insert(name, base);
        }
    }
    types
}

/// Scan a function body for `let` declarations and infer variable → type
/// mappings from:
/// - Explicit annotations: `let x: Type = ...` → `x` is `Type`
/// - Constructor calls: `let x = Type::new(...)` → `x` is `Type`
/// - Struct literals: `let x = Type { ... }` → `x` is `Type`
pub(super) fn infer_local_types(body: &Node, source: &str) -> HashMap<String, String> {
    let mut types = HashMap::new();
    let mut stack: Vec<Node> = vec![*body];

    while let Some(node) = stack.pop() {
        if node.kind() == "let_declaration" {
            // Extract variable name from pattern.
            // AN-006: `let mut x = …` wraps the binding in a `mut_pattern`, so
            // filtering to bare `identifier` silently dropped every mutable
            // local — ~900 of them in this repo alone. A dropped binding costs
            // its type, and the call extractor then falls back to the receiver's
            // source text (`graph::detect_smells`), which resolves to nothing.
            let var_name = node
                .child_by_field_name("pattern")
                .and_then(|p| match p.kind() {
                    "identifier" => Some(p),
                    "mut_pattern" => p.named_child(0).filter(|c| c.kind() == "identifier"),
                    _ => None,
                })
                .map(|p| node_text(&p, source).to_string());

            if let Some(name) = var_name {
                // Try explicit type annotation: `let x: Type = ...`
                if let Some(type_node) = node.child_by_field_name("type") {
                    // Shares the normalizer with parameter inference, so
                    // `let c: &'a mut crate::lsp::Client` is handled the same
                    // way as the equivalent parameter.
                    if let Some(base) = base_type_name(node_text(&type_node, source)) {
                        types.insert(name.clone(), base);
                    }
                }

                // Try value-based inference (only if not already typed).
                if !types.contains_key(&name) {
                    if let Some(value) = node.child_by_field_name("value") {
                        match value.kind() {
                            // `Type::new(...)` → call_expression with scoped_identifier
                            "call_expression" => {
                                if let Some(func) = value.child_by_field_name("function") {
                                    if func.kind() == "scoped_identifier" {
                                        let text = node_text(&func, source);
                                        let stripped = strip_generics(&text);
                                        let segments: Vec<&str> = stripped.split("::").collect();
                                        // Take the type segment (second-to-last)
                                        if segments.len() >= 2 {
                                            let type_seg = segments[segments.len() - 2].to_string();
                                            if type_seg.starts_with(|c: char| c.is_uppercase()) {
                                                types.insert(name, type_seg);
                                            }
                                        }
                                    }
                                }
                            }
                            // `Type { field: value }` → struct_expression
                            "struct_expression" => {
                                if let Some(name_node) = value.child_by_field_name("name") {
                                    let type_text = node_text(&name_node, source).to_string();
                                    let base = strip_generics(&type_text);
                                    // Take the last segment for qualified paths
                                    let last = base.split("::").last().unwrap_or("").to_string();
                                    if last.starts_with(|c: char| c.is_uppercase()) {
                                        types.insert(name, last);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        // Recurse into children (but NOT into nested function bodies —
        // those have their own scope).
        if !matches!(node.kind(), "closure_expression" | "function_item") || node == *body {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                stack.push(child);
            }
        }
    }
    types
}
