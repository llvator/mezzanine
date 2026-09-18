//! docker — topology parser for `Dockerfile`s and Compose files.
//!
//! Like ansible-deploy, this is not a code-quality parser. It recovers
//! what *builds* from what and what *runs* against what, in a pair of
//! file formats whose whole content is data:
//!
//! ```text
//! File (compose.yaml)
//!   └─contains→ Service (api)
//!                 ├─BuildsFrom→ Stage (app/Dockerfile#builder)
//!                 ├─DependsOn→  Service (db)
//!                 └─Requires→   Volume (pgdata)
//!
//! File (app/Dockerfile)
//!   ├─contains→ Stage (builder) ─Inherits→ BaseImage (node:20)
//!   └─contains→ Stage (runtime) ─CopiesFrom→ Stage (builder)
//! ```
//!
//! See ADR 0034 (docker build/run topology) for the model, and ADR 0003
//! for why the complexity metrics stay empty.
//!
//! ## The per-file / cross-file split
//!
//! `LanguageParser::parse` sees one file at a time, so the Compose side
//! cannot read the Dockerfile it names. It emits a `BuildsFrom` edge to
//! the *predicted* id instead, and the two sides meet post-merge in the
//! analyzer, which keys entities by id. Same trick the ansible parser
//! uses for `RendersFrom`.
//!
//! That prediction is exact when the service names a `target:` — the
//! stage id is a pure function of the resolved Dockerfile path and the
//! target name. Without a `target:`, Docker builds the *last* stage in
//! the file, which the Compose side has no way to know. Rather than
//! guess a name and emit an edge that dangles whenever the guess is
//! wrong, the edge then points at the Dockerfile's own `File` entity:
//! "builds from this Dockerfile", which is all the Compose file actually
//! says.
//!
//! ## Id scheme (must agree across both sides)
//!
//! - File:      `docker::file.<normalized path>`
//! - Stage:     `docker::stage.<normalized dockerfile path>#<name|index>`
//! - BaseImage: `docker::image.<image ref>`   (global — one node per ref)
//! - Service:   `docker::service.<normalized compose path>#<name>`
//! - Volume:    `docker::volume.<normalized compose path>#<name>`
//! - Network:   `docker::network.<normalized compose path>#<name>`
//!
//! Paths are normalized absolute paths (lexically, without touching the
//! filesystem), because that is the only key both sides can compute: the
//! Compose side resolves `context` + `dockerfile` against its own
//! directory, and the Dockerfile side knows only where it is. BaseImage
//! is deliberately *not* path-keyed — one `node:20` node that every stage
//! and service points at is what makes "which external images are we on"
//! a question the graph answers.

#[cfg(test)]
mod tests;

mod blocks;
mod compose;
mod dockerfile;

use super::language_parser::{LanguageParser, ParseResult};
use crate::models::file_info::Language;
use crate::models::{CodeEntity, EntityKind, Position, Relationship, Span, Visibility};
use anyhow::Result;
use std::path::{Component, Path, PathBuf};

pub struct DockerParser;

impl DockerParser {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DockerParser {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageParser for DockerParser {
    fn language(&self) -> Language {
        Language::Docker
    }

    fn parse(&self, path: &Path, content: &str) -> Result<ParseResult> {
        let mut result = ParseResult::new();
        match classify(path) {
            FileRole::Dockerfile => dockerfile::parse(path, content, &mut result),
            FileRole::Compose => compose::parse(path, content, &mut result),
            FileRole::Other => {}
        }
        Ok(result)
    }

    /// docker is filename-classified rather than extension-owned: the
    /// canonical `Dockerfile` has no extension, and `compose.yaml` shares
    /// `.yaml` with every other YAML in the repo.
    fn can_parse(&self, path: &Path) -> bool {
        !matches!(classify(path), FileRole::Other)
    }
}

// =====================================================================
// Filename classification
// =====================================================================

/// Which kind of Docker file this path is, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileRole {
    /// A `Dockerfile`, `Dockerfile.<variant>`, `<name>.Dockerfile`, or
    /// the Podman-compatible `Containerfile` spellings of each.
    Dockerfile,
    /// A Compose file, by the filenames the Compose specification itself
    /// lists — plus an optional middle segment for the override and
    /// per-environment convention (`docker-compose.prod.yml`).
    Compose,
    /// Not a Docker file.
    Other,
}

/// True if mezz should parse this path as Docker.
pub fn is_docker_file(path: &Path) -> bool {
    !matches!(classify(path), FileRole::Other)
}

/// Classify a path by its filename alone.
///
/// Filename-only, never directory layout: unlike ansible-deploy — where
/// `group_vars/` is the signal and the filename means nothing — a
/// Dockerfile is a Dockerfile wherever it sits, and a `docker/` directory
/// is full of files that are not.
pub(crate) fn classify(path: &Path) -> FileRole {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return FileRole::Other;
    };
    if is_dockerfile_name(name) {
        return FileRole::Dockerfile;
    }
    if is_compose_name(name) {
        return FileRole::Compose;
    }
    FileRole::Other
}

/// The base names a Dockerfile goes by, lowercased. `Containerfile` is
/// Podman's spelling of the same format and parses identically.
const DOCKERFILE_STEMS: &[&str] = &["dockerfile", "containerfile"];

/// True for `Dockerfile`, `Dockerfile.prod`, `prod.Dockerfile`, and the
/// `Containerfile` equivalents. Case-insensitive, because the convention
/// is capitalised but the filesystem is not always.
fn is_dockerfile_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    DOCKERFILE_STEMS.iter().any(|stem| {
        // `Dockerfile` exactly, or `prod.Dockerfile` (prefixed variant).
        lower == *stem || lower.ends_with(&format!(".{stem}")) || is_variant(&lower, stem)
    })
}

/// `Dockerfile.prod` — the suffixed-variant spelling — but not
/// `dockerfile.rs`.
///
/// The naive `starts_with("dockerfile.")` claimed this module's own
/// `dockerfile.rs`, and would claim a `dockerfile.ts` or `dockerfile.go`
/// in any repo with code *about* Docker. A trailing extension that some
/// language owns means the file is that language's source, whatever its
/// stem says — so the variant suffix has to be a name (`prod`, `ci`)
/// rather than an extension.
fn is_variant(lower: &str, stem: &str) -> bool {
    if !lower.starts_with(&format!("{stem}.")) {
        return false;
    }
    let ext = Path::new(lower)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    Language::from_extension(ext) == Language::Unknown
}

/// True for the Compose specification's filenames, with an optional
/// middle segment: `compose.yaml`, `compose.yml`, `docker-compose.yaml`,
/// `docker-compose.yml`, and `<either>.<anything>.<either ext>`.
///
/// A filename match is necessary but not sufficient — `compose::parse`
/// additionally requires a top-level `services:` mapping before claiming
/// the file, so a repo whose `compose.yml` means something else yields an
/// empty parse rather than a wrong graph.
fn is_compose_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    let Some(stem) = lower
        .strip_suffix(".yaml")
        .or_else(|| lower.strip_suffix(".yml"))
    else {
        return false;
    };
    for prefix in ["docker-compose", "compose"] {
        if stem == prefix || stem.starts_with(&format!("{prefix}.")) {
            return true;
        }
    }
    false
}

// =====================================================================
// Shared id + entity helpers
// =====================================================================

/// Lexically normalize a path for use as an id key: drop `.` components
/// and fold `..` into the parent where one is present.
///
/// Deliberately not `canonicalize`: that hits the filesystem, fails for a
/// path that does not exist (a Compose file naming a Dockerfile that was
/// deleted still deserves an edge), and resolves symlinks, which would
/// make the two sides of a `BuildsFrom` disagree whenever one of them
/// reached the file by a different route.
pub(crate) fn normalize(path: &Path) -> String {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out.to_string_lossy().replace('\\', "/")
}

/// The id of the `File` entity for a Docker file.
pub(crate) fn file_id(path: &Path) -> String {
    format!("docker::file.{}", normalize(path))
}

/// The id of a stage in a Dockerfile, by the name `COPY --from=` and
/// Compose's `target:` would use for it.
pub(crate) fn stage_id(dockerfile: &Path, stage: &str) -> String {
    format!("docker::stage.{}#{}", normalize(dockerfile), stage)
}

/// The id of an external image. Global rather than per-file, so every
/// reference to `node:20` lands on one node.
pub(crate) fn image_id(image: &str) -> String {
    format!("docker::image.{image}")
}

/// Byte offset of the start of each line, plus the file length — enough
/// to turn any line range into a `Span` with real offsets and into the
/// text those lines hold.
///
/// Built once per file. Both formats here are line-oriented (a `FROM`
/// block, a YAML mapping), so a line table is the whole of what either
/// side needs to locate anything.
pub(crate) struct Lines<'a> {
    content: &'a str,
    starts: Vec<usize>,
}

impl<'a> Lines<'a> {
    pub(crate) fn index(content: &'a str) -> Self {
        let mut starts = vec![0];
        starts.extend(
            content
                .char_indices()
                .filter(|(_, c)| *c == '\n')
                .map(|(i, c)| i + c.len_utf8()),
        );
        Self { content, starts }
    }

    /// How many lines the file has.
    pub(crate) fn count(&self) -> usize {
        self.starts.len()
    }

    /// The text of one line, without its newline.
    fn line_text(&self, line: usize) -> &str {
        match self.starts.get(line) {
            Some(&start) => &self.content[start..self.line_end(line)],
            None => "",
        }
    }

    /// The `Span` covering lines `first..=last`, clamped to the file and
    /// ending at the last line that actually holds something.
    ///
    /// The trailing trim is why this is the only place spans are built: a
    /// stage ends where the next `FROM` begins and a service ends where
    /// the next one does, so both arrive here with the blank separator
    /// line included. An entity whose span runs past its own last
    /// instruction reads, in the details pane, as though the empty space
    /// were part of it.
    pub(crate) fn span(&self, first: usize, last: usize) -> Span {
        let first = first.min(self.count().saturating_sub(1));
        let mut last = last.max(first).min(self.count().saturating_sub(1));
        while last > first && self.line_text(last).trim().is_empty() {
            last -= 1;
        }
        let start = self.starts[first];
        let end = self.line_end(last);
        Span::new(
            Position::new(first, 0, start),
            Position::new(last, end - self.starts[last], end),
        )
    }

    /// Byte offset of the end of `line`, excluding its newline.
    fn line_end(&self, line: usize) -> usize {
        self.starts
            .get(line + 1)
            .map(|next| next.saturating_sub(1))
            .unwrap_or(self.content.len())
            .min(self.content.len())
    }

    /// The text of lines `first..=last`, with trailing blank lines
    /// trimmed — a stage that is followed by two blank lines before the
    /// next `FROM` should not show them as part of itself.
    pub(crate) fn text(&self, first: usize, last: usize) -> String {
        let span = self.span(first, last);
        self.content[span.start.offset..span.end.offset]
            .trim_end()
            .to_string()
    }
}

/// Build one entity with the conventions every Docker node shares:
/// public visibility, the `docker` tag the UI keys metric-free rendering
/// off, and — when the caller knows the line range — the span and source
/// text the details pane shows.
///
/// `lines` is what makes an entity selectable-and-readable rather than a
/// bare name: without it the span collapses to a zero-width point and the
/// pane has nothing to display (DK-001).
pub(crate) fn new_entity(
    name: &str,
    kind: EntityKind,
    path: &Path,
    span: Span,
    source: Option<String>,
) -> CodeEntity {
    let mut e = CodeEntity::new(name, kind, path, span);
    e.visibility = Visibility::Public;
    e.tags.insert("docker".to_string());
    e.source_code = source;
    e
}

/// The common case: an entity defined by lines `first..=last`.
pub(crate) fn entity_at(
    name: &str,
    kind: EntityKind,
    path: &Path,
    lines: &Lines,
    first: usize,
    last: usize,
) -> CodeEntity {
    new_entity(
        name,
        kind,
        path,
        lines.span(first, last),
        Some(lines.text(first, last)),
    )
}

/// Emit the `File` node a Docker file's contents hang off, and return its
/// id. Spans the whole file, which is what it is.
pub(crate) fn emit_file(path: &Path, lines: &Lines, result: &mut ParseResult) -> String {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Dockerfile");
    let id = file_id(path);
    let mut entity = entity_at(
        name,
        EntityKind::File,
        path,
        lines,
        0,
        lines.count().saturating_sub(1),
    );
    entity.id = id.clone();
    entity.qualified_name = normalize(path);
    result.add_entity(entity);
    id
}

/// Emit an external image node if this parse has not already, and return
/// its id. Deduplicated within the file; across files the analyzer merges
/// by id.
///
/// `line` is where the image is first *referenced*, not defined — an
/// external image has no definition in this repo. The span points at the
/// `FROM` or `image:` that brought it into the graph, which is the most
/// useful thing a reader selecting it can be shown.
pub(crate) fn ensure_image(
    image: &str,
    path: &Path,
    lines: &Lines,
    line: usize,
    result: &mut ParseResult,
) -> String {
    let id = image_id(image);
    if !result.entities.iter().any(|e| e.id == id) {
        let mut entity = entity_at(image, EntityKind::BaseImage, path, lines, line, line);
        entity.id = id.clone();
        entity.qualified_name = image.to_string();
        result.add_entity(entity);
    }
    id
}

/// Shorthand for the `add_relationship(Relationship::new(..))` pair this
/// module writes on almost every line.
pub(crate) fn link(
    source: &str,
    target: &str,
    kind: crate::models::RelationshipKind,
    result: &mut ParseResult,
) {
    result.add_relationship(Relationship::new(source, target, kind));
}
