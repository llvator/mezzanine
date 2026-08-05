//! Code parsing using tree-sitter for multi-language support.

pub mod language_parser;
mod rust;
mod python;
mod java;
mod kotlin;
mod typescript;
mod svelte;
mod groovy;
mod impex;
mod elevator;
mod ansible;
pub(crate) mod markdown;
pub(crate) mod sql;
mod generic_parser;

pub use language_parser::{LanguageParser, ParseResult};
pub use rust::RustParser;
/// Normalise a written-out Rust type to the bare name the entity index is
/// keyed by. Shared with the analyzer's cross-file field index (AN-012), which
/// reads the same declared types off entities instead of off the tree.
pub(crate) use rust::inference::base_type_name as rust_base_type_name;
pub use python::PythonParser;
pub use java::JavaParser;
pub use kotlin::KotlinParser;
pub use typescript::TypeScriptParser;
pub use svelte::SvelteParser;
pub use groovy::GroovyParser;
pub use impex::ImpexParser;
pub use elevator::ElevatorParser;
pub use ansible::AnsibleParser;
pub use markdown::MarkdownParser;
pub use sql::SqlParser;
pub use generic_parser::GenericParser;

use crate::models::file_info::Language;
use anyhow::Result;
use std::path::Path;

/// Detect a file's language for parsing.
///
/// Prefers path-structural detection for languages that own no file
/// extension (ansible-deploy classifies by repo layout: var files and
/// `environments/*/files/` manifests). Falls back to extension-based
/// detection for everything else. This is the single detection entry
/// point the walker and analyzer share, so inclusion, `FileInfo`, and
/// parser dispatch never disagree.
pub fn detect_language(path: &Path) -> Language {
    if ansible::is_deploy_file(path) {
        return Language::AnsibleDeploy;
    }
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    Language::from_extension(ext)
}

/// Get the appropriate parser for a file
pub fn get_parser(language: Language) -> Box<dyn LanguageParser> {
    match language {
        Language::Rust => Box::new(RustParser::new()),
        Language::Python => Box::new(PythonParser::new()),
        Language::Java => Box::new(JavaParser::new()),
        Language::Kotlin => Box::new(KotlinParser::new()),
        Language::TypeScript => Box::new(TypeScriptParser::new()),
        Language::Svelte => Box::new(SvelteParser::new()),
        Language::Groovy => Box::new(GroovyParser::new()),
        Language::Impex => Box::new(ImpexParser::new()),
        Language::Elevator => Box::new(ElevatorParser::new()),
        Language::AnsibleDeploy => Box::new(AnsibleParser::new()),
        Language::Sql => Box::new(SqlParser::new()),
        Language::Markdown => Box::new(MarkdownParser::new()),
        // For other languages, use the generic parser for now
        _ => Box::new(GenericParser::new(language)),
    }
}

/// Parse a source file and return entities and relationships
pub fn parse_file(path: &Path) -> Result<ParseResult> {
    let language = detect_language(path);
    let parser = get_parser(language);

    let content = std::fs::read_to_string(path)?;
    parser.parse(path, &content)
}

/// Parse already-read source content for a known language. Lets callers
/// that have the bytes in hand (e.g. the AN-003 parse store, which reads
/// once to hash) parse without a second `read_to_string`.
pub fn parse_content(path: &Path, content: &str, language: Language) -> Result<ParseResult> {
    get_parser(language).parse(path, content)
}
