//! Rust doc-comment extraction (`///`, `//!`, `/** */`, `/*! */`).
//!
//! Rust has two kinds of doc comment and they point in opposite directions.
//! An *outer* doc (`///`, `/** */`) documents the item that follows it; an
//! *inner* doc (`//!`, `/*! */`) documents the item that encloses it — the
//! file, or the `mod` block it sits at the top of. So the two need separate
//! walks: [`extract_doc_comment`] looks backward from an item,
//! [`extract_inner_doc`] looks forward from a container's body.

use super::super::language_parser::node_text;
use tree_sitter::Node;

/// A doc comment split into "which way does it point" and its text with the
/// marker stripped. `None` for an ordinary comment.
///
/// The grammar's `line_comment` node includes the newline that ends it, so
/// each line is trimmed on the right — otherwise every joined doc comes out
/// double-spaced and every single-line one ends in a stray blank line.
fn doc_body(text: &str) -> Option<(Marker, String)> {
    if let Some(rest) = text.strip_prefix("///") {
        return Some((Marker::Outer, line_body(rest)));
    }
    if let Some(rest) = text.strip_prefix("//!") {
        return Some((Marker::Inner, line_body(rest)));
    }
    let block = text
        .strip_prefix("/**")
        .map(|rest| (Marker::Outer, rest))
        .or_else(|| text.strip_prefix("/*!").map(|rest| (Marker::Inner, rest)));
    let (marker, rest) = block?;
    // Strip leading `* ` on each line (Javadoc-style block comments).
    let cleaned: Vec<&str> = rest
        .trim_end_matches("*/")
        .trim()
        .lines()
        .map(|l| l.trim().trim_start_matches('*').trim_start_matches(' '))
        .collect();
    Some((marker, cleaned.join("\n")))
}

/// One `///` or `//!` line, minus its marker, the single space authors put
/// after it, and the trailing newline the node carries.
fn line_body(rest: &str) -> String {
    rest.strip_prefix(' ')
        .unwrap_or(rest)
        .trim_end()
        .to_string()
}

/// Which item a doc comment describes: the one after it, or the one around it.
#[derive(PartialEq, Eq)]
enum Marker {
    /// `///` or `/** */` — documents the item that follows.
    Outer,
    /// `//!` or `/*! */` — documents the enclosing file or module.
    Inner,
}

/// Extract the outer doc comment (`///`, `/** */`) attached to an item.
/// Walks backward through `prev_sibling` until a non-comment node is found;
/// collects and joins all contiguous doc comments into a single string with
/// the marker stripped.
///
/// An inner doc ends the run rather than joining it. Without that, a file's
/// `//!` header is read as the description of whichever item happens to
/// follow it — which is what a header with no `use` block under it does.
pub(super) fn extract_doc_comment(node: &Node, source: &str) -> Option<String> {
    let mut lines = Vec::new();
    let mut current = node.prev_sibling();
    while let Some(sib) = current {
        match preceding_step(&sib, source) {
            Step::Take(body) => lines.push(body),
            Step::Skip => {}
            Step::Stop => break,
        }
        current = sib.prev_sibling();
    }
    if lines.is_empty() {
        return None;
    }
    lines.reverse(); // comments were collected bottom-up
    Some(lines.join("\n"))
}

/// What one node means to a walk that is collecting a doc comment: take it,
/// step over it, or stop here. Shared by both walks so each stays a loop
/// over a decision rather than a nest of matches.
enum Step {
    /// Part of the doc being collected.
    Take(String),
    /// Not part of it, but legally in the way: an attribute, the `{` of a
    /// `mod` body, an ordinary comment such as a licence banner.
    Skip,
    /// The doc is over.
    Stop,
}

/// One step backward from an item, collecting its outer doc.
fn preceding_step(sib: &Node, source: &str) -> Step {
    match sib.kind() {
        // Attribute macros (#[…]) sit between a doc comment and its item.
        "attribute_item" | "inner_attribute_item" => Step::Skip,
        "line_comment" | "block_comment" => match doc_body(node_text(sib, source)) {
            Some((Marker::Outer, body)) => Step::Take(body),
            // An inner doc belongs to the enclosing scope, and a plain
            // comment ends the run either way.
            _ => Step::Stop,
        },
        _ => Step::Stop,
    }
}

/// One step forward through a container's body, collecting its header.
fn header_step(child: &Node, source: &str) -> Step {
    match child.kind() {
        "{" | "inner_attribute_item" => Step::Skip,
        "line_comment" | "block_comment" => match doc_body(node_text(child, source)) {
            Some((Marker::Inner, body)) => Step::Take(body),
            Some((Marker::Outer, _)) => Step::Stop,
            None => Step::Skip,
        },
        _ => Step::Stop,
    }
}

/// Extract the inner doc comment (`//!`, `/*! */`) heading a container's
/// body — the whole file for a `source_file`, the block for a `mod foo { }`.
///
/// Walks forward from the first child and stops at the first thing that
/// isn't part of the header: an item, or an outer doc, which belongs to the
/// item under it rather than to the container. Ordinary comments (a licence
/// banner) and inner attributes (`#![allow(…)]`) are stepped over, since
/// both legally sit above the header.
pub(super) fn extract_inner_doc(container: &Node, source: &str) -> Option<String> {
    let mut lines = Vec::new();
    let mut cursor = container.walk();
    for child in container.children(&mut cursor) {
        match header_step(&child, source) {
            Step::Take(body) => lines.push(body),
            Step::Skip => continue,
            Step::Stop => break,
        }
    }
    if lines.is_empty() {
        return None;
    }
    Some(lines.join("\n"))
}
