//! Lightweight local-type inference for TypeScript function bodies.
//!
//! The TypeScript sibling of [`crate::parser::rust::bodies::inference`], and it exists
//! for the same reason: without it, `client.run(q)` emits the callee
//! `client.run`, which matches no entity, so the resolver drops the edge
//! entirely (a qualified callee that misses `typed_method_to_id` is
//! deliberately *not* retried as a bare name — see `graph.rs`). The type is
//! written down at the declaration; reading it is exact rather than a guess.
//!
//! Three sources, in the order the call extractor consults them:
//!
//! 1. **Parameters** — `fetch(q: Query)` types `q` as `Query`.
//! 2. **Locals** — `const c: ApiClient = …`, `const c = new ApiClient()`.
//! 3. **Class members** — declared fields *and* constructor parameter
//!    properties (`constructor(private readonly api: ApiClient)`), which is
//!    how most dependency-injected TypeScript declares its collaborators.
//!    These let `this.api.run()` resolve to `ApiClient.run`.
//!
//! What is deliberately *not* inferred: anything requiring a return type to
//! be looked up (`const c = makeClient()`), or a receiver with a call or an
//! index in the path (`a.b().c`). Those yield no type rather than a guess.

use super::super::helpers::extract_type_text;
use crate::parser::language_parser::{find_child_by_kind, node_text};
use std::collections::HashMap;
use tree_sitter::Node;

/// Reduce a written-out TypeScript type to the bare name the entity index is
/// keyed by, or `None` when the text names nothing a receiver could be typed
/// as.
///
/// `Map<string, Entry>` → `Map` (the receiver *is* a Map), `ns.Widget` →
/// `Widget`, `Widget | null` → `Widget`. Rejected: primitives (`string`,
/// `number`) because TypeScript writes them lowercase, arrays (`Widget[]` is
/// an Array, not a Widget), object literal types, function types, and unions
/// of two real types — a receiver that could be either is not typed.
pub(super) fn base_type_name(text: &str) -> Option<String> {
    let cleaned = text.trim().trim_start_matches("readonly ").trim();
    // Drop the nullable arms of a union; anything else left over means the
    // receiver has more than one possible type and we decline.
    let mut arms = cleaned
        .split('|')
        .map(str::trim)
        .filter(|a| !matches!(*a, "null" | "undefined" | ""));
    let only = arms.next()?;
    if arms.next().is_some() {
        return None;
    }
    // `Widget[]`, `readonly Widget[]` and `(A | B)[]` are all Array receivers.
    if only.ends_with(']') {
        return None;
    }
    // Take the head of a generic application: `Map<string, Entry>` → `Map`.
    let head = only.split('<').next()?.trim();
    // Namespaced types (`models.Widget`) are keyed by their last segment,
    // exactly as the entity index keys them.
    let last = head.rsplit('.').next()?.trim();
    let name: String = last
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
        .collect();
    if name.len() != last.len() {
        // Something non-identifier followed — an object literal, a function
        // type, an indexed access. Not a name we can key on.
        return None;
    }
    name.chars().next()?.is_uppercase().then_some(name)
}

/// `class name → (member name → declared type)` for every class and interface
/// declared in the file.
///
/// Keyed by container name because a method only knows its enclosing type by
/// name. Two same-named classes in one file are a compile error, so the inner
/// map is unambiguous.
pub(in crate::parser::typescript) fn collect_class_members(
    root: &Node,
    source: &str,
) -> HashMap<String, HashMap<String, String>> {
    let mut out: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut stack: Vec<Node> = vec![*root];
    while let Some(node) = stack.pop() {
        if is_type_container(node.kind()) {
            collect_one_container(&node, source, &mut out);
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }
    out
}

fn is_type_container(kind: &str) -> bool {
    matches!(
        kind,
        "class_declaration" | "abstract_class_declaration" | "interface_declaration"
    )
}

/// Read one class/interface body into the member index.
fn collect_one_container(
    node: &Node,
    source: &str,
    out: &mut HashMap<String, HashMap<String, String>>,
) {
    let Some(name) = node
        .child_by_field_name("name")
        .map(|n| node_text(&n, source).to_string())
    else {
        return;
    };
    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let members = out.entry(name).or_default();
    let mut cursor = body.walk();
    for member in body.children(&mut cursor) {
        match member.kind() {
            "public_field_definition" | "property_signature" => {
                insert_declared_member(&member, source, members);
            }
            // `constructor(private readonly api: ApiClient)` declares a field
            // in its parameter list. The grammar keeps those as ordinary
            // parameters carrying an `accessibility_modifier`.
            "method_definition" => collect_parameter_properties(&member, source, members),
            _ => {}
        }
    }
}

/// Record `name: Type` from a field or property signature node.
fn insert_declared_member(node: &Node, source: &str, members: &mut HashMap<String, String>) {
    let (Some(name), Some(type_ann)) = (
        node.child_by_field_name("name"),
        node.child_by_field_name("type"),
    ) else {
        return;
    };
    if let Some(base) = base_type_name(&extract_type_text(&type_ann, source)) {
        members.insert(node_text(&name, source).to_string(), base);
    }
}

/// Pull TypeScript parameter properties out of a constructor's signature.
fn collect_parameter_properties(
    method: &Node,
    source: &str,
    members: &mut HashMap<String, String>,
) {
    let is_constructor = method
        .child_by_field_name("name")
        .map(|n| node_text(&n, source) == "constructor")
        .unwrap_or(false);
    if !is_constructor {
        return;
    }
    let Some(params) = method.child_by_field_name("parameters") else {
        return;
    };
    let mut cursor = params.walk();
    for param in params.children(&mut cursor) {
        if find_child_by_kind(&param, "accessibility_modifier").is_none() {
            continue;
        }
        let (Some(pattern), Some(type_ann)) = (
            param.child_by_field_name("pattern"),
            param.child_by_field_name("type"),
        ) else {
            continue;
        };
        if let Some(base) = base_type_name(&extract_type_text(&type_ann, source)) {
            members.insert(node_text(&pattern, source).to_string(), base);
        }
    }
}

/// Everything the call extractor knows about types in one function body: its
/// bindings, the file's class members, and the enclosing class's name.
pub(super) struct TypeEnv<'a> {
    /// Variable → type, from parameters and `const`/`let` declarations.
    /// Locals are inserted after parameters, so a shadowing local wins.
    pub locals: HashMap<String, String>,
    /// Class/interface name → (member → type), for walking dotted receivers.
    pub members: &'a HashMap<String, HashMap<String, String>>,
    /// The enclosing class's name, if the caller is a method.
    pub self_type: Option<&'a str>,
}

impl TypeEnv<'_> {
    /// Type of a receiver expression, walking a dotted path.
    ///
    /// `this.api.run()` types `this` as the enclosing class, follows `api` to
    /// `ApiClient`, and the caller emits `ApiClient.run`. Every step reads a
    /// *declared* type, so a resolved path is exact; a head with no
    /// declaration, or a call or index anywhere in the path, yields `None`
    /// rather than a guess.
    pub(super) fn resolve_receiver(&self, receiver: &str) -> Option<String> {
        let mut segments = receiver.split('.');
        let mut base = self.head_type(segments.next()?.trim())?;
        for segment in segments {
            let segment = segment.trim();
            // A call or index in the path is an expression whose type cannot
            // be read off a declaration. Optional chaining (`a?.b`) leaves a
            // trailing `?` on the previous segment, which the split above
            // already isolated — see the `?` guard below.
            if segment.contains('(') || segment.contains('[') || segment.contains('?') {
                return None;
            }
            base = self.members.get(&base)?.get(segment)?.clone();
        }
        Some(base)
    }

    /// Type of the first segment of a receiver: `this`, or a variable bound by
    /// a parameter or a declaration.
    fn head_type(&self, head: &str) -> Option<String> {
        if head == "this" {
            return self.self_type.map(str::to_string);
        }
        self.locals.get(head).cloned()
    }
}

/// Types of a function's parameters, read off the signature.
///
/// Destructuring patterns bind no single name a type can be attributed to and
/// are skipped rather than guessed.
pub(super) fn infer_param_types(params: &Node, source: &str) -> HashMap<String, String> {
    let mut types = HashMap::new();
    let mut cursor = params.walk();
    for child in params.children(&mut cursor) {
        if !matches!(child.kind(), "required_parameter" | "optional_parameter") {
            continue;
        }
        let (Some(pattern), Some(type_ann)) = (
            child.child_by_field_name("pattern"),
            child.child_by_field_name("type"),
        ) else {
            continue;
        };
        if pattern.kind() != "identifier" {
            continue;
        }
        if let Some(base) = base_type_name(&extract_type_text(&type_ann, source)) {
            types.insert(node_text(&pattern, source).to_string(), base);
        }
    }
    types
}

/// Scan a function body for `const`/`let`/`var` declarations, mapping each
/// variable to its type from either an explicit annotation (`const c: Api = …`)
/// or a constructor call (`const c = new Api()`).
///
/// Nested function and arrow bodies are walked too: their `const`s are in
/// scope for the calls attributed to this caller, because the TypeScript
/// dispatcher does not register anonymous callbacks as their own entities —
/// their calls belong to the enclosing function, so their bindings must be
/// visible here as well.
pub(super) fn infer_local_types(body: &Node, source: &str) -> HashMap<String, String> {
    let mut types = HashMap::new();
    let mut stack: Vec<Node> = vec![*body];
    while let Some(node) = stack.pop() {
        if node.kind() == "variable_declarator" {
            if let Some((name, ty)) = declarator_type(&node, source) {
                types.entry(name).or_insert(ty);
            }
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }
    types
}

/// `(variable name, type)` for one `variable_declarator`, when both can be
/// read off the declaration.
fn declarator_type(node: &Node, source: &str) -> Option<(String, String)> {
    let name_node = node.child_by_field_name("name")?;
    if name_node.kind() != "identifier" {
        return None; // destructuring binds no single name
    }
    let name = node_text(&name_node, source).to_string();
    if let Some(type_ann) = node.child_by_field_name("type") {
        if let Some(base) = base_type_name(&extract_type_text(&type_ann, source)) {
            return Some((name, base));
        }
    }
    let value = node.child_by_field_name("value")?;
    if value.kind() != "new_expression" {
        return None;
    }
    let ctor = value.child_by_field_name("constructor")?;
    let base = base_type_name(node_text(&ctor, source))?;
    Some((name, base))
}
