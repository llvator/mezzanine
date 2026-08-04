//! Groovy parser — entry point and entity-extraction dispatcher.
//!
//! Submodules group the parsing logic by concern:
//! - [`containers`] — class, interface, enum
//! - [`callables`] — methods, constructors, `def`-style functions
//! - [`fields`] — class fields and `@Field` script-scope state
//! - [`imports`] — `import` declarations
//! - [`calls`] — call-site relationships, control-flow arms, closure bodies
//! - [`complexity`] — per-callable cyclomatic / cognitive / nesting metrics
//! - [`flow`] — synthetic Branch and Loop entity emission
//! - [`javadoc`] — `/** ... */` extraction
//! - [`helpers`] — visibility, modifiers, parameters, declared types
//! - [`stdlib`] — built-in name table for call filtering
//! - [`types`] — `UsesType` edge extraction from signature/field types

mod callables;
mod calls;
mod complexity;
mod containers;
mod fields;
mod flow;
mod helpers;
mod imports;
mod javadoc;
mod stdlib;
mod types;

#[cfg(test)]
mod tests;

use super::language_parser::{node_text, node_to_span, LanguageParser, ParseResult};
use crate::models::file_info::Language;
use crate::models::{CodeEntity, EntityKind, Relationship, RelationshipKind};
use anyhow::Result;
use std::path::Path;
use tree_sitter::{Node, Parser, Tree};

pub struct GroovyParser {
    parser: Parser,
}

/// Shared context threaded through the entity-extraction walk.
pub(super) struct ExtractCtx<'a> {
    pub source: &'a str,
    pub path: &'a Path,
    pub package: &'a str,
    pub result: &'a mut ParseResult,
}

/// Bridge `tree-sitter-groovy`'s `LanguageFn` (built against the
/// 0.23+ `tree-sitter-language` ABI) onto our `tree-sitter` 0.22
/// `Language` type. Both are `repr(transparent)` over the same
/// `*const TSLanguage` pointer the C grammar returns, so a transmute
/// of the raw return value is sound. We go through `into_raw` rather
/// than declaring our own `extern "C" fn tree_sitter_groovy()` so the
/// crate's static C library stays linked — `LanguageFn` carries the
/// symbol reference that prevents dead-strip from dropping it.
fn groovy_language() -> tree_sitter::Language {
    let raw_fn = tree_sitter_groovy::LANGUAGE.into_raw();
    unsafe { std::mem::transmute(raw_fn()) }
}

impl GroovyParser {
    pub fn new() -> Self {
        let mut parser = Parser::new();
        parser
            .set_language(&groovy_language())
            .expect("Failed to set Groovy language");
        Self { parser }
    }

    fn parse_tree(&mut self, content: &str) -> Result<Tree> {
        self.parser
            .parse(content, None)
            .ok_or_else(|| anyhow::anyhow!("Failed to parse Groovy code"))
    }

    /// Parse a Groovy expression / statement fragment in isolation and
    /// return only the relationships it produces — no entities. Used by
    /// other parsers that embed Groovy snippets (e.g. Impex's `#%`
    /// script lines per IM-003) so they can attribute the snippet's
    /// calls / writes back to whatever caller they're embedded in.
    ///
    /// The fragment is parsed as a free-standing program (top-level
    /// statements, no enclosing class). `caller_id` is used as the
    /// relationship `source_id` and as the parent identifier for any
    /// synthetic locals — the embedding parser typically passes its
    /// file's module entity id. `current_branch` carries the enclosing
    /// arm path (e.g. an Impex `#% if:` Branch) so calls from inside a
    /// gated `#%` body inherit the Branch's metadata.
    pub fn parse_expression(
        source: &str,
        path: &Path,
        caller_id: &str,
        current_branch: Option<&str>,
    ) -> Result<Vec<Relationship>> {
        let mut parser = Self::new();
        let tree = parser.parse_tree(source)?;
        let mut result = ParseResult::new();
        let mut call_order = 0u32;
        let mut arm_counter = 0u32;
        let mut loop_counter = 0u32;
        let mut ctx = calls::CallCtx {
            source,
            path,
            caller_id,
            caller_name: "",
            parent_class: None,
            call_order: &mut call_order,
            arm_counter: &mut arm_counter,
            loop_counter: &mut loop_counter,
            result: &mut result,
        };
        calls::extract_calls(&tree.root_node(), &mut ctx, current_branch);
        Ok(result.relationships)
    }
}

/// True when the program node holds at least one statement that
/// requires a caller for call extraction — top-level `try_statement`,
/// raw `method_invocation` / `expression_statement`, or a
/// `local_variable_declaration` (`def x = init()` or `@Field …`).
/// Pure class-only files (no top-level statements) skip the synthetic
/// container so they parse identically to a Java equivalent.
fn is_script_file(root: Node) -> bool {
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        match child.kind() {
            "class_declaration"
            | "interface_declaration"
            | "enum_declaration"
            | "import_declaration"
            | "package_declaration"
            | "block_comment"
            | "comment"
            | "line_comment" => {}
            _ => return true,
        }
    }
    false
}

/// Build the synthetic file-level container for a Groovy script. All
/// top-level statements (and `@Field` declarations) attribute to this
/// entity, so the script's body shows up in the graph as one
/// connected node rather than as a heap of dangling calls.
/// The container also carries the script's own complexity metrics. A
/// Gradle build file keeps nearly all of its logic in top-level
/// statements and closures rather than in declared methods, so without
/// this the file's `composite_score` stays 0 and `quality` / `hotspots`
/// — which filter on `score > 0` — report a build script as clean when
/// they simply never measured it. `compute_complexity` stops at nested
/// definitions, so a class declared inside the script keeps its own
/// numbers instead of being folded in here.
fn build_script_container(root: Node, path: &Path, source: &str) -> CodeEntity {
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("script")
        .to_string();
    let span = node_to_span(&root);
    let mut entity = CodeEntity::new(&name, EntityKind::File, path, span);
    entity.tags.insert("groovy_script".to_string());
    entity.qualified_name = name;

    let (cc, nesting, cog) = complexity::compute_complexity(&root, source);
    entity.metrics.loc = (entity.span.end.line - entity.span.start.line + 1) as u32;
    entity.metrics.cyclomatic = Some(cc);
    entity.metrics.max_nesting = Some(nesting);
    entity.metrics.cognitive_complexity = Some(cog);
    entity
}

/// Extract the package name from the root program node, so containers can
/// be qualified the same way the Java parser qualifies its own — a Groovy
/// class and a Java class in one package share a namespace at runtime and
/// should share one in the graph.
fn extract_package(root: Node, source: &str) -> String {
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if child.kind() != "package_declaration" {
            continue;
        }
        let mut inner = child.walk();
        for c in child.children(&mut inner) {
            if c.kind() == "scoped_identifier" || c.kind() == "identifier" {
                return node_text(&c, source).to_string();
            }
        }
    }
    String::new()
}

/// Walk a node's children dispatching each to the right extractor.
/// Container bodies recurse with the new entity id as parent_id;
/// method/function bodies recurse via `handle_callable` which also
/// runs call-extraction; `@Field`-annotated script-scope
/// declarations route through the script container rather than
/// becoming generic locals; everything else recurses through.
fn extract_entities(node: Node, parent_id: Option<&str>, ctx: &mut ExtractCtx<'_>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "class_declaration" => add_container(&child, parent_id, ctx, containers::parse_class),
            "interface_declaration" => {
                add_container(&child, parent_id, ctx, containers::parse_interface)
            }
            "enum_declaration" => add_container(&child, parent_id, ctx, containers::parse_enum),
            "method_declaration" => {
                callables::handle_callable(&child, parent_id, ctx, callables::parse_method);
            }
            "function_definition" => {
                callables::handle_callable(&child, parent_id, ctx, callables::parse_function);
            }
            "constructor_declaration" => {
                callables::handle_callable(&child, parent_id, ctx, callables::parse_constructor);
            }
            "field_declaration" => {
                for entity in fields::parse_class_field(&child, ctx.source, ctx.path, parent_id) {
                    ctx.result.add_entity(entity);
                }
            }
            "local_variable_declaration" if fields::has_field_annotation(&child, ctx.source) => {
                handle_field_decl(&child, parent_id, ctx);
            }
            "import_declaration" => {
                if let Some(import) = imports::parse_import(&child, ctx.source) {
                    ctx.result.add_import(import);
                }
            }
            "package_declaration" => {}
            _ => extract_entities(child, parent_id, ctx),
        }
    }
}

/// Parse a container, register it, and recurse into its body with the
/// new entity id as parent_id.
fn add_container(
    node: &Node,
    parent_id: Option<&str>,
    ctx: &mut ExtractCtx<'_>,
    parse: fn(&Node, &str, &Path, Option<&str>, &str) -> Option<CodeEntity>,
) {
    let Some(entity) = parse(node, ctx.source, ctx.path, parent_id, ctx.package) else { return };
    let entity_id = entity.id.clone();
    ctx.result.add_entity(entity);
    if let Some(body) = node.child_by_field_name("body") {
        extract_entities(body, Some(&entity_id), ctx);
    }
}

/// Register a `@Field`-annotated script-scope declaration as a
/// `module_state` Variable and emit a `WritesTo` edge from the script
/// container so the field appears as part of the script's state in
/// the graph. No edge is emitted when there is no script container —
/// `@Field` outside a script (e.g. mistakenly inside a class body) is
/// a no-op per the ticket's "out of scope" note.
fn handle_field_decl(node: &Node, parent_id: Option<&str>, ctx: &mut ExtractCtx<'_>) {
    let entities = fields::parse_field_decl(node, ctx.source, ctx.path, parent_id);
    let Some(caller_id) = parent_id else {
        for entity in entities {
            ctx.result.add_entity(entity);
        }
        return;
    };
    for entity in entities {
        let target_id = entity.id.clone();
        ctx.result.add_entity(entity);
        let mut rel = Relationship::new(
            caller_id.to_string(),
            target_id,
            RelationshipKind::WritesTo,
        );
        rel.metadata
            .insert("module_state".to_string(), "true".to_string());
        ctx.result.add_relationship(rel);
    }
}

/// Walk a script's top-level statements as if they were the body of
/// the synthetic container, so calls and `try`/`catch` arms attribute
/// to it. `extract_calls` handles the structural skips internally —
/// classes, methods, and `@Field` decls are already registered by
/// `extract_entities` and are noops here.
fn extract_script_calls(root: Node, container_id: &str, ctx: &mut ExtractCtx<'_>) {
    let mut call_order = 0u32;
    let mut arm_counter = 0u32;
    let mut loop_counter = 0u32;
    let mut call_ctx = calls::CallCtx {
        source: ctx.source,
        path: ctx.path,
        caller_id: container_id,
        caller_name: "",
        parent_class: None,
        call_order: &mut call_order,
        arm_counter: &mut arm_counter,
        loop_counter: &mut loop_counter,
        result: &mut *ctx.result,
    };
    calls::extract_calls(&root, &mut call_ctx, None);
}

impl Default for GroovyParser {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageParser for GroovyParser {
    fn language(&self) -> Language {
        Language::Groovy
    }

    fn parse(&self, path: &Path, content: &str) -> Result<ParseResult> {
        let mut parser = Self::new();
        let tree = parser.parse_tree(content)?;
        let root = tree.root_node();

        let mut result = ParseResult::new();
        let script_container = if is_script_file(root) {
            let entity = build_script_container(root, path, content);
            let id = entity.id.clone();
            result.add_entity(entity);
            Some(id)
        } else {
            None
        };

        let parent_id = script_container.as_deref();
        let package = extract_package(root, content);
        let mut ctx = ExtractCtx {
            source: content,
            path,
            package: &package,
            result: &mut result,
        };
        extract_entities(root, parent_id, &mut ctx);

        if let Some(container_id) = parent_id {
            extract_script_calls(root, container_id, &mut ctx);
        }

        // Emit UsesType edges from signature/field types, mirroring JV-001.
        types::emit_uses_type_edges(&mut result);

        Ok(result)
    }
}
