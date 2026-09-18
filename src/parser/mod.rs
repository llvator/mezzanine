//! Code parsing using tree-sitter for multi-language support.

mod ansible;
mod docker;
pub(crate) mod costs;
mod cpp;
mod dart;
mod elevator;
mod generic_parser;
mod go;
mod groovy;
mod impex;
mod java;
mod javascript;
mod kotlin;
pub mod language_parser;
pub(crate) mod loops;
pub(crate) mod markdown;
mod python;
mod rust;
pub(crate) mod rust_type_names;
pub(crate) mod sql;
pub(crate) mod effects;
pub(crate) mod stdlib;
mod svelte;
mod typescript;
pub(crate) mod working_set;

pub use ansible::AnsibleParser;
pub use docker::DockerParser;
pub use cpp::CppParser;
pub use dart::DartParser;
pub use elevator::ElevatorParser;
pub use generic_parser::GenericParser;
pub use go::GoParser;
pub use groovy::GroovyParser;
pub use impex::ImpexParser;
pub use java::JavaParser;
pub use javascript::JavaScriptParser;
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
/// `environments/*/files/` manifests; docker classifies by filename,
/// since `Dockerfile` has no extension). Falls back to extension-based
/// detection for everything else. This is the single detection entry
/// point the walker and analyzer share, so inclusion, `FileInfo`, and
/// parser dispatch never disagree.
pub fn detect_language(path: &Path) -> Language {
    if ansible::is_deploy_file(path) {
        return Language::AnsibleDeploy;
    }
    // After ansible-deploy deliberately: that parser claims by repo
    // layout (`group_vars/`, `files/`), and a deploy repo is entitled to
    // hold a `compose.yml` that belongs to it rather than to Docker.
    if docker::is_docker_file(path) {
        return Language::Docker;
    }
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    Language::from_extension(ext)
}

/// How a parser is built, given the language it was selected for. The
/// argument matters for the two parsers that answer for more than one
/// language: `CppParser` reports C or C++, `GenericParser` reports
/// whatever it was handed.
type Build = fn(Language) -> Box<dyn LanguageParser>;

/// Which parser owns which language.
///
/// A table rather than a `match`, for the reason
/// [`working_set::BINDING_FIELDS`](working_set) gives for being one:
/// adding a language should cost a row and no complexity. The `match` this
/// replaced had grown an arm per language and a cyclomatic count to go
/// with it, so the seventeenth language could not be added without
/// pushing an already-grandfathered function higher.
///
/// A language with no row here gets [`GenericParser`], which recovers what
/// a shape-agnostic walk can.
const PARSERS: &[(Language, Build)] = &[
    (Language::Rust, |_| Box::new(RustParser::new())),
    (Language::Python, |_| Box::new(PythonParser::new())),
    (Language::Java, |_| Box::new(JavaParser::new())),
    (Language::Go, |_| Box::new(GoParser::new())),
    // One grammar, two languages: C++ is a superset of C for everything
    // the walk reads, and `.h` belongs to both. See `parser/cpp` for why a
    // second parser would differ only by the arms it never reaches.
    (Language::Cpp, |language| {
        Box::new(CppParser::for_language(language))
    }),
    (Language::C, |language| {
        Box::new(CppParser::for_language(language))
    }),
    (Language::Kotlin, |_| Box::new(KotlinParser::new())),
    (Language::Dart, |_| Box::new(DartParser::new())),
    (Language::TypeScript, |_| Box::new(TypeScriptParser::new())),
    (Language::JavaScript, |_| Box::new(JavaScriptParser::new())),
    (Language::Svelte, |_| Box::new(SvelteParser::new())),
    (Language::Groovy, |_| Box::new(GroovyParser::new())),
    (Language::Impex, |_| Box::new(ImpexParser::new())),
    (Language::Elevator, |_| Box::new(ElevatorParser::new())),
    (Language::AnsibleDeploy, |_| Box::new(AnsibleParser::new())),
    (Language::Docker, |_| Box::new(DockerParser::new())),
    (Language::Sql, |_| Box::new(SqlParser::new())),
    (Language::Markdown, |_| Box::new(MarkdownParser::new())),
];

/// The parser a language owns, or `None` for one with no row in
/// [`PARSERS`].
fn parser_for(language: Language) -> Option<Build> {
    PARSERS
        .iter()
        .find(|(owned, _)| *owned == language)
        .map(|(_, build)| *build)
}

/// Get the appropriate parser for a file
pub fn get_parser(language: Language) -> Box<dyn LanguageParser> {
    match parser_for(language) {
        Some(build) => build(language),
        None => Box::new(GenericParser::new(language)),
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
