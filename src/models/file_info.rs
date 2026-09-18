//! File and position information.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Information about a source file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct FileInfo {
    /// Absolute path to the file
    pub path: PathBuf,

    /// Detected programming language
    pub language: Language,

    /// File size in bytes
    pub size: u64,

    /// Number of lines
    pub line_count: usize,

    /// SHA256 hash of contents (for caching)
    pub content_hash: Option<String>,

    /// What this file is for, in its own words: the `//!` header of a Rust
    /// file and the equivalent elsewhere. `None` when the file has none, or
    /// when its language's parser doesn't recover one yet.
    ///
    /// It lives here rather than on an entity because it describes the file,
    /// and the module a file defines is declared somewhere else. Renderers
    /// read it through `DependencyGraph::file_documentation`.
    #[serde(default)]
    pub documentation: Option<String>,
}

/// Supported programming languages
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Java,
    Go,
    CSharp,
    Cpp,
    C,
    Ruby,
    Swift,
    Kotlin,
    /// Dart (`.dart`). Parsed by `tree-sitter-dart`, pinned to the last
    /// grammar release built at an ABI our tree-sitter can load — which
    /// predates Dart 3, so the class modifiers it never learned are
    /// blanked out of the source before parsing. See `parser/dart/dart3`.
    Dart,
    Scala,
    PHP,
    Groovy,
    Impex,
    /// Svelte single-file component (`.svelte`). Parsed by masking the
    /// non-`<script>` regions and delegating the logic to the TypeScript
    /// extractor, then scanning the template markup for child-component
    /// ("widget") usage. See `parser/svelte`.
    Svelte,
    /// Elevator — domain-level spec language for describing
    /// Features / Functionalities / Concepts / UI Pages of a project.
    /// Source files use the `.elv` extension. Named for its purpose:
    /// take an onboarding reader from the ground floor (categories) up
    /// to the specific functionality they need.
    Elevator,
    /// ansible-deploy — declarative Kubernetes deployment repos
    /// (Ansible + Helm + Jinja2). Not a general-purpose language: it
    /// recovers the deploy *topology* (playbook → role → var file →
    /// template → k8s primitive) rather than code quality. See
    /// ADR 0003 (ansible/k8s topology).
    ///
    /// Deliberately NOT wired into `from_extension`: `.yml`/`.yaml`/`.j2`
    /// are far too common to claim globally. Selection is path-aware
    /// (see the parser's `classify`) and, for now, explicit via
    /// `from_name`.
    AnsibleDeploy,
    /// Markdown (`.md`, `.markdown`). A documentation *link* language, not
    /// a code one: the parser recovers one Note per file and the links
    /// between notes, plus the source paths a note points at. Complexity
    /// metrics stay empty for the same reason they do for SQL and
    /// ansible-deploy — see ADR 0003.
    ///
    /// **Opt-in.** Unlike every other extension here, `.md` is ubiquitous
    /// in code repos: READMEs, ADRs, changelogs and issue trackers would
    /// otherwise land in every graph unasked. `from_extension` still claims
    /// the extension so single-file analysis works, but the walker skips
    /// Markdown unless it is named explicitly (`-l markdown`) — see
    /// `Language::is_opt_in`.
    Markdown,
    /// Docker — `Dockerfile`s and Compose files. Like ansible-deploy, a
    /// topology language rather than a code-quality one: it recovers what
    /// builds from what (stage → base image, stage → stage) and what runs
    /// against what (service → stage, service → volume/network), so
    /// complexity metrics stay empty per ADR 0003. Lint-shaped findings
    /// (`latest` tags, root users, layer counts) are hadolint's job and
    /// deliberately not this parser's — see ADR 0034.
    ///
    /// Deliberately NOT wired into `from_extension`: the canonical
    /// `Dockerfile` has no extension at all, and `compose.yaml` shares
    /// `.yml`/`.yaml` with every other YAML in a repo. Selection is
    /// filename-structural (see the parser's `classify`), and Compose
    /// files additionally have to *look* like Compose before being
    /// claimed.
    Docker,
    /// SQL schema files (`.sql`). Parsed for *topology* — tables, columns and
    /// the foreign keys between them — not for control flow, so complexity
    /// metrics stay empty. In migration-based repos a `.sql` file is a
    /// fragment rather than a whole schema; folding the fragments into the
    /// effective schema is an analyzer pass, not this parser's job, per
    /// ADR-0007.
    Sql,
    #[default]
    Unknown,
}

/// Language groupings for name resolution (AN-014). Languages in the same
/// family share an entity space by design; languages in different families
/// share only accidents of naming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    /// Compiles to, or references, the JVM entity space.
    Jvm,
    /// TypeScript and the things that embed it.
    Web,
    /// Shares headers.
    CLike,
    /// Its own entity space. Discriminated by the `Language` it came from
    /// so two solo languages never compare equal.
    Solo(u8),
    /// Undetermined — binds with anything rather than silently dropping
    /// edges we have no basis to reject.
    Any,
}

impl Language {
    /// Detect language from file extension
    pub fn from_extension(ext: &str) -> Self {
        match ext.to_lowercase().as_str() {
            "rs" => Language::Rust,
            "py" | "pyi" => Language::Python,
            "js" | "mjs" | "cjs" => Language::JavaScript,
            "ts" | "tsx" => Language::TypeScript,
            "jsx" => Language::JavaScript,
            "java" => Language::Java,
            "go" => Language::Go,
            "cs" => Language::CSharp,
            "cpp" | "cc" | "cxx" | "hpp" | "hxx" => Language::Cpp,
            "c" | "h" => Language::C,
            "rb" => Language::Ruby,
            "swift" => Language::Swift,
            "kt" | "kts" => Language::Kotlin,
            "dart" => Language::Dart,
            "scala" | "sc" => Language::Scala,
            "php" => Language::PHP,
            "groovy" | "gradle" => Language::Groovy,
            "impex" => Language::Impex,
            "svelte" => Language::Svelte,
            "elv" => Language::Elevator,
            "sql" => Language::Sql,
            "md" | "markdown" => Language::Markdown,
            _ => Language::Unknown,
        }
    }

    /// Detect language from a path's extension alone.
    ///
    /// `parser::detect_language` is the entry point for deciding how a file
    /// is *parsed* — it also classifies by repo layout, which this does not.
    /// Use this one where the answer only ranks or filters candidates
    /// (AN-014) and `Unknown` is an acceptable, permissive answer.
    pub fn from_path(path: &Path) -> Self {
        Self::from_extension(path.extension().and_then(|e| e.to_str()).unwrap_or(""))
    }

    /// Get common file extensions for this language
    pub fn extensions(&self) -> &[&str] {
        match self {
            Language::Rust => &["rs"],
            Language::Python => &["py", "pyi"],
            Language::JavaScript => &["js", "mjs", "cjs", "jsx"],
            Language::TypeScript => &["ts", "tsx"],
            Language::Java => &["java"],
            Language::Go => &["go"],
            Language::CSharp => &["cs"],
            Language::Cpp => &["cpp", "cc", "cxx", "hpp", "hxx", "h"],
            Language::C => &["c", "h"],
            Language::Ruby => &["rb"],
            Language::Swift => &["swift"],
            Language::Kotlin => &["kt", "kts"],
            Language::Dart => &["dart"],
            Language::Scala => &["scala", "sc"],
            Language::PHP => &["php"],
            Language::Groovy => &["groovy", "gradle"],
            Language::Impex => &["impex"],
            Language::Svelte => &["svelte"],
            Language::Elevator => &["elv"],
            Language::Sql => &["sql"],
            Language::Markdown => &["md", "markdown"],
            // Intentionally empty: ansible-deploy is path-classified,
            // not extension-owned (`.yml`/`.j2` belong to no one).
            Language::AnsibleDeploy => &[],
            // Likewise empty, and for a sharper version of the same
            // reason: `Dockerfile` carries no extension for this to
            // return, and the one it *could* claim (`.yaml`) belongs to
            // no one. Filename-classified — see `parser::docker::classify`.
            Language::Docker => &[],
            Language::Unknown => &[],
        }
    }

    /// Parse a language from its human-readable name (case-insensitive).
    /// Accepts both full names ("rust", "python") and short aliases ("rs", "py").
    pub fn from_name(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "rust" | "rs" => Some(Language::Rust),
            "python" | "py" => Some(Language::Python),
            "javascript" | "js" => Some(Language::JavaScript),
            "typescript" | "ts" => Some(Language::TypeScript),
            "java" => Some(Language::Java),
            "go" | "golang" => Some(Language::Go),
            "csharp" | "cs" | "c#" => Some(Language::CSharp),
            "cpp" | "c++" => Some(Language::Cpp),
            "c" => Some(Language::C),
            // These five had no arm until the UI-list drift test went looking.
            // The "Parsed Languages" panel has always offered them, and the
            // server dropped each one as an unknown language — five checkboxes
            // that did nothing, Kotlin among them, which has a real parser.
            "ruby" | "rb" => Some(Language::Ruby),
            "swift" => Some(Language::Swift),
            "kotlin" | "kt" => Some(Language::Kotlin),
            "dart" => Some(Language::Dart),
            "scala" => Some(Language::Scala),
            "php" => Some(Language::PHP),
            "groovy" => Some(Language::Groovy),
            "impex" => Some(Language::Impex),
            "svelte" => Some(Language::Svelte),
            "elevator" | "elv" => Some(Language::Elevator),
            "ansible" | "ansible-deploy" | "ansibledeploy" => Some(Language::AnsibleDeploy),
            "docker" | "dockerfile" | "compose" | "docker-compose" => Some(Language::Docker),
            "sql" | "postgres" | "postgresql" => Some(Language::Sql),
            "markdown" | "md" => Some(Language::Markdown),
            _ => None,
        }
    }

    /// The canonical token for this language in a filter list — the one
    /// `from_name` round-trips and the one the UI's checkboxes are keyed by.
    ///
    /// `from_name` accepts aliases (`rs`, `golang`, `ansible-deploy`); this
    /// picks the single spelling to *emit*. Needed because
    /// `GET /api/analysis/scope` reports the server's current filter and the
    /// UI has to match those strings against its own list. Deriving it from
    /// serde's lowercase rename would give `ansibledeploy`, which the UI
    /// spells `ansible` — close enough to look right and wrong enough to
    /// leave that checkbox unticked.
    pub fn filter_name(&self) -> &'static str {
        match self {
            Language::Rust => "rust",
            Language::Python => "python",
            Language::JavaScript => "javascript",
            Language::TypeScript => "typescript",
            Language::Java => "java",
            Language::Go => "go",
            Language::CSharp => "csharp",
            Language::Cpp => "cpp",
            Language::C => "c",
            Language::Ruby => "ruby",
            Language::Swift => "swift",
            Language::Kotlin => "kotlin",
            Language::Dart => "dart",
            Language::Scala => "scala",
            Language::PHP => "php",
            Language::Groovy => "groovy",
            Language::Impex => "impex",
            Language::Svelte => "svelte",
            Language::Elevator => "elevator",
            Language::AnsibleDeploy => "ansible",
            Language::Docker => "docker",
            Language::Sql => "sql",
            Language::Markdown => "markdown",
            Language::Unknown => "unknown",
        }
    }

    /// Whether the walker should skip this language unless the user names
    /// it explicitly (`-l <lang>`).
    ///
    /// Only Markdown qualifies today. Every other extension mezz claims
    /// belongs to source code, so finding one is evidence the user wants it
    /// analyzed. `.md` is different: a code repo is full of READMEs, ADRs,
    /// changelogs and issue trackers that are *about* the code rather than
    /// part of it. Claiming them by default would add hundreds of nodes to
    /// every existing graph — mezz's own checkout has 368 `.md` files
    /// against 175 `.rs` — and change what every current user sees without
    /// them asking. Opt-in keeps the doc layer a deliberate view.
    pub fn is_opt_in(&self) -> bool {
        matches!(self, Language::Markdown)
    }

    /// Whether a bare name defined in `other` may bind to a reference
    /// written in `self` (AN-014).
    ///
    /// Name resolution ranks candidates by path locality, and something
    /// always wins — so in a polyglot monorepo a TypeScript `SessionItem`
    /// bound to a Rust `struct SessionItem` in `backend/`, and the graph
    /// reported it as a dependency. Locality cannot separate those; only
    /// language can.
    ///
    /// This is a *family* relation, not equality, because several
    /// cross-language edges here are deliberate and load-bearing:
    /// Groovy resolves Spring beans into the Java entity space (GR-010),
    /// Impex header modifiers reference Java classes by FQN (IM-001), and
    /// a `.svelte` file's script members simply *are* TypeScript.
    ///
    /// `Unknown` interoperates with everything: the generic parser's
    /// output and any path-classified language that reaches here through
    /// an unclaimed extension must not start disappearing.
    pub fn interoperates_with(&self, other: Language) -> bool {
        let (a, b) = (self.family(), other.family());
        a == Family::Any || b == Family::Any || a == b
    }

    fn family(&self) -> Family {
        match self {
            Language::Java
            | Language::Groovy
            | Language::Kotlin
            | Language::Scala
            | Language::Impex => Family::Jvm,
            Language::TypeScript | Language::JavaScript | Language::Svelte => Family::Web,
            Language::C | Language::Cpp => Family::CLike,
            Language::Unknown => Family::Any,
            other => Family::Solo(*other as u8),
        }
    }

    /// Get display name for this language
    pub fn display_name(&self) -> &'static str {
        match self {
            Language::Rust => "Rust",
            Language::Python => "Python",
            Language::JavaScript => "JavaScript",
            Language::TypeScript => "TypeScript",
            Language::Java => "Java",
            Language::Go => "Go",
            Language::CSharp => "C#",
            Language::Cpp => "C++",
            Language::C => "C",
            Language::Ruby => "Ruby",
            Language::Swift => "Swift",
            Language::Kotlin => "Kotlin",
            Language::Dart => "Dart",
            Language::Scala => "Scala",
            Language::PHP => "PHP",
            Language::Groovy => "Groovy",
            Language::Impex => "Impex",
            Language::Svelte => "Svelte",
            Language::Elevator => "Elevator",
            Language::AnsibleDeploy => "Ansible Deploy",
            Language::Docker => "Docker",
            Language::Sql => "SQL",
            Language::Markdown => "Markdown",
            Language::Unknown => "Unknown",
        }
    }
}

/// A position within a source file (line and column).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
pub struct Position {
    /// Line number (0-indexed)
    pub line: usize,
    /// Column number (0-indexed, in bytes)
    pub column: usize,
    /// Byte offset from start of file
    pub offset: usize,
}

impl Position {
    pub fn new(line: usize, column: usize, offset: usize) -> Self {
        Self {
            line,
            column,
            offset,
        }
    }
}

/// A span (range) within a source file.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
pub struct Span {
    /// Start position
    pub start: Position,
    /// End position
    pub end: Position,
}

impl Span {
    pub fn new(start: Position, end: Position) -> Self {
        Self { start, end }
    }

    /// Create a span from line/column pairs
    pub fn from_positions(
        start_line: usize,
        start_col: usize,
        end_line: usize,
        end_col: usize,
    ) -> Self {
        Self {
            start: Position::new(start_line, start_col, 0),
            end: Position::new(end_line, end_col, 0),
        }
    }

    /// Check if this span contains a position
    pub fn contains(&self, pos: Position) -> bool {
        if pos.line < self.start.line || pos.line > self.end.line {
            return false;
        }
        if pos.line == self.start.line && pos.column < self.start.column {
            return false;
        }
        if pos.line == self.end.line && pos.column > self.end.column {
            return false;
        }
        true
    }

    /// Get the number of lines this span covers
    pub fn line_count(&self) -> usize {
        self.end.line - self.start.line + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `Language` the backend can parse, sans `Unknown`. Adding a
    /// variant means adding it here — which is the point: the assertion
    /// below then tells you the UI needs it too.
    const ALL: &[Language] = &[
        Language::Rust,
        Language::Python,
        Language::JavaScript,
        Language::TypeScript,
        Language::Java,
        Language::Go,
        Language::CSharp,
        Language::Cpp,
        Language::C,
        Language::Ruby,
        Language::Swift,
        Language::Kotlin,
        Language::Dart,
        Language::Scala,
        Language::PHP,
        Language::Groovy,
        Language::Impex,
        Language::Svelte,
        Language::Elevator,
        Language::AnsibleDeploy,
        Language::Docker,
        Language::Sql,
        Language::Markdown,
    ];

    /// The UI's "Parsed Languages" panel offers a hardcoded list, and it had
    /// silently drifted from this enum four times over — `svelte`, `sql`,
    /// `ansible` and `markdown` were all parseable while the panel that
    /// claims to control "which languages the analyzer reads at all" could
    /// not offer them. Nothing failed; the checkbox just wasn't there.
    ///
    /// Reading the TypeScript from a Rust test is crude, and it is still the
    /// cheapest thing that makes the drift *loud*. The alternative — serving
    /// the list from the backend — is the real fix and is not this test's
    /// job to force.
    #[test]
    fn the_ui_language_list_matches_the_enum() {
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/ui/src/utils/languageScope.ts"
        ))
        .expect("the UI language list must exist; move this test if the path changes");

        let body = source
            .split_once("ALL_ANALYSIS_LANGUAGES: readonly string[] = [")
            .expect("the exported list must still be named this")
            .1
            .split_once(']')
            .expect("unterminated list literal")
            .0;

        let listed: Vec<String> = body
            .split(',')
            .filter_map(|s| {
                s.trim()
                    .strip_prefix('\'')?
                    .strip_suffix('\'')
                    .map(str::to_string)
            })
            .collect();

        let listed: std::collections::BTreeSet<&str> = listed.iter().map(String::as_str).collect();
        let expected: std::collections::BTreeSet<&str> =
            ALL.iter().map(|l| l.filter_name()).collect();

        assert_eq!(
            expected, listed,
            "the UI's ALL_ANALYSIS_LANGUAGES must be exactly the canonical \
             filter names — left is the enum, right is the TypeScript"
        );

        for name in &listed {
            assert!(
                Language::from_name(name).is_some(),
                "the UI offers `{name}`, which `Language::from_name` rejects"
            );
        }
    }
}
