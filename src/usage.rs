//! The command list `mezz --help` prints.
//!
//! Clap lists subcommands in declaration order under one `Commands:`
//! heading, and offers no way to group them — so thirty commands arrived as
//! one undifferentiated column, each followed by the whole first paragraph
//! of its doc comment, and nothing said what any of them was pointed *at*.
//! A reader looking for "what do I run on a folder" had to read all thirty
//! and infer the answer from the prose.
//!
//! This replaces that column with groups, one per kind of input: the
//! heading says what the commands under it take, the row says what the
//! command answers. The argument signature on each row — `[PATH]`,
//! `<QUERY>`, `--from <FROM>` — is read off clap's own model rather than
//! written here, so a command that gains or loses an argument says so
//! without anyone remembering to edit this file. Only the grouping and the
//! one-line summaries live here, and [`GROUPS`] is checked against the real
//! subcommand list by a test, so a new command cannot quietly go unlisted.

use clap::{Arg, Command};

/// A run of commands that take the same kind of input.
pub struct Group {
    /// The heading, which ends in what every command below it is pointed at.
    pub heading: &'static str,

    /// `(subcommand name, one-line summary)`, in reading order. The summary
    /// is deliberately shorter than the command's own `about`: this list is
    /// for choosing a command, and `mezz <command> --help` is for using it.
    pub commands: &'static [(&'static str, &'static str)],
}

/// Every `mezz` subcommand, grouped by what it takes.
///
/// One line per command: rustfmt would break any pair wider than sixty
/// columns across four lines, which turns a table you can read down into a
/// hundred and forty lines you cannot.
#[rustfmt::skip]
pub const GROUPS: &[Group] = &[
    Group {
        heading: "Set up a repo — takes a repo root (default: the current directory)",
        commands: &[(
            "init",
            "Write .mezz/settings.json, VS Code tasks, MCP, hooks",
        )],
    },
    Group {
        heading: "Look at an area — take a folder or file, or nothing for the whole tree",
        commands: &[
            ("map", "Files, entities, metrics and coupling"),
            ("quality", "Smells, complexity offenders, cycles, folder shape"),
            ("hotspots", "Risk ranking: git churn × complexity"),
            ("dead-code", "Entities nothing references — deletion candidates"),
            ("reshape", "The one change that would improve its structure"),
            ("layout", "Where files would sit if the dependencies decided"),
            ("boundaries", "Imports that reach past another folder's door"),
            ("overview", "Domain shape from the Elevator (.elv) specs"),
            ("spec-slice", "That folder's spec claims, as standalone .elv"),
        ],
    },
    Group {
        heading: "Ask about one entity — take --entity <NAME>, or --path <FILE> --line <N>",
        commands: &[
            ("impact", "Blast radius of changing it (a whole file when --line is omitted)"),
            ("cost", "How it scales: worst-case time, composed along the call chain"),
            ("context", "The minimal context pack for editing it"),
            ("tests-for", "Which tests exercise it, directly or transitively"),
        ],
    },
    Group {
        heading: "Search the graph — take entity names, or free text",
        commands: &[
            ("trace", "Shortest dependency path between two entities"),
            ("similar", "Does something like this exist already?"),
        ],
    },
    Group {
        heading: "Review a change — take a git ref, compared against the working tree",
        commands: &[
            ("assess-change", "Metric deltas vs a base ref, for self-review"),
            ("pr-report", "The same report, as a PR comment body"),
            ("diff", "Structural changes between two commits, as JSON"),
            ("check", "Grade the tree against .mezz/rules.json"),
            ("hook", "Push-mode reports for a Claude Code hook"),
        ],
    },
    Group {
        heading: "Keep a view open — take a repo root, then hold the terminal",
        commands: &[
            ("watch", "Live-updating browser UI over a watched tree"),
            ("monitor", "Live terminal dashboard: quality over time"),
            ("serve", "Several analyzed repos at once, chosen in the UI"),
            ("mcp", "The graph tools over stdio, for an AI agent"),
        ],
    },
    Group {
        heading: "Export the whole graph — take a repo root",
        commands: &[
            ("analyze", "The graph as ascii, dot, mermaid or json"),
            ("cycles", "Circular dependencies, listed"),
        ],
    },
    Group {
        heading: "Java Educator — take a file, or a language name",
        commands: &[
            ("educate", "Every Educator rule a file trips, linter-style"),
            ("construct-kinds", "Construct-kind catalog markdown"),
            ("educator-index", "Rules and lessons by construct-kind"),
        ],
    },
    Group {
        heading: "Read an answer — takes one abbreviation, or nothing for the whole key",
        commands: &[("explain", "What cx, cog, ws, in, out, cycle and ⚠ mean")],
    },
    Group {
        heading: "Superseded — these still work; the named command answers better",
        commands: &[
            ("deps", "use `impact`: what a file depends on, and the reverse"),
            ("find", "use `similar`: names containing a substring"),
            ("stats", "use `quality`: raw tallies over the graph"),
        ],
    },
];

/// The help template that replaces clap's flat `Commands:` block with
/// [`GROUPS`], rendered against `cmd`'s own subcommands.
///
/// The list goes in the template rather than in `after_help` because clap
/// re-wraps `after_help` to the terminal width, which would fold the
/// summary column into the command column on a narrow window. Template text
/// is emitted verbatim, so the rows stay aligned; keeping every line inside
/// 80 columns is then this file's job, and a test holds it to that.
pub fn with_grouped_help(cmd: Command) -> Command {
    let template = format!(
        "{{about-with-newline}}\n\
         {{usage-heading}} {{usage}}\n\
         \n\
         {}Options:\n\
         {{options}}\n\
         \n\
         Run `mezz <COMMAND> --help` for that command's own flags, and\n\
         `mezz explain` for what the abbreviations in an answer mean.",
        command_list(&cmd)
    );
    cmd.help_template(template)
}

/// The grouped list itself: a heading per group, then one aligned row per
/// command.
pub fn command_list(cmd: &Command) -> String {
    let mut out = String::new();
    for group in GROUPS {
        out.push_str(group.heading);
        out.push('\n');
        let rows: Vec<(String, &str)> = group
            .commands
            .iter()
            .filter_map(|(name, summary)| {
                let sub = cmd.find_subcommand(name)?;
                Some((invocation(name, sub), *summary))
            })
            .collect();
        let width = rows
            .iter()
            .map(|(call, _)| call.chars().count())
            .max()
            .unwrap_or(0);
        for (call, summary) in &rows {
            let pad = " ".repeat(width - call.chars().count());
            out.push_str(&format!("  {call}{pad}  {summary}\n"));
        }
        out.push('\n');
    }
    out
}

/// How a command is typed: its name, the positionals it accepts, and the
/// options it cannot run without.
///
/// Optional flags are left out — there are up to fifteen of them on a
/// command like `watch`, and this list exists to say what a command is
/// *about*, not to replace its own `--help`.
fn invocation(name: &str, sub: &Command) -> String {
    let mut parts = vec![name.to_string()];
    for arg in sub.get_positionals() {
        let value = value_name(arg);
        parts.push(if arg.is_required_set() {
            format!("<{value}>")
        } else {
            format!("[{value}]")
        });
    }
    for arg in sub.get_arguments() {
        if arg.is_positional() || !arg.is_required_set() {
            continue;
        }
        if let Some(long) = arg.get_long() {
            parts.push(format!("--{long} <{}>", value_name(arg)));
        }
    }
    if sub.get_subcommands().next().is_some() {
        parts.push("<COMMAND>".to_string());
    }
    parts.join(" ")
}

/// What clap would print for an argument's value: its declared value name,
/// or its id in the upper case clap uses when none was given.
fn value_name(arg: &Arg) -> String {
    arg.get_value_names()
        .and_then(|names| names.first())
        .map(|name| name.to_string())
        .unwrap_or_else(|| arg.get_id().as_str().to_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    /// A command clap knows about and this file does not is a command that
    /// vanished from `mezz --help`: the template prints [`GROUPS`], so an
    /// unlisted subcommand still parses and still works while being
    /// invisible to every reader who goes looking for it. That is a worse
    /// failure than the flat list this replaced, and it is what this test
    /// exists to make impossible.
    #[test]
    fn every_subcommand_is_in_exactly_one_group() {
        let cmd = crate::Cli::command();
        let listed: Vec<&str> = GROUPS
            .iter()
            .flat_map(|g| g.commands.iter().map(|(name, _)| *name))
            .collect();

        for name in &listed {
            assert_eq!(
                listed.iter().filter(|other| *other == name).count(),
                1,
                "`{name}` is grouped twice"
            );
            assert!(
                cmd.find_subcommand(name).is_some(),
                "`{name}` is grouped but is not a subcommand"
            );
        }

        for sub in cmd.get_subcommands() {
            let name = sub.get_name();
            assert!(
                name == "help" || listed.contains(&name),
                "`{name}` is a subcommand but no group lists it"
            );
        }
    }

    /// The rows are aligned with spaces, and clap emits the template
    /// verbatim — so a row longer than the window wraps and the column
    /// breaks. Eighty is the narrow terminal this has to survive.
    #[test]
    fn no_line_runs_past_eighty_columns() {
        let cmd = crate::Cli::command();
        for line in command_list(&cmd).lines() {
            assert!(
                line.chars().count() <= 80,
                "{} columns: {line}",
                line.chars().count()
            );
        }
        for group in GROUPS {
            assert!(
                group.heading.chars().count() <= 80,
                "heading runs past 80 columns: {}",
                group.heading
            );
        }
    }

    /// The signature is read off clap, not written down: a required
    /// positional prints as `<NAME>`, an optional one as `[NAME]`, and a
    /// required option comes after them.
    #[test]
    fn an_invocation_shows_what_a_command_takes() {
        let cmd = crate::Cli::command();
        let of = |name: &str| invocation(name, cmd.find_subcommand(name).expect(name));

        assert_eq!(of("map"), "map [PATH]");
        assert_eq!(of("similar"), "similar <QUERY>");
        assert_eq!(of("diff"), "diff [PATH] --from <FROM>");
        assert_eq!(of("hook"), "hook <COMMAND>");
    }
}
