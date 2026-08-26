//! What the settings resolved to, and which link of the chain decided it.
//!
//! [`super::load`] answers "what is the value". It cannot answer "why", and
//! why is the part nobody can reconstruct: `max_depth` is 3, and that 3 could
//! be a flag in the VS Code task that launched the server, this repo's file,
//! the machine-wide file, or nothing at all. Four sources collapse into one
//! number and [`super::Settings::over`] discards the seam as it merges.
//!
//! So the report is built from the scopes *before* they merge ([`super::Loaded`]),
//! against the values the process actually settled on. Each row says what the
//! value is, which link supplied it, and which of the three tiers the key
//! belongs to — because those tiers decide what the UI may offer to edit:
//!
//! | Tier | Editable | Why |
//! |---|---|---|
//! | [`Tier::Analysis`] | yes, by re-analyzing | the keys that decide what gets parsed |
//! | [`Tier::View`] | no | the filter panel and saved views already own these |
//! | [`Tier::Process`] | no | they cannot take effect without a restart |
//!
//! Two keys resist the one-source-won story and are reported honestly rather
//! than forced into it. See [`widened`] and [`patterns`].

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{json, Value};

use super::{Loaded, Settings, Warning};
use crate::config::Config;

/// Which link of the precedence chain supplied a value.
///
/// Ordered highest-priority first, which is also the order
/// [`Row::sources`] lists them in when more than one contributed.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Origin {
    /// Typed on the command line that started this process.
    Flag,
    /// One of the `MEZZ_*` environment variables.
    Env,
    /// `<analyzed-root>/.mezz/settings.json`.
    RepoFile,
    /// `~/.config/mezz/settings.json`.
    UserFile,
    /// Nobody set it.
    Default,
}

/// What a key controls, which is what decides whether it can be edited here.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tier {
    /// Decides what the analyzer parses. Changing one means re-analyzing.
    Analysis,
    /// Decides what reaches the canvas out of what was parsed.
    View,
    /// Describes the process or the installation.
    Process,
}

/// One key, as resolved.
#[derive(Clone, Debug, Serialize)]
pub struct Row {
    pub key: &'static str,
    pub tier: Tier,
    /// The settled value, `null` where the key is genuinely unset.
    pub value: Value,
    /// Every link that contributed, highest first. Exactly one entry unless
    /// the key merges rather than overrides.
    pub sources: Vec<Origin>,
    /// Set only where one badge would misrepresent the merge.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<&'static str>,
    /// Per-item origin, for the keys whose value is a concatenated list.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<Entry>,
}

/// One element of a list-valued key, with the scope that contributed it.
#[derive(Clone, Debug, Serialize)]
pub struct Entry {
    pub value: String,
    pub source: Origin,
}

/// The values this process settled on that live outside [`Config`].
///
/// The analysis and view tiers can be read off the `Config` the analyzer is
/// using. The process tier cannot — `port` and `debounce_ms` were consumed at
/// startup and never stored anywhere else — so the server hands them over.
#[derive(Clone, Debug, Default)]
pub struct Effective {
    pub port: Option<u16>,
    pub debounce_ms: Option<u64>,
    pub output_dir: Option<PathBuf>,
    pub ui_dir: Option<PathBuf>,
    pub content_fallback: Option<PathBuf>,
}

/// Everything needed to attribute a value to a link of the chain.
pub struct Inputs<'a> {
    pub loaded: &'a Loaded,
    /// Keys this process was given on its command line. Names match
    /// [`Row::key`]; see [`named`] for the builder.
    pub flags: &'a BTreeSet<String>,
    pub config: &'a Config,
    pub effective: &'a Effective,
    /// False under `serve`, which never reads a submitted repo's file. The
    /// panel has to say so rather than imply an empty repo scope means an
    /// empty repo file.
    pub repo_scope_read: bool,
}

/// The whole answer to "what are my settings, and why".
#[derive(Clone, Debug, Serialize)]
pub struct SettingsReport {
    pub rows: Vec<Row>,
    /// Everything the loader refused, in the order it refused it.
    pub warnings: Vec<Warning>,
    pub user_path: Option<PathBuf>,
    pub user_exists: bool,
    /// Where the repo-scope file was looked for — the `.mezz` of the checkout
    /// the analyzed path lies in, which is not the analyzed path itself when
    /// mezz was pointed at a subfolder (CFG-012). Reporting the resolved path
    /// rather than the root it was derived from is what makes that resolution
    /// something a reader can check instead of infer.
    ///
    /// `None` under `serve`, which analyzes many repos and reads no repo's
    /// file — there is no single path to name.
    pub repo_path: Option<PathBuf>,
    pub repo_exists: bool,
    /// See [`Inputs::repo_scope_read`].
    pub repo_scope_read: bool,
}

/// Collect the keys a command named on its command line.
///
/// Callers pass `(key, was_named)` rather than the values themselves: the
/// report only has to say *that* a flag won, and the value it won with is
/// already in the `Config` or the [`Effective`] beside it.
pub fn named(pairs: &[(&str, bool)]) -> BTreeSet<String> {
    pairs
        .iter()
        .filter(|(_, set)| *set)
        .map(|(key, _)| (*key).to_string())
        .collect()
}

impl SettingsReport {
    pub fn build(root: Option<&Path>, inputs: &Inputs) -> Self {
        let user_path = super::user_path();
        let repo_path = root.map(super::repo_path);
        let mut rows = analysis_rows(inputs);
        rows.extend(view_rows(inputs));
        rows.extend(process_rows(inputs));
        Self {
            rows,
            warnings: inputs.loaded.warnings.clone(),
            user_exists: user_path.as_ref().is_some_and(|p| p.exists()),
            user_path,
            repo_exists: repo_path.as_ref().is_some_and(|p| p.exists()),
            repo_path,
            repo_scope_read: inputs.repo_scope_read,
        }
    }
}

impl Inputs<'_> {
    fn repo(&self) -> &Settings {
        &self.loaded.repo
    }

    fn user(&self) -> &Settings {
        &self.loaded.user
    }

    /// Which link won a key that overrides rather than merges.
    ///
    /// `env` is passed in rather than looked up per key because only two keys
    /// have an environment variable at all; asking the environment about
    /// `max_depth` would invent a link that does not exist.
    fn origin(&self, key: &str, env: bool, repo: bool, user: bool) -> Origin {
        if self.flags.contains(key) {
            Origin::Flag
        } else if env {
            Origin::Env
        } else if repo {
            Origin::RepoFile
        } else if user {
            Origin::UserFile
        } else {
            Origin::Default
        }
    }
}

/// A key whose value the last writer wins outright.
fn scalar(
    inputs: &Inputs,
    key: &'static str,
    tier: Tier,
    value: Value,
    present: impl Fn(&Settings) -> bool,
) -> Row {
    let origin = inputs.origin(key, false, present(inputs.repo()), present(inputs.user()));
    Row {
        key,
        tier,
        value,
        sources: vec![origin],
        note: None,
        entries: Vec::new(),
    }
}

/// A key an environment variable can also supply.
fn from_env(
    inputs: &Inputs,
    key: &'static str,
    var: &str,
    value: Value,
    present: impl Fn(&Settings) -> bool,
) -> Row {
    let env = std::env::var_os(var).is_some_and(|v| !v.is_empty());
    let origin = inputs.origin(key, env, present(inputs.repo()), present(inputs.user()));
    Row {
        key,
        tier: Tier::Process,
        value,
        sources: vec![origin],
        note: None,
        entries: Vec::new(),
    }
}

/// One of the four widening switches.
///
/// These merge with `|=` in `apply_to_config`: the file can turn a switch on
/// and never off. So "which source won" is not a question with an answer —
/// a `true` may have come from the flag, the file, or both at once — and a
/// badge naming a single winner would be lying. Every contributor is listed
/// instead.
fn widened(
    inputs: &Inputs,
    key: &'static str,
    on: bool,
    saying_yes: impl Fn(&Settings) -> bool,
) -> Row {
    let mut sources = Vec::new();
    if inputs.flags.contains(key) {
        sources.push(Origin::Flag);
    }
    if saying_yes(inputs.repo()) {
        sources.push(Origin::RepoFile);
    }
    if saying_yes(inputs.user()) {
        sources.push(Origin::UserFile);
    }
    let note = (sources.len() > 1)
        .then_some("Widened, not overridden — this switch is on if any source turns it on.");
    if sources.is_empty() {
        sources.push(Origin::Default);
    }
    Row {
        key,
        tier: Tier::Analysis,
        value: json!(on),
        sources,
        note,
        entries: Vec::new(),
    }
}

/// One of the two glob lists.
///
/// They concatenate rather than override — writing one extra ignore rule must
/// not silently re-enable scanning `node_modules` (ADR-0008) — so the list as
/// a whole has no single origin and each pattern carries its own.
fn patterns(
    inputs: &Inputs,
    key: &'static str,
    defaults: &[String],
    of: impl Fn(&Settings) -> &Vec<String>,
) -> Row {
    // The order `apply_to_config` produces: the built-ins, then whatever the
    // two files added, user before repo.
    let entries: Vec<Entry> = defaults
        .iter()
        .map(|p| (p, Origin::Default))
        .chain(of(inputs.user()).iter().map(|p| (p, Origin::UserFile)))
        .chain(of(inputs.repo()).iter().map(|p| (p, Origin::RepoFile)))
        .map(|(value, source)| Entry {
            value: value.clone(),
            source,
        })
        .collect();
    let mut sources: Vec<Origin> = Vec::new();
    for e in &entries {
        if !sources.contains(&e.source) {
            sources.push(e.source);
        }
    }
    let value = json!(entries.iter().map(|e| e.value.clone()).collect::<Vec<_>>());
    let note = (sources.len() > 1)
        .then_some("Extends rather than replaces — every source's patterns all apply.");
    Row {
        key,
        tier: Tier::View,
        value,
        sources,
        note,
        entries,
    }
}

fn analysis_rows(inputs: &Inputs) -> Vec<Row> {
    let a = &inputs.config.analysis;
    let mut languages: Vec<String> = a
        .languages
        .iter()
        .map(|l| l.filter_name().to_string())
        .collect();
    languages.sort();
    vec![
        scalar(
            inputs,
            "language",
            Tier::Analysis,
            // Empty is "no filter", which the wire spells `null` — the same
            // convention `/api/analysis/scope` already uses.
            if languages.is_empty() {
                Value::Null
            } else {
                json!(languages)
            },
            |s| s.language.is_some(),
        ),
        widened(inputs, "include_tests", a.include_tests, |s| {
            s.include_tests == Some(true)
        }),
        widened(inputs, "include_docs", a.include_docs, |s| {
            s.include_docs == Some(true)
        }),
        widened(inputs, "include_locals", a.include_locals, |s| {
            s.include_locals == Some(true)
        }),
        widened(inputs, "include_external", a.include_external, |s| {
            s.include_external == Some(true)
        }),
        scalar(
            inputs,
            "max_depth",
            Tier::Analysis,
            json!(a.max_depth),
            |s| s.max_depth.is_some(),
        ),
        scalar(
            inputs,
            "spec_dir",
            Tier::Analysis,
            json!(a.spec_dir.as_ref().map(|p| p.display().to_string())),
            |s| s.spec_dir.is_some(),
        ),
    ]
}

fn view_rows(inputs: &Inputs) -> Vec<Row> {
    let defaults = Config::default();
    let mut kinds: Vec<String> = inputs
        .config
        .filters
        .entity_kinds
        .iter()
        .map(|k| format!("{k:?}").to_lowercase())
        .collect();
    kinds.sort();
    vec![
        scalar(
            inputs,
            "min_weight",
            Tier::View,
            json!(inputs.config.filters.min_weight),
            |s| s.min_weight.is_some(),
        ),
        scalar(
            inputs,
            "kind",
            Tier::View,
            if kinds.is_empty() {
                Value::Null
            } else {
                json!(kinds)
            },
            |s| s.kind.is_some(),
        ),
        patterns(
            inputs,
            "exclude_patterns",
            &defaults.analysis.exclude_patterns,
            |s| &s.exclude_patterns,
        ),
        patterns(
            inputs,
            "include_patterns",
            &defaults.analysis.include_patterns,
            |s| &s.include_patterns,
        ),
    ]
}

fn process_rows(inputs: &Inputs) -> Vec<Row> {
    let e = inputs.effective;
    vec![
        scalar(inputs, "port", Tier::Process, json!(e.port), |s| {
            s.port.is_some()
        }),
        scalar(
            inputs,
            "debounce_ms",
            Tier::Process,
            json!(e.debounce_ms),
            |s| s.debounce_ms.is_some(),
        ),
        scalar(
            inputs,
            "output_dir",
            Tier::Process,
            path(&e.output_dir),
            |s| s.output_dir.is_some(),
        ),
        from_env(inputs, "ui_dir", "MEZZ_UI_DIR", path(&e.ui_dir), |s| {
            s.ui_dir.is_some()
        }),
        from_env(
            inputs,
            "content_fallback",
            "MEZZ_EDUCATOR_CONTENT",
            path(&e.content_fallback),
            |s| s.content_fallback.is_some(),
        ),
    ]
}

fn path(p: &Option<PathBuf>) -> Value {
    json!(p.as_ref().map(|p| p.display().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs<'a>(
        loaded: &'a Loaded,
        flags: &'a BTreeSet<String>,
        config: &'a Config,
        eff: &'a Effective,
    ) -> Inputs<'a> {
        Inputs {
            loaded,
            flags,
            config,
            effective: eff,
            repo_scope_read: true,
        }
    }

    fn row<'a>(rows: &'a [Row], key: &str) -> &'a Row {
        rows.iter().find(|r| r.key == key).expect("no such row")
    }

    fn report(loaded: Loaded, flags: &[&str], config: Config) -> SettingsReport {
        let flags = named(&flags.iter().map(|f| (*f, true)).collect::<Vec<_>>());
        let eff = Effective::default();
        let root = Path::new("/tmp/mezz-report-test");
        SettingsReport::build(Some(root), &inputs(&loaded, &flags, &config, &eff))
    }

    /// CFG-012: the report is where a reader checks *which* file was read,
    /// so it has to name the one the loader actually opened — the checkout's,
    /// not the subdirectory's.
    #[test]
    fn the_repo_file_is_named_where_the_loader_looked_for_it() {
        let dir = std::env::temp_dir().join(format!(
            "mezz-report-root-{}-{}",
            std::process::id(),
            "cfg012"
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        let sub = dir.join("src");
        std::fs::create_dir_all(&sub).unwrap();

        let loaded = Loaded::default();
        let flags = named(&[]);
        let config = Config::default();
        let eff = Effective::default();
        let r = SettingsReport::build(Some(&sub), &inputs(&loaded, &flags, &config, &eff));

        assert_eq!(r.repo_path, Some(dir.join(".mezz").join("settings.json")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The distinction the whole module exists for: the same 3 means
    /// different things depending on who chose it.
    #[test]
    fn a_value_nobody_set_reads_as_default() {
        let r = report(Loaded::default(), &[], Config::default());
        assert_eq!(row(&r.rows, "max_depth").sources, vec![Origin::Default]);
    }

    #[test]
    fn the_repo_file_outranks_the_user_file() {
        let loaded = Loaded {
            repo: Settings {
                max_depth: Some(5),
                ..Default::default()
            },
            user: Settings {
                max_depth: Some(9),
                ..Default::default()
            },
            warnings: Vec::new(),
        };
        let r = report(loaded, &[], Config::default());
        assert_eq!(row(&r.rows, "max_depth").sources, vec![Origin::RepoFile]);
    }

    #[test]
    fn a_flag_outranks_both_files() {
        let loaded = Loaded {
            repo: Settings {
                max_depth: Some(5),
                ..Default::default()
            },
            ..Default::default()
        };
        let r = report(loaded, &["max_depth"], Config::default());
        assert_eq!(row(&r.rows, "max_depth").sources, vec![Origin::Flag]);
    }

    /// A widening switch that two sources turned on names both, because
    /// naming one would imply the other could have turned it off.
    #[test]
    fn a_widened_switch_names_every_contributor() {
        let loaded = Loaded {
            repo: Settings {
                include_docs: Some(true),
                ..Default::default()
            },
            user: Settings {
                include_docs: Some(true),
                ..Default::default()
            },
            warnings: Vec::new(),
        };
        let mut config = Config::default();
        config.analysis.include_docs = true;
        let r = report(loaded, &[], config);
        let docs = row(&r.rows, "include_docs");
        assert_eq!(docs.sources, vec![Origin::RepoFile, Origin::UserFile]);
        assert!(
            docs.note.is_some(),
            "a two-source merge needs its explanation"
        );
    }

    #[test]
    fn a_switch_nobody_turned_on_is_a_plain_default() {
        let r = report(Loaded::default(), &[], Config::default());
        let docs = row(&r.rows, "include_docs");
        assert_eq!(docs.sources, vec![Origin::Default]);
        assert!(docs.note.is_none());
    }

    /// Patterns concatenate, so the list has no single origin and each entry
    /// carries its own.
    #[test]
    fn each_pattern_carries_its_own_origin() {
        let loaded = Loaded {
            repo: Settings {
                exclude_patterns: vec!["**/from-repo/**".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        let r = report(loaded, &[], Config::default());
        let ex = row(&r.rows, "exclude_patterns");
        let mine = ex
            .entries
            .iter()
            .find(|e| e.value == "**/from-repo/**")
            .unwrap();
        assert_eq!(mine.source, Origin::RepoFile);
        assert!(
            ex.entries.iter().any(|e| e.source == Origin::Default),
            "the built-in ignores must still be listed — they still apply"
        );
    }

    /// Every key in the settings file has to appear, or the panel silently
    /// omits a setting that is shaping the reader's graph.
    #[test]
    fn every_settable_key_is_reported() {
        let r = report(Loaded::default(), &[], Config::default());
        for key in [
            "language",
            "kind",
            "include_tests",
            "include_docs",
            "include_locals",
            "include_external",
            "max_depth",
            "min_weight",
            "port",
            "debounce_ms",
            "exclude_patterns",
            "include_patterns",
            "output_dir",
            "spec_dir",
            "ui_dir",
            "content_fallback",
        ] {
            assert!(
                r.rows.iter().any(|row| row.key == key),
                "{key} is not reported"
            );
        }
    }

    /// The tiers are what the UI gates editing on, so a key moving between
    /// them is a behaviour change and should break a test.
    #[test]
    fn tiers_match_what_each_key_controls() {
        let r = report(Loaded::default(), &[], Config::default());
        assert_eq!(row(&r.rows, "max_depth").tier, Tier::Analysis);
        assert_eq!(row(&r.rows, "min_weight").tier, Tier::View);
        assert_eq!(row(&r.rows, "port").tier, Tier::Process);
    }
}
