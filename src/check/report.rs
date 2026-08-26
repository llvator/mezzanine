//! What `check` says, for a person and for CI.
//!
//! Both formats say the same things in the same order; the JSON exists so a
//! CI job can act on a verdict without parsing prose, not so it can learn
//! anything the human output withholds.

use serde_json::{json, Map, Value};

use super::{Cited, Outcome, Violation};

/// Which rendering of the verdict the caller asked for.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum Format {
    /// Lines an author reads in a terminal. The default.
    #[default]
    Human,
    /// One JSON object, for a CI consumer.
    Json,
}

/// Print the verdict.
///
/// A human error goes to stderr, where errors belong and where it cannot be
/// mistaken for a finding. A JSON one goes to stdout with everything else,
/// so a consumer parsing one stream always has an object to parse — a gate
/// that emitted nothing on failure is how a broken rules file comes to read
/// as a clean tree.
pub(super) fn print(outcome: &Outcome, format: Format) {
    if let (Format::Human, Outcome::Unusable { message }) = (format, outcome) {
        eprintln!("{message}");
        return;
    }
    match format {
        Format::Human => println!("{}", human(outcome)),
        Format::Json => println!("{}", json_text(outcome)),
    }
}

/// The human rendering, as one block of text. Returned rather than printed
/// so the tests can read it.
pub(super) fn human(outcome: &Outcome) -> String {
    match outcome {
        Outcome::NoRules { path } => format!("no rules declared in {path}"),
        Outcome::Unusable { message } => message.clone(),
        Outcome::Checked {
            rules_file,
            declared,
            violations,
            files_checked,
            files_exempt,
        } => {
            let mut lines: Vec<String> = violations.iter().map(line).collect();
            lines.push(format!(
                "{} · {} checked, {files_exempt} exempt · {} from {rules_file}",
                super::tally(violations.len(), "violation"),
                super::tally(*files_checked, "file"),
                super::tally(declared.len(), "rule"),
            ));
            lines.join("\n")
        }
    }
}

/// One violation, as the author will read it: where to look, which rule,
/// what was measured, where that came from, and the bar they wrote.
///
/// `pub(super)` so the push-mode hook (MCP-019) prints the same sentence as
/// the command. A hook with its own phrasing is a second place for the rule
/// to be described, and the two drift.
pub(super) fn line(v: &Violation) -> String {
    format!(
        "{}{} — {}: {}{}, bar is {}",
        v.path,
        v.line.map(|n| format!(":{n}")).unwrap_or_default(),
        v.rule.name(),
        v.rule.phrase(v.measured, v.subject.as_deref()),
        named(&v.path, &v.names),
        v.bar
    )
}

/// The places behind the count, all of them. A rule that says a file has
/// two importers and stops has told the author to go and find them.
fn named(subject: &str, names: &[Cited]) -> String {
    if names.is_empty() {
        return String::new();
    }
    let shown: Vec<String> = names.iter().map(|cited| cite(subject, cited)).collect();
    format!(" ({})", shown.join(", "))
}

/// One cited place, on a line that has already named the subject: a door
/// drops the folder in front of it, having just been told which folder, and
/// an importer — never inside the file it imports — keeps its whole path.
fn cite(subject: &str, cited: &Cited) -> String {
    let path = cited
        .path
        .strip_prefix(&format!("{subject}/"))
        .unwrap_or(&cited.path);
    format!(
        "{path}{}{}",
        cited.line.map(|n| format!(":{n}")).unwrap_or_default(),
        cited
            .via
            .as_ref()
            .map(|shim| format!(" via {shim}"))
            .unwrap_or_default(),
    )
}

/// The JSON rendering, as one line of text.
pub(super) fn json_text(outcome: &Outcome) -> String {
    let value = as_json(outcome);
    serde_json::to_string_pretty(&value).unwrap_or_else(|e| value_error(&e.to_string()))
}

fn value_error(message: &str) -> String {
    json!({"status": "error", "message": message}).to_string()
}

fn as_json(outcome: &Outcome) -> Value {
    match outcome {
        Outcome::NoRules { path } => json!({"status": "no-rules", "rules_file": path}),
        Outcome::Unusable { message } => json!({"status": "error", "message": message}),
        Outcome::Checked {
            rules_file,
            declared,
            violations,
            files_checked,
            files_exempt,
        } => json!({
            "status": if violations.is_empty() { "pass" } else { "violations" },
            "rules_file": rules_file,
            "rules": declared
                .iter()
                .map(|(rule, bar)| (rule.name().to_string(), json!(bar)))
                .collect::<Map<String, Value>>(),
            "files_checked": files_checked,
            "files_exempt": files_exempt,
            "violations": violations.iter().map(as_json_violation).collect::<Vec<_>>(),
        }),
    }
}

fn as_json_violation(v: &Violation) -> Value {
    json!({
        "rule": v.rule.name(),
        // `path` rather than `file`: `max_doors_per_folder` grades a folder,
        // and a consumer told that key holds a file would parse three of the
        // four rules correctly.
        "path": v.path,
        // Null for a subject with no line — a folder is not a place in a
        // file.
        "line": v.line,
        // Absent for a rule whose subject is the path itself, rather than
        // repeating the path under a second key.
        "entity": v.subject,
        "measured": v.measured,
        // The importers, the doors: every place behind `measured`, so a CI
        // consumer can say what the human line says.
        "names": v.names.iter().map(as_json_cited).collect::<Vec<_>>(),
        "bar": v.bar,
    })
}

fn as_json_cited(cited: &Cited) -> Value {
    json!({
        "path": cited.path,
        // Null rather than absent for both: a consumer reading a fixed set
        // of keys is the reason this is JSON at all.
        "line": cited.line,
        "via": cited.via,
    })
}
