//! The `Dockerfile` side: stages, the images they start from, and the
//! `COPY --from=` edges between them.

use super::{emit_file, ensure_image, entity_at, link, stage_id, Lines};
use crate::models::{EntityKind, RelationshipKind};
use crate::parser::language_parser::ParseResult;
use std::path::Path;

/// One logical instruction: a physical line plus every line its trailing
/// backslash pulled in.
struct LogicalLine {
    /// 0-indexed line the instruction starts on.
    line: usize,
    text: String,
}

/// The stages declared so far, in file order. `COPY --from=` resolves
/// against this: by index (`--from=0`) or by name (`--from=builder`).
#[derive(Default)]
struct Stages {
    /// `None` for an unnamed stage — it is addressable only by index.
    names: Vec<Option<String>>,
    ids: Vec<String>,
}

impl Stages {
    /// The two names the next stage takes: the key it is addressed by —
    /// which is also its id suffix — and the label a reader sees.
    ///
    /// Both fall back to the stage's index when it has no `AS` name, which
    /// is what `COPY --from=0` addresses it by.
    fn next_names(&self, name: Option<&str>) -> (String, String) {
        let index = self.ids.len();
        match name {
            Some(n) => (n.to_string(), n.to_string()),
            None => (index.to_string(), format!("stage {index}")),
        }
    }

    /// Resolve a `--from=` reference to a stage id, or `None` when it
    /// names something this file does not declare — an external image.
    fn resolve(&self, reference: &str) -> Option<&str> {
        if let Ok(index) = reference.parse::<usize>() {
            return self.ids.get(index).map(String::as_str);
        }
        self.names
            .iter()
            .position(|n| n.as_deref() == Some(reference))
            .and_then(|i| self.ids.get(i))
            .map(String::as_str)
    }
}

/// The walk's running state: where we are and what the file has declared
/// so far. A struct rather than five parameters threaded through each
/// instruction handler — mezz called that shape an Overfull Head, and it
/// was right.
struct Walk<'a> {
    path: &'a Path,
    /// Id of the `File` entity every stage hangs off.
    file: String,
    stages: Stages,
    lines: &'a Lines<'a>,
    /// The stage still being built: its index in `result.entities` and the
    /// line its `FROM` sits on. A stage's extent is only known once the
    /// next `FROM` arrives, so it is patched shut rather than emitted
    /// complete.
    open: Option<(usize, usize)>,
}

pub(super) fn parse(path: &Path, content: &str, result: &mut ParseResult) {
    let lines = Lines::index(content);
    let mut walk = Walk {
        path,
        file: emit_file(path, &lines, result),
        stages: Stages::default(),
        lines: &lines,
        open: None,
    };

    for logical in logical_lines(content) {
        let mut tokens = logical.text.split_whitespace();
        let Some(instruction) = tokens.next() else {
            continue;
        };
        let rest: Vec<&str> = tokens.collect();
        match instruction.to_uppercase().as_str() {
            "FROM" => walk.open_stage(&logical, &rest, result),
            "COPY" | "ADD" | "RUN" => walk.copy_from(&rest, logical.line, result),
            _ => {}
        }
    }
    // The last stage runs to the end of the file.
    walk.close_stage(lines.count().saturating_sub(1), result);
}

impl Walk<'_> {
    /// `FROM [--platform=…] <image> [AS <name>]`
    ///
    /// The image may name an earlier stage in the same file, which is how
    /// a stage extends another rather than an external base. That case
    /// emits an `Inherits` to the sibling stage and no `BaseImage` at all.
    fn open_stage(&mut self, logical: &LogicalLine, tokens: &[&str], result: &mut ParseResult) {
        let Some((image, name)) = split_from(tokens) else {
            return;
        };
        // The stage this `FROM` ends runs to the line before it.
        self.close_stage(logical.line.saturating_sub(1), result);

        let id = self.emit_stage(name, logical.line, result);
        let parent = self.base_of(image, logical.line, result);
        link(&id, &parent, RelationshipKind::Inherits, result);

        self.stages.names.push(name.map(str::to_string));
        self.stages.ids.push(id);
    }

    /// Emit the `Stage` entity for the `FROM` being opened and hang it off
    /// the file. Returns its id.
    ///
    /// The entity goes out with a one-line span covering its `FROM`;
    /// `close_stage` widens it to the stage's real extent once the end is
    /// known.
    fn emit_stage(&mut self, name: Option<&str>, line: usize, result: &mut ParseResult) -> String {
        let (key, display) = self.stages.next_names(name);
        let id = stage_id(self.path, &key);
        let mut entity = entity_at(
            &display,
            EntityKind::Stage,
            self.path,
            self.lines,
            line,
            line,
        );
        entity.id = id.clone();
        entity.qualified_name = format!("{}#{}", super::normalize(self.path), key);
        self.open = Some((result.entities.len(), line));
        result.add_entity(entity);
        link(&self.file, &id, RelationshipKind::Contains, result);
        id
    }

    /// Widen the stage still open so it spans down to `last`, and give it
    /// the text of those lines — what the details pane shows when a reader
    /// selects it.
    fn close_stage(&mut self, last: usize, result: &mut ParseResult) {
        let Some((index, first)) = self.open.take() else {
            return;
        };
        let Some(entity) = result.entities.get_mut(index) else {
            return;
        };
        entity.span = self.lines.span(first, last);
        entity.source_code = Some(self.lines.text(first, last));
    }

    /// What a stage starts from: the sibling stage it names, or — when the
    /// file declares no such stage — an external image.
    fn base_of(&self, image: &str, line: usize, result: &mut ParseResult) -> String {
        match self.stages.resolve(image) {
            Some(sibling) => sibling.to_string(),
            None => ensure_image(image, self.path, self.lines, line, result),
        }
    }

    /// `COPY --from=<stage|image> …`, and the `RUN --mount=…,from=<stage>`
    /// that does the same thing through a bind mount.
    ///
    /// Emitted from the stage currently being built, which is the last one
    /// declared. An instruction before any `FROM` has no owning stage and
    /// is skipped.
    fn copy_from(&self, tokens: &[&str], line: usize, result: &mut ParseResult) {
        let Some(current) = self.stages.ids.last() else {
            return;
        };
        for reference in tokens.iter().filter_map(|t| from_flag(t)) {
            let target = match self.stages.resolve(reference) {
                Some(stage) => stage.to_string(),
                None => ensure_image(reference, self.path, self.lines, line, result),
            };
            // A stage copying from itself is not a dependency, and
            // `--from=` naming the current stage is legal but inert.
            if target != *current {
                link(current, &target, RelationshipKind::CopiesFrom, result);
            }
        }
    }
}

/// Split a `FROM`'s tokens into the image it starts from and the `AS`
/// name it takes, skipping flags like `--platform=`.
fn split_from<'a>(tokens: &[&'a str]) -> Option<(&'a str, Option<&'a str>)> {
    let mut words = tokens.iter().filter(|t| !t.starts_with("--"));
    let image = words.next()?;
    // `AS <name>` — the only thing that can follow the image.
    let name = match (words.next(), words.next()) {
        (Some(kw), Some(n)) if kw.eq_ignore_ascii_case("as") => Some(*n),
        _ => None,
    };
    Some((image, name))
}

/// The stage reference carried by a token, for the two spellings that
/// carry one: `--from=builder` and `--mount=type=bind,from=builder`.
fn from_flag(token: &str) -> Option<&str> {
    if let Some(value) = token.strip_prefix("--from=") {
        return Some(value);
    }
    let mount = token.strip_prefix("--mount=")?;
    mount
        .split(',')
        .find_map(|field| field.strip_prefix("from="))
}

/// Fold a Dockerfile's physical lines into logical ones: strip comments,
/// and join any line whose trailing backslash continues it.
///
/// Comment lines are dropped *inside* a continuation too, which is what
/// the builder does — a `#` line in the middle of a multi-line `RUN` is
/// not part of the command.
fn logical_lines(content: &str) -> Vec<LogicalLine> {
    let mut out: Vec<LogicalLine> = Vec::new();
    let mut pending: Option<LogicalLine> = None;

    for (index, raw) in content.lines().enumerate() {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let continues = trimmed.ends_with('\\');
        let body = trimmed.trim_end_matches('\\').trim_end();

        let mut current = pending.take().unwrap_or(LogicalLine {
            line: index,
            text: String::new(),
        });
        if !current.text.is_empty() {
            current.text.push(' ');
        }
        current.text.push_str(body);

        if continues {
            pending = Some(current);
        } else {
            out.push(current);
        }
    }
    // A file ending mid-continuation still stated an instruction.
    out.extend(pending);
    out
}
