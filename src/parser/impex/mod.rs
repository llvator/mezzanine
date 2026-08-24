//! Impex parser — line-oriented, hand-rolled (no tree-sitter grammar
//! exists for Impex; SAP doesn't publish one). Reference docs:
//! <https://help.sap.com/docs/SAP_COMMERCE/d0224eca81e249cb821f2cdf45a82ace/8c014deb866910149601c28b53d9a91f.html>.
//!
//! Submodules group the parsing logic by concern:
//! - [`headers`] — `INSERT` / `UPDATE` / `INSERT_UPDATE` / `REMOVE` lines + header modifiers
//! - [`columns`] — column-header cells, including reference columns (IM-002)
//! - [`macros`] — `$<name> = <value>` definitions and `$<name>` substitution (IM-004)
//! - [`scripts`] — `#%` script-line forms (IM-003)
//! - [`lexer`] — quote-aware splitter shared by every concern above
//!
//! Per IM-001, every `.impex` file emits a synthetic `Module` entity
//! tagged `impex_file` so top-level statements have a `caller_id` —
//! the same shape Groovy script containers use (one synthetic entity
//! standing in for the file's body).

mod columns;
mod headers;
mod lexer;
mod macros;
mod scripts;

#[cfg(test)]
mod tests;

use super::language_parser::{LanguageParser, ParseResult};
use crate::models::file_info::Language;
use crate::models::{
    CodeEntity, EntityKind, Position, Relationship, RelationshipKind, Span, Visibility,
};
use anyhow::Result;
use std::path::Path;

use self::columns::Column;
use self::headers::{HeaderOp, ImpexHeader};
use self::macros::MacroTable;
use self::scripts::{HookKind, ScriptLine};

pub struct ImpexParser;

impl ImpexParser {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ImpexParser {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-file parse state threaded through the line walk.
struct ParseCtx<'a> {
    path: &'a Path,
    file_id: String,
    macros: MacroTable,
    /// Stack of `if:` Branch paths currently open. The top of the
    /// stack is the active `current_branch` when emitting
    /// relationships from gated regions (IM-003).
    if_stack: Vec<String>,
    arm_counter: u32,
    result: &'a mut ParseResult,
}

impl<'a> ParseCtx<'a> {
    fn current_branch(&self) -> Option<&str> {
        self.if_stack.last().map(|s| s.as_str())
    }

    fn next_branch_path(&mut self) -> String {
        self.arm_counter += 1;
        match self.if_stack.last() {
            Some(parent) => format!("{}.c{}", parent, self.arm_counter),
            None => format!("c{}", self.arm_counter),
        }
    }
}

impl LanguageParser for ImpexParser {
    fn language(&self) -> Language {
        Language::Impex
    }

    fn parse(&self, path: &Path, content: &str) -> Result<ParseResult> {
        let mut result = ParseResult::new();

        // Synthetic file-level container so top-level statements
        // (headers, `#%` lines, macros) have a caller. Same shape
        // Groovy uses for script files.
        let file_entity = build_file_container(path, content);
        let file_id = file_entity.id.clone();
        result.add_entity(file_entity);

        let mut ctx = ParseCtx {
            path,
            file_id,
            macros: MacroTable::new(),
            if_stack: Vec::new(),
            arm_counter: 0,
            result: &mut result,
        };

        for raw_line in content.lines() {
            dispatch_line(raw_line, &mut ctx);
        }

        flush_macro_warnings(&mut ctx);
        Ok(result)
    }
}

/// File entity name = the file stem; same convention Groovy uses.
fn build_file_container(path: &Path, content: &str) -> CodeEntity {
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("impex")
        .to_string();
    let line_count = content.lines().count().max(1);
    let span = Span::new(
        Position::new(0, 0, 0),
        Position::new(line_count, 0, content.len()),
    );
    let mut entity = CodeEntity::new(&name, EntityKind::Module, path, span);
    entity.qualified_name = name;
    entity.tags.insert("impex_file".to_string());
    entity.visibility = Visibility::Public;
    entity
}

/// Single-line dispatcher. Order matters: macro definitions and `#%`
/// script lines are recognised before headers because the leading
/// character set overlaps (`$START_USERRIGHTS` would otherwise look
/// like a macro; `INSERT_UPDATE` would otherwise look like a comment
/// if the dispatcher mishandled `#%`).
fn dispatch_line(raw_line: &str, ctx: &mut ParseCtx<'_>) {
    let trimmed = raw_line.trim();
    if trimmed.is_empty() {
        return;
    }

    // `#%` script lines — IM-003. Must check BEFORE the `#`-comment
    // arm because the prefix overlaps.
    if let Some(line) = scripts::parse_script_line(trimmed) {
        handle_script_line(line, ctx);
        return;
    }

    // Plain comments.
    if trimmed.starts_with('#') {
        return;
    }

    // Macros — IM-004. Substitution happens later for header /
    // column values, so we only learn definitions here.
    if let Some((name, value)) = MacroTable::parse_definition(trimmed) {
        ctx.macros.define(name, value);
        return;
    }

    // Skip data rows: per Impex grammar, lines starting with `;` are
    // data rows beneath the active header. We don't emit entities
    // for them — IM-001 explicit out-of-scope note.
    if trimmed.starts_with(';') {
        return;
    }

    // Header lines: substitute macros first so any `$<name>` inside
    // modifier values is resolved by the time we see it.
    let substituted = ctx.macros.substitute(trimmed);
    if let Some(header) = headers::parse_header(&substituted) {
        handle_header(&header, ctx);
    }
    // Anything else (block markers like `$START_USERRIGHTS`,
    // unrecognised lines) is silently ignored — the parser's job is
    // to surface what it understands, not to police the file.
}

/// Emit the relationships a header carries: `WritesTo` to the target
/// type from the file (IM-001), `References` to FQN class names from
/// the header modifiers (IM-001), and per-reference-column
/// type-to-type `References` from the target type to the column's
/// referenced type (IM-002).
fn handle_header(header: &ImpexHeader, ctx: &mut ParseCtx<'_>) {
    let branch = ctx.current_branch().map(str::to_string);
    let kind = match header.op {
        HeaderOp::Insert | HeaderOp::Update | HeaderOp::InsertUpdate => RelationshipKind::WritesTo,
        HeaderOp::Remove => RelationshipKind::WritesTo,
    };

    let mut rel = Relationship::new(ctx.file_id.clone(), header.target_type.clone(), kind);
    rel.metadata
        .insert("operation".to_string(), op_label(header.op).to_string());
    if let Some(b) = &branch {
        rel.metadata.insert("branch".to_string(), b.clone());
    }
    ctx.result.add_relationship(rel);

    // FQN class refs from header modifiers — cross-language edges
    // into the Java entity space.
    for class in &header.class_refs {
        let mut class_rel = Relationship::new(
            ctx.file_id.clone(),
            class.clone(),
            RelationshipKind::References,
        );
        class_rel
            .metadata
            .insert("from".to_string(), "impex_header_modifier".to_string());
        if let Some(b) = &branch {
            class_rel.metadata.insert("branch".to_string(), b.clone());
        }
        ctx.result.add_relationship(class_rel);
    }

    // IM-002 — reference columns become type-to-type edges sourced
    // from the row's target type, *not* the file. The file already
    // owns the WritesTo above; the reference column's structural
    // signal survives across many files writing the same type.
    for column in &header.columns {
        emit_reference_column(&header.target_type, column, ctx);
    }
}

fn emit_reference_column(target_type: &str, column: &Column, ctx: &mut ParseCtx<'_>) {
    let Some(attrs) = &column.target_attrs else {
        return;
    };
    if attrs.is_empty() {
        return;
    }
    // Hybris convention: the column's name is the attribute on the
    // row's type, and the target type is the same word capitalised
    // (`unit(code)` on `UnitMapping` = FK from `UnitMapping`
    // to `Unit` keyed by `code`). The lowercase original lives in
    // metadata as `field` so the resolver can do a fuzzier match
    // when items.xml resolution lands (IM-006); the capitalised
    // form is the relationship target so cross-language resolution
    // into the existing entity space works today.
    let referenced_type = capitalize_type_name(&column.name);
    let mut rel = Relationship::new(
        target_type.to_string(),
        referenced_type,
        RelationshipKind::References,
    );
    rel.metadata
        .insert("field".to_string(), column.name.clone());
    rel.metadata
        .insert("target_attrs".to_string(), attrs.join(","));
    rel.metadata
        .insert("from".to_string(), "impex_reference_column".to_string());
    if column
        .modifiers
        .iter()
        .any(|(k, v)| k == "unique" && v == "true")
    {
        rel.metadata
            .insert("identity".to_string(), "true".to_string());
    }
    if let Some(b) = ctx.current_branch() {
        rel.metadata.insert("branch".to_string(), b.to_string());
    }
    ctx.result.add_relationship(rel);
}

/// Dispatch an already-parsed `#%` line. Statement / hook bodies go
/// through the Groovy parser; conditionals open / close a Branch arm
/// on the if-stack.
fn handle_script_line(line: ScriptLine<'_>, ctx: &mut ParseCtx<'_>) {
    match line {
        ScriptLine::Statement(body) => parse_groovy_into(body, ctx, None),
        ScriptLine::Hook { kind, body } => {
            parse_groovy_into(body, ctx, Some(kind));
        }
        ScriptLine::IfStart(condition) => {
            let condition = ctx.macros.substitute(condition);
            let branch_path = ctx.next_branch_path();
            emit_if_branch(&branch_path, &condition, ctx);
            ctx.if_stack.push(branch_path);
        }
        ScriptLine::IfEnd => {
            // Pop the most recent. A stray `endif:` (no matching
            // `if:`) is silently dropped — the parser's job is to be
            // robust against malformed input, not to enforce
            // structure.
            let _ = ctx.if_stack.pop();
        }
    }
}

/// Hand a Groovy snippet to the Groovy parser via the public
/// `parse_expression` API exposed for cross-parser use, then merge
/// the relationships it produced into our result. Fail-soft: a parse
/// error from the Groovy side surfaces as a warning, never aborts the
/// Impex parse.
fn parse_groovy_into(body: &str, ctx: &mut ParseCtx<'_>, hook: Option<HookKind>) {
    if body.trim().is_empty() {
        return;
    }
    let substituted = ctx.macros.substitute(body);
    let branch_owned = ctx.current_branch().map(str::to_string);
    match super::groovy::GroovyParser::parse_expression(
        &substituted,
        ctx.path,
        &ctx.file_id,
        branch_owned.as_deref(),
    ) {
        Ok(rels) => {
            for mut rel in rels {
                if let Some(kind) = hook {
                    rel.metadata
                        .insert("impex_hook".to_string(), kind.label().to_string());
                }
                rel.metadata
                    .insert("from".to_string(), "impex_script".to_string());
                ctx.result.add_relationship(rel);
            }
        }
        Err(e) => {
            ctx.result
                .add_warning(format!("Groovy parse failed for `#%` body: {}", e));
        }
    }
}

/// Emit a synthetic `Branch` entity for an `#% if:` block — same
/// shape Groovy / Python use for `if` arms. Tags `if_node` and
/// `impex_condition`; the condition text travels as
/// `condition:<text>` and as the entity's `documentation` so the
/// detail panel reads cleanly without a re-parse.
fn emit_if_branch(branch_path: &str, condition: &str, ctx: &mut ParseCtx<'_>) {
    let branch_id = format!("{}::branch::{}", ctx.file_id, branch_path);
    let parent_id = match ctx.if_stack.last() {
        Some(p) => format!("{}::branch::{}", ctx.file_id, p),
        None => ctx.file_id.clone(),
    };
    // The Branch is a structural entity — the span is meaningless
    // (it covers a region we don't track precisely) so we use the
    // file entity's span as a placeholder.
    let span = Span::default();
    let mut entity = CodeEntity::new(branch_path.to_string(), EntityKind::Branch, ctx.path, span);
    entity.id = branch_id;
    entity.qualified_name = format!("{}::{}", ctx.file_id, branch_path);
    entity.parent_id = Some(parent_id);
    entity.tags.insert("branch_node".to_string());
    entity.tags.insert("if_node".to_string());
    entity.tags.insert("impex_condition".to_string());
    entity.visibility = Visibility::Private;
    let trimmed = condition.trim();
    if !trimmed.is_empty() {
        entity.attributes.push(format!("condition:{}", trimmed));
        entity.documentation = Some(format!("condition: {}", trimmed));
    }
    ctx.result.add_entity(entity);
}

/// Capitalise the first character of an attribute name so it matches
/// Hybris's type-name convention (`unit` → `Unit`, `catalogItemCategory`
/// → `CatalogItemCategory`). Existing capitalised names pass through
/// unchanged.
fn capitalize_type_name(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) => c.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

fn op_label(op: HeaderOp) -> &'static str {
    match op {
        HeaderOp::Insert => "INSERT",
        HeaderOp::Update => "UPDATE",
        HeaderOp::InsertUpdate => "INSERT_UPDATE",
        HeaderOp::Remove => "REMOVE",
    }
}

/// Move accumulated macro warnings (unresolved references, recursion
/// cycles) into the `ParseResult`. Done at end-of-file rather than
/// inline so each warning lands once per file regardless of how many
/// times the offending macro is referenced.
fn flush_macro_warnings(ctx: &mut ParseCtx<'_>) {
    let mut unresolved: Vec<String> = std::mem::take(&mut ctx.macros.unresolved_refs);
    unresolved.sort();
    unresolved.dedup();
    for name in unresolved {
        ctx.result.add_warning(format!(
            "Impex macro `${}` referenced before definition",
            name
        ));
    }
    for w in std::mem::take(&mut ctx.macros.cycle_warnings) {
        ctx.result.add_warning(w);
    }
}
