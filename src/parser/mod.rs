//! Code parsing using tree-sitter for multi-language support.

mod ansible;
mod dart;
mod elevator;
mod generic_parser;
mod go;
mod groovy;
mod impex;
mod java;
mod kotlin;
pub mod language_parser;
pub(crate) mod markdown;
mod python;
mod rust;
pub(crate) mod rust_type_names;
pub(crate) mod sql;
mod svelte;
mod typescript;

pub use ansible::AnsibleParser;
pub use dart::DartParser;
pub use elevator::ElevatorParser;
pub use generic_parser::GenericParser;
pub use go::GoParser;
pub use groovy::GroovyParser;
pub use impex::ImpexParser;
pub use java::JavaParser;
pub use kotlin::KotlinParser;
pub use language_parser::{LanguageParser, ParseResult};
pub use markdown::MarkdownParser;
pub use python::PythonParser;
pub use rust::RustParser;
pub use sql::SqlParser;
pub use svelte::SvelteParser;
pub use typescript::TypeScriptParser;

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
        Language::Go => Box::new(GoParser::new()),
        Language::Kotlin => Box::new(KotlinParser::new()),
        Language::Dart => Box::new(DartParser::new()),
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
