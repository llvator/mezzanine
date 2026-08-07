//! The settings file: defaults a user or a repo writes down once (CFG-002..004).
//!
//! Two scopes, resolved in this order, each losing to the one after it:
//!
//! ```text
//! CLI flag  >  env var  >  <repo>/.nao/settings.json  >  ~/.config/nao/settings.json  >  defaults
//! ```
//!
//! The split is not cosmetic. `ui_dir` is a property of the *installation* —
//! putting it in a repo file means writing the same absolute path into every
//! repo you ever analyze. `language` is a property of the *repo* and belongs
//! in version control beside the code it describes. Each key is therefore
//! valid in one scope, the other, or both, and saying so is most of what this
//! module does.
//!
//! # The keys that are not here
//!
//! `allow_agent_spawn`, `no_token`, `allow_origin` and `allow_unsafe_passes`
//! are deliberately absent from [`Settings`]. A repo-scope file is content
//! cloned from a stranger, and a stranger who can turn on agent spawning has
//! executed code on the reader's machine. They are absent rather than parsed
//! and filtered so that no future edit can reintroduce them by forgetting a
//! check — see [`report_rejected`], which names them when it sees them.
//!
//! `nao serve` goes further and never reads a repo-scope file at all: there
//! the whole tree arrived from a URL a stranger pasted. That mirrors the
//! reasoning already in [`crate::config::AnalysisConfig::allow_unsafe_passes`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub mod report;
pub use report::{Origin, Row, SettingsReport, Tier};

use crate::config::Config;
use crate::models::EntityKind;
use crate::models::file_info::Language;

/// Explicit override of the user-scope directory, for tests and for wrapper
/// scripts with nowhere to put a flag. Exists for the same reason
/// `NAO_CACHE_DIR` does: a test must be able to run without reading — or
/// writing — the developer's real home directory.
const CONFIG_DIR_ENV: &str = "NAO_CONFIG_DIR";

/// The repo-scope directory, resolved against the *analyzed root* rather than
/// the process working directory. `nao watch /elsewhere/repo` reads
/// `/elsewhere/repo/.nao/settings.json`, or the file would be useless the
/// moment you analyze anything but the directory you are standing in.
const REPO_DIR: &str = ".nao";

const FILE_NAME: &str = "settings.json";

/// The port `watch` and `serve` fall back to when neither a flag nor a
/// settings file names one. Public because anything that *generates* a way
/// to reach the server — `nao init`'s VS Code tasks — has to agree with what
/// the server will actually bind, and a second literal `3000` is how those
/// two drift apart.
pub const DEFAULT_PORT: u16 = 3000;

/// Keys a settings file may never set, at either scope, with the reason to
/// print when one turns up. Each grants something a file should not be able
/// to grant: code execution, or read access to the reader's source.
const REJECTED: &[(&str, &str)] = &[
    ("allow_agent_spawn", "it opens a terminal on this machine"),
    ("no_token", "it drops the pairing-token requirement"),
    ("allow_origin", "it lets another browser origin read your source"),
    (
        "allow_unsafe_passes",
        "it permits passes that execute build scripts from the analyzed tree",
    ),
];

/// Which file a value came from. Determines which keys are honoured.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    /// `~/.config/nao/settings.json` — properties of this installation.
    User,
    /// `<analyzed-root>/.nao/settings.json` — properties of the repo.
    Repo,
}

/// How much a reader should care about a diagnostic.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Severity {
    /// A key that would have granted a capability. The repo asked for
    /// something a cloned file may never have.
    Rejected,
    /// A key nao could not use: a typo, the wrong scope, a path that
    /// escapes the repo, a language nobody parses.
    Ignored,
    /// The file itself could not be read or parsed. Nothing in it applied.
    Malformed,
}

/// Something the loader could not honour, kept rather than only printed.
///
/// Every one of these used to be an `eprintln!` and nothing else, which is
/// fine for `nao analyze` in a terminal and useless for the browser UI: a
/// reader who typos `exclude_pattern` saw their setting do nothing, with the
/// explanation on a stream they were not watching. Collecting them costs a
/// `Vec` per load and lets both surfaces say the same thing.
#[derive(Clone, Debug, Serialize)]
pub struct Warning {
    /// The file it came from, so a reader with two scopes knows which to fix.
    pub file: PathBuf,
    /// The offending key, when the diagnostic is about one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub severity: Severity,
    /// Prose, already addressed to the person who wrote the file.
    pub message: String,
}

impl Warning {
    fn new(file: &Path, key: Option<&str>, severity: Severity, message: String) -> Self {
        Self {
            file: file.to_path_buf(),
            key: key.map(str::to_string),
            severity,
            message,
        }
    }

    /// The stderr line this used to be, unchanged. A terminal user keeps
    /// exactly the output they had before any of this was collected.
    fn print(&self) {
        eprintln!("   ⚠ {}: {}", self.file.display(), self.message);
    }
}

/// Both scopes with the seam still visible, plus everything the loader
/// refused along the way.
///
/// [`load`] merges and forgets. Anything that wants to *report* on the
/// settings — which file a value came from, which key was thrown away —
/// needs the two sides apart, so the merge happens at the end rather than
/// inside the reader.
#[derive(Clone, Debug, Default)]
pub struct Loaded {
    /// `<root>/.nao/settings.json`, already stripped of keys its scope may
    /// not set. Always default under `serve`, which never reads it.
    pub repo: Settings,
    /// `~/.config/nao/settings.json`, same treatment.
    pub user: Settings,
    pub warnings: Vec<Warning>,
}

impl Loaded {
    /// The merged view the rest of nao consumes: repo over user.
    pub fn merged(&self) -> Settings {
        self.repo.clone().over(self.user.clone())
    }
}

/// The scalars a command named on its command line.
///
/// Everything else in the precedence chain can be read off the `Config` a
/// flag has already written: an unset `spec_dir` is `None`, an unset language
/// filter is empty. These two have no such spelling, so a command that offers
/// a flag for one has to say so — see [`Settings::apply_scalars`] for what
/// goes wrong when it cannot.
///
/// `min_weight` has no flag today. It is here so that adding one is a change
/// to `main.rs` alone, rather than a change that has to rediscover this rule.
#[derive(Copy, Clone, Debug, Default)]
pub struct Flags {
    pub max_depth: Option<usize>,
    pub min_weight: Option<u32>,
}

/// The settled contents of a settings file, or of both merged.
///
/// Every scalar is an `Option` so that "absent" and "set to the default
/// value" stay distinguishable — without that, a file could never be
/// overridden by a flag, because the flag has no way to know whether the
/// value it sees was chosen or defaulted.
///
/// The field names are the CLI flag names rather than `Config`'s internal
/// layout (ADR-0008). That keeps the file's surface deliberately smaller than
/// `Config`, which is what makes the rejected keys above *absent* rather than
/// merely unused.
/// `Serialize` is here so the browser UI can be shown the file, and so
/// CFG-010 can write one back through the same struct it reads. Writing
/// through `Settings` rather than a hand-built JSON object is what keeps the
/// capability-granting keys unwritable: they are not fields, so no serializer
/// can emit them.
///
/// `skip_serializing_if` on every `Option` keeps a written file free of
/// `"max_depth": null` — noise in a diff, and it would destroy the
/// absent-versus-defaulted distinction the whole module rests on.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    // ---- user scope only ----
    /// Where the built browser UI lives. Installation property: see the
    /// module docs for why this is not repo-scoped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui_dir: Option<PathBuf>,
    /// Fallback Educator content root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_fallback: Option<PathBuf>,

    // ---- repo scope only ----
    /// Where `watch` writes the JSON it serves. Repo-relative by default
    /// (`ui/public`), so a machine-wide value would point every analyzed repo
    /// at one directory — a footgun rather than a feature.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_dir: Option<PathBuf>,

    /// Which directory holds this repo's Elevator (`.elv`) spec. Absent —
    /// the usual case — means every `.elv` under the root is the spec. See
    /// [`crate::config::AnalysisConfig::spec_dir`] for what setting it does.
    ///
    /// Repo-scope only, and **relative paths only**, checked by
    /// [`Self::clear_escaping_spec_dir`]. A repo file is content cloned from
    /// a stranger, and this key is a path nao will read from: an absolute
    /// `/home/you/.ssh` or a `../../..` would let the clone choose where.
    /// Naming a directory outside the tree is a real layout — a spec in a
    /// sibling docs repo — but it takes an operator saying so, with
    /// `--spec-dir` or from the browser UI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec_dir: Option<PathBuf>,

    // ---- both scopes ----
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<Vec<EntityKind>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_tests: Option<bool>,
    /// Analyze Markdown alongside the code. The durable way to turn the doc
    /// layer on, which matters more here than for most flags: the VS Code
    /// extension and `nao watch` are launched without anyone typing a flag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_docs: Option<bool>,
    /// Keep local and module-level assignments as entities (CFG-005). Off by
    /// default, and a repo that wants the detail — a focused reading, dataflow
    /// work — says so here rather than in a flag nobody types: `nao watch` and
    /// the VS Code extension are launched without one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_locals: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_external: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_depth: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_weight: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debounce_ms: Option<u64>,
    /// Extra ignore globs. **Extends** the built-in defaults rather than
    /// replacing them (ADR-0008): writing one extra rule should not silently
    /// re-enable scanning `node_modules`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub exclude_patterns: Vec<String>,
    /// Extra include globs. Extends, for symmetry with `exclude_patterns`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub include_patterns: Vec<String>,

    /// Everything serde did not recognise. Captured rather than dropped so a
    /// key can be named back to whoever wrote it — a silently ignored setting
    /// reads as "nao is broken" to an author who meant well, and tells a
    /// reader being attacked nothing at all.
    // Never serialized: writing these back out would re-emit the very keys
    // `report_rejected` just refused.
    #[serde(flatten, skip_serializing)]
    unrecognized: BTreeMap<String, serde_json::Value>,
}

/// The user-scope directory: `NAO_CONFIG_DIR`, else `$XDG_CONFIG_HOME/nao`,
/// else `$HOME/.config/nao`.
///
/// Deliberately the same shape as
/// [`crate::analyzer::parse_store`]'s cache root, three strings changed. The
/// alternative, `~/.nao`, would scatter nao's state across two conventions
/// and reopen the question every time something new needs a home. `None`
/// when nothing resolves — some CI and container environments have no `HOME`,
/// and the answer there is "no settings file", not a synthesized one.
fn config_root() -> Option<PathBuf> {
    config_root_from(
        std::env::var_os(CONFIG_DIR_ENV),
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
}

/// The branch order alone, with the environment passed in.
///
/// Split out because the alternative is a test that mutates process-wide
/// environment variables — racy under a threaded test runner, and `unsafe` as
/// of the 2024 edition. Empty is treated as unset throughout: an exported but
/// empty variable is a shell accident, not a choice.
fn config_root_from(
    explicit: Option<std::ffi::OsString>,
    xdg: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    if let Some(dir) = explicit.filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    if let Some(dir) = xdg.filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir).join("nao"));
    }
    home.filter(|h| !h.is_empty())
        .map(|home| PathBuf::from(home).join(".config").join("nao"))
}

/// The user-scope settings file path, whether or not it exists.
pub fn user_path() -> Option<PathBuf> {
    config_root().map(|dir| dir.join(FILE_NAME))
}

/// The repo-scope settings file path for an analyzed root.
pub fn repo_path(root: &Path) -> PathBuf {
    repo_dir(root).join(FILE_NAME)
}

/// The repo-scope directory for an analyzed root, whether or not it exists.
///
/// Exported because settings are no longer the only thing that lives there —
/// saved views (`views.json`) are repo-scope for the same reason, and the
/// spelling of the directory belongs in one place.
pub fn repo_dir(root: &Path) -> PathBuf {
    root.join(REPO_DIR)
}

/// The repo-scope file as raw JSON, for a writer that has to preserve keys it
/// does not understand.
///
/// Deliberately *not* forgiving where [`read`] is. `read` treats a malformed
/// file as "no settings" because a stray comma must never fail `nao analyze`.
/// A writer cannot afford that reading: answering "you had nothing" for a
/// file we failed to parse is exactly how the next save destroys it. Absent
/// is still the normal case and still says nothing.
pub fn read_raw(path: &Path) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Default::default()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    match serde_json::from_str(&text) {
        Ok(serde_json::Value::Object(map)) => Ok(map),
        Ok(_) => Err(format!("{}: the file is not a JSON object", path.display())),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Write a settings file whole, atomically.
///
/// Rename over a sibling temp file rather than truncate-and-write, so an
/// interrupted save leaves the previous settings intact instead of half a
/// JSON document. Pretty-printed because this file is meant to be read and
/// reviewed in a diff — the same bargain `views.json` makes next door.
pub fn write(
    path: &Path,
    body: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let text = serde_json::to_string_pretty(body).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, format!("{text}\n")).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Read and validate one settings file. Absent is the normal case and says
/// nothing; unreadable or malformed warns and yields defaults.
///
/// A bad settings file is never fatal. `nao analyze` does not need the UI
/// directory and must not die because of a stray comma in a file it barely
/// consults.
fn read(path: &Path, scope: Scope) -> (Settings, Vec<Warning>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return (Settings::default(), Vec::new());
    };
    match serde_json::from_str::<Settings>(&text) {
        Ok(mut settings) => {
            let warnings = settings.validate(scope, path);
            (settings, warnings)
        }
        Err(e) => {
            // The most important warning of the set, and the one most easily
            // missed: nothing in the file applied, so every value the reader
            // believes they set is actually a default.
            let w = Warning::new(
                path,
                None,
                Severity::Malformed,
                format!("{e} — ignoring the whole file."),
            );
            (Settings::default(), vec![w])
        }
    }
}

/// Read one scope and print whatever it refused, for callers that only want
/// the values. The printing is what every caller had before warnings were
/// collected at all.
fn read_and_print(path: &Path, scope: Scope) -> Settings {
    let (settings, warnings) = read(path, scope);
    warnings.iter().for_each(Warning::print);
    settings
}

/// The user-scope settings alone. This is what `nao serve` gets: the
/// operator's own preferences apply, the submitted repo's do not.
pub fn user() -> Settings {
    user_path().map_or_else(Settings::default, |p| read_and_print(&p, Scope::User))
}

/// The user scope with the seam kept, for `serve`'s settings report. `repo`
/// stays default because `serve` genuinely never looked at one.
pub fn user_scoped() -> Loaded {
    let Some(path) = user_path() else {
        return Loaded::default();
    };
    let (user, warnings) = read(&path, Scope::User);
    Loaded { repo: Settings::default(), user, warnings }
}

/// Both scopes, repo winning over user, for a trusted local root.
pub fn load(root: &Path) -> Settings {
    load_scoped(root).merged()
}

/// Both scopes kept apart, warnings retained, everything printed once.
///
/// The reading half of every caller in nao goes through here, so the stderr
/// output is emitted in exactly one place and cannot drift from what the
/// browser is shown.
pub fn load_scoped(root: &Path) -> Loaded {
    let (repo, mut warnings) = read(&repo_path(root), Scope::Repo);
    let (user, user_warnings) = user_path()
        .map(|p| read(&p, Scope::User))
        .unwrap_or_default();
    warnings.extend(user_warnings);
    warnings.iter().for_each(Warning::print);
    Loaded { repo, user, warnings }
}

impl Settings {
    /// Name every key that will not be honoured, then clear it.
    fn validate(&mut self, scope: Scope, from: &Path) -> Vec<Warning> {
        let mut warnings = self.report_rejected(from);
        if scope == Scope::Repo {
            warnings.extend(self.clear_user_only(from));
            warnings.extend(self.clear_escaping_spec_dir(from));
        } else {
            warnings.extend(self.clear_repo_only(from));
        }
        warnings
    }

    /// Drop a repo-scope `spec_dir` that names anywhere but inside the repo.
    ///
    /// The whole of this module's threat model in one rule: the file was
    /// cloned, so it may not choose which directories nao reads. Syntactic
    /// on purpose — absolute paths and `..` components, decided without
    /// touching the filesystem — because a check that canonicalizes has to
    /// answer what a not-yet-existing path means, and every answer to that
    /// is a hole.
    fn clear_escaping_spec_dir(&mut self, from: &Path) -> Vec<Warning> {
        let escapes = self.spec_dir.as_ref().is_some_and(|dir| {
            dir.is_absolute() || dir.components().any(|c| c == std::path::Component::ParentDir)
        });
        if !escapes {
            return Vec::new();
        }
        self.spec_dir = None;
        vec![Warning::new(
            from,
            Some("spec_dir"),
            Severity::Ignored,
            "`spec_dir` must stay inside the repo — a cloned file does not get \
             to pick which directories nao reads. Pass `--spec-dir` to name one \
             elsewhere."
                .to_string(),
        )]
    }

    /// Report unrecognized keys, calling out the privileged ones by the thing
    /// they would have granted.
    fn report_rejected(&self, from: &Path) -> Vec<Warning> {
        self.unrecognized
            .keys()
            .map(|key| match REJECTED.iter().find(|(name, _)| name == key) {
                Some((_, why)) => Warning::new(
                    from,
                    Some(key),
                    Severity::Rejected,
                    format!(
                        "`{key}` is never settable from a settings file — {why}. \
                         Pass it as a flag instead."
                    ),
                ),
                None => Warning::new(
                    from,
                    Some(key),
                    Severity::Ignored,
                    format!("unknown setting `{key}` — ignoring it."),
                ),
            })
            .collect()
    }

    fn clear_user_only(&mut self, from: &Path) -> Vec<Warning> {
        let warnings = misplaced(
            from,
            &[
                ("ui_dir", self.ui_dir.is_some()),
                ("content_fallback", self.content_fallback.is_some()),
            ],
            "describes this installation, not this repo — move it to the user \
             settings file.",
        );
        self.ui_dir = None;
        self.content_fallback = None;
        warnings
    }

    fn clear_repo_only(&mut self, from: &Path) -> Vec<Warning> {
        let warnings = self.report_repo_only(from);
        self.output_dir = None;
        self.spec_dir = None;
        warnings
    }

    /// Name the repo-scope keys found in a user-scope file. Split from the
    /// clearing so that adding a key grows the reporting loop's data rather
    /// than the caller's branch count — the complexity gate fails a PR that
    /// makes an existing function busier, however slightly.
    fn report_repo_only(&self, from: &Path) -> Vec<Warning> {
        misplaced(
            from,
            &[
                ("output_dir", self.output_dir.is_some()),
                ("spec_dir", self.spec_dir.is_some()),
            ],
            "is repo-relative — set it in a repo's .nao/settings.json, not \
             machine-wide.",
        )
    }

    /// The analysis-shaping keys of a live config, shaped as a settings file
    /// (CFG-010).
    ///
    /// The inverse of [`Self::apply_to_config`], and deliberately narrower:
    /// only the keys that decide *what gets parsed*. `min_weight` and `kind`
    /// are filters the panel and saved views already own, and writing them
    /// from here would give one value two controls.
    ///
    /// Lives beside the struct rather than in the handler because
    /// `unrecognized` is private — which is the point. A writer that cannot
    /// name that field cannot resurrect a key `report_rejected` refused.
    pub fn analysis_scope_of(config: &Config) -> Self {
        let a = &config.analysis;
        let mut languages: Vec<String> =
            a.languages.iter().map(|l| l.filter_name().to_string()).collect();
        languages.sort();
        Self {
            // Empty means "no filter", which the file spells by omitting the
            // key rather than by storing an empty list.
            language: (!languages.is_empty()).then_some(languages),
            include_tests: Some(a.include_tests),
            include_docs: Some(a.include_docs),
            include_locals: Some(a.include_locals),
            include_external: Some(a.include_external),
            max_depth: Some(a.max_depth),
            spec_dir: a.spec_dir.clone(),
            ..Default::default()
        }
    }

    /// Every configured language name nao has no parser for.
    ///
    /// `apply_filters` says this on stderr as it drops them, but that runs
    /// once per command and long after the file was read. Recomputing it is
    /// two string comparisons and lets the report name the key without
    /// depending on when a config was last built.
    pub fn unknown_languages(&self) -> Vec<String> {
        self.language
            .iter()
            .flatten()
            .filter(|name| Language::from_name(name).is_none())
            .cloned()
            .collect()
    }

    /// `self` layered over `lower`: every scalar `self` sets wins, and the
    /// pattern lists concatenate so a user's machine-wide ignores survive a
    /// repo adding its own.
    fn over(self, lower: Settings) -> Settings {
        Settings {
            ui_dir: self.ui_dir.or(lower.ui_dir),
            content_fallback: self.content_fallback.or(lower.content_fallback),
            output_dir: self.output_dir.or(lower.output_dir),
            spec_dir: self.spec_dir.or(lower.spec_dir),
            language: self.language.or(lower.language),
            kind: self.kind.or(lower.kind),
            include_tests: self.include_tests.or(lower.include_tests),
            include_docs: self.include_docs.or(lower.include_docs),
            include_locals: self.include_locals.or(lower.include_locals),
            include_external: self.include_external.or(lower.include_external),
            max_depth: self.max_depth.or(lower.max_depth),
            min_weight: self.min_weight.or(lower.min_weight),
            port: self.port.or(lower.port),
            debounce_ms: self.debounce_ms.or(lower.debounce_ms),
            exclude_patterns: concat(lower.exclude_patterns, self.exclude_patterns),
            include_patterns: concat(lower.include_patterns, self.include_patterns),
            unrecognized: BTreeMap::new(),
        }
    }

    /// Fold the analysis-shaped settings into a `Config` that already carries
    /// its defaults and any explicit CLI flags.
    ///
    /// Only touches fields the caller left at their default, because by the
    /// time this runs the flags have already been applied and a flag outranks
    /// a file. The pattern lists are the exception: they extend.
    ///
    /// Use [`Self::apply_with`] from a command that has a flag for one of the
    /// scalars in [`Flags`]; those cannot be inferred from the config alone.
    pub fn apply_to_config(&self, config: &mut Config) {
        self.apply_with(config, Flags::default());
    }

    /// [`Self::apply_to_config`], told which scalars the CLI named outright.
    pub fn apply_with(&self, config: &mut Config, flags: Flags) {
        if let Some(v) = self.include_tests {
            config.analysis.include_tests |= v;
        }
        if let Some(v) = self.include_docs {
            config.analysis.include_docs |= v;
        }
        if let Some(v) = self.include_locals {
            config.analysis.include_locals |= v;
        }
        if let Some(v) = self.include_external {
            config.analysis.include_external |= v;
        }
        self.apply_scalars(config, flags);
        self.apply_spec_dir(config);
        config
            .analysis
            .exclude_patterns
            .extend(self.exclude_patterns.iter().cloned());
        config
            .analysis
            .include_patterns
            .extend(self.include_patterns.iter().cloned());
        self.apply_filters(config);
    }

    /// The scalars with no "unset" value of their own.
    ///
    /// `spec_dir` and `languages` can be guarded on the config alone —
    /// `None` and "empty" mean nobody chose. `max_depth` cannot: a `3` the
    /// caller typed and a `3` [`Config`]'s default chose are the same
    /// `usize`, so applying the file unconditionally here silently outranked
    /// a `--depth` the user had just typed. The flag is therefore passed in
    /// rather than inferred, and the chain resolves in one expression —
    /// the same shape `port` has always used in `main.rs`.
    fn apply_scalars(&self, config: &mut Config, flags: Flags) {
        if let Some(v) = flags.max_depth.or(self.max_depth) {
            config.analysis.max_depth = v;
        }
        if let Some(v) = flags.min_weight.or(self.min_weight) {
            config.filters.min_weight = v;
        }
    }

    /// The spec directory, which only applies when the CLI named none —
    /// `--spec-dir` has already been written into the config by the time
    /// this runs, and a flag outranks a file.
    fn apply_spec_dir(&self, config: &mut Config) {
        if config.analysis.spec_dir.is_none() {
            config.analysis.spec_dir = self.spec_dir.clone();
        }
    }

    /// The two selection lists, which only apply when the CLI named none.
    fn apply_filters(&self, config: &mut Config) {
        if config.analysis.languages.is_empty() {
            for name in self.language.iter().flatten() {
                match Language::from_name(name) {
                    Some(lang) => {
                        config.analysis.languages.insert(lang);
                    }
                    None => eprintln!("   ⚠ settings: unknown language `{name}` — ignoring it."),
                }
            }
        }
        if config.filters.entity_kinds.is_empty() {
            for kind in self.kind.iter().flatten() {
                config.filters.entity_kinds.insert(*kind);
            }
        }
    }
}

fn concat(mut lower: Vec<String>, upper: Vec<String>) -> Vec<String> {
    lower.extend(upper);
    lower
}

/// One warning per key that turned up in the wrong scope, sharing a reason.
///
/// The two callers differ only in their key list and their sentence, and both
/// were the same loop before. Keeping it one function means adding a key
/// stays a change to data.
fn misplaced(from: &Path, keys: &[(&str, bool)], why: &str) -> Vec<Warning> {
    keys.iter()
        .filter(|(_, present)| *present)
        .map(|(key, _)| {
            Warning::new(from, Some(key), Severity::Ignored, format!("`{key}` {why}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The values half of `read`, for the tests that only assert on those.
    fn values(path: &Path, scope: Scope) -> Settings {
        read(path, scope).0
    }

    /// A per-test config dir, isolated from the developer's real `~/.config`
    /// and from sibling tests — the same reason `parse_store`'s tests build
    /// their own cache root.
    struct TempConfig(PathBuf);

    impl TempConfig {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("nao-settings-test-{}-{}", std::process::id(), tag));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn write(&self, body: &str) -> PathBuf {
            let path = self.0.join(FILE_NAME);
            std::fs::write(&path, body).unwrap();
            path
        }
    }

    impl Drop for TempConfig {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn reads_a_user_scope_file() {
        let dir = TempConfig::new("user");
        let path = dir.write(r#"{"ui_dir":"/tmp/somewhere","include_tests":true}"#);
        let settings = values(&path, Scope::User);
        assert_eq!(settings.ui_dir, Some(PathBuf::from("/tmp/somewhere")));
        assert_eq!(settings.include_tests, Some(true));
    }

    /// The regression that matters most in this set: a cloned repo must not
    /// be able to turn on agent spawning, and the same holds for the other
    /// three privileged keys.
    #[test]
    fn privileged_keys_are_never_honored() {
        let dir = TempConfig::new("privileged");
        let path = dir.write(
            r#"{"allow_agent_spawn":true,"no_token":true,
                "allow_origin":["https://evil.example"],"allow_unsafe_passes":true}"#,
        );
        let settings = values(&path, Scope::Repo);
        // They are not fields, so the only place they can land is the
        // unrecognized bag — which nothing reads.
        assert_eq!(settings.unrecognized.len(), 4);
        for (key, _) in REJECTED {
            assert!(settings.unrecognized.contains_key(*key), "{key} vanished silently");
        }
    }

    /// The one-shot rendering flags describe a single invocation, not a
    /// standing preference. A file that could set `format` would make
    /// `nao analyze -f json | jq` fail depending on a file the reader forgot
    /// about, so they are not fields either.
    #[test]
    fn rendering_flags_are_not_settable_from_a_file() {
        let dir = TempConfig::new("rendering");
        let path = dir.write(r#"{"format":"json","layout":"lr","focus":"main"}"#);
        let settings = values(&path, Scope::Repo);
        assert_eq!(settings.unrecognized.len(), 3);
    }

    #[test]
    fn repo_scope_drops_installation_keys() {
        let dir = TempConfig::new("repo-uidir");
        let path = dir.write(r#"{"ui_dir":"/tmp/x","content_fallback":"/tmp/y","language":["rust"]}"#);
        let settings = values(&path, Scope::Repo);
        assert_eq!(settings.ui_dir, None);
        assert_eq!(settings.content_fallback, None);
        assert_eq!(settings.language, Some(vec!["rust".to_string()]));
    }

    #[test]
    fn user_scope_drops_repo_relative_keys() {
        let dir = TempConfig::new("user-outdir");
        let path = dir.write(r#"{"output_dir":"ui/public","spec_dir":"spec"}"#);
        let settings = values(&path, Scope::User);
        assert_eq!(settings.output_dir, None);
        assert_eq!(settings.spec_dir, None);
    }

    /// A repo that keeps its spec somewhere other than the root is the whole
    /// point of the key, so the ordinary relative path must survive.
    #[test]
    fn a_repo_scope_spec_dir_inside_the_repo_is_honored() {
        let dir = TempConfig::new("spec-inside");
        let path = dir.write(r#"{"spec_dir":"docs/domain"}"#);
        assert_eq!(values(&path, Scope::Repo).spec_dir, Some(PathBuf::from("docs/domain")));
    }

    /// The regression that matters for this key: `spec_dir` is a path nao
    /// reads from, and a cloned settings file must not be able to aim it
    /// outside the tree it came with.
    #[test]
    fn a_repo_scope_spec_dir_may_not_escape_the_repo() {
        for body in [r#"{"spec_dir":"/etc"}"#, r#"{"spec_dir":"../../elsewhere"}"#] {
            let dir = TempConfig::new("spec-escape");
            let path = dir.write(body);
            assert_eq!(values(&path, Scope::Repo).spec_dir, None, "{body} was honored");
        }
    }

    /// `--spec-dir` beat the file before this ran; it must still win after.
    #[test]
    fn a_spec_dir_flag_outranks_the_file() {
        let mut config = Config::for_path(".");
        config.analysis.spec_dir = Some(PathBuf::from("from/the/flag"));
        let settings = Settings {
            spec_dir: Some(PathBuf::from("from/the/file")),
            ..Default::default()
        };
        settings.apply_to_config(&mut config);
        assert_eq!(config.analysis.spec_dir, Some(PathBuf::from("from/the/flag")));
    }

    #[test]
    fn a_spec_dir_from_the_file_applies_when_no_flag_was_passed() {
        let mut config = Config::for_path(".");
        let settings = Settings {
            spec_dir: Some(PathBuf::from("docs/domain")),
            ..Default::default()
        };
        settings.apply_to_config(&mut config);
        assert_eq!(config.analysis.spec_dir, Some(PathBuf::from("docs/domain")));
    }

    /// The regression CFG-009 exists for: a privileged key was named on
    /// stderr and nowhere else, so a browser reader handed a hostile repo
    /// saw nothing at all.
    #[test]
    fn a_privileged_key_is_kept_not_just_printed() {
        let dir = TempConfig::new("warn-privileged");
        let path = dir.write(r#"{"allow_agent_spawn":true}"#);
        let (_, warnings) = read(&path, Scope::Repo);
        let w = warnings.first().expect("the key was refused silently");
        assert_eq!(w.severity, Severity::Rejected);
        assert_eq!(w.key.as_deref(), Some("allow_agent_spawn"));
        assert!(w.message.contains("opens a terminal"), "the reason has to survive");
    }

    /// The everyday case: a typo reads as "nao is broken" unless the key is
    /// named back to whoever wrote it.
    #[test]
    fn an_unknown_key_is_kept_with_its_file() {
        let dir = TempConfig::new("warn-typo");
        let path = dir.write(r#"{"exclude_pattern":[]}"#);
        let (_, warnings) = read(&path, Scope::Repo);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].severity, Severity::Ignored);
        assert_eq!(warnings[0].file, path);
    }

    /// A file that failed to parse is the warning most easily missed, because
    /// every value the reader believes they set is quietly a default.
    #[test]
    fn a_malformed_file_warns_rather_than_vanishing() {
        let dir = TempConfig::new("warn-malformed");
        let path = dir.write("{ not json");
        let (_, warnings) = read(&path, Scope::User);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].severity, Severity::Malformed);
    }

    #[test]
    fn a_misplaced_key_says_which_scope_it_belongs_in() {
        let dir = TempConfig::new("warn-scope");
        let path = dir.write(r#"{"ui_dir":"/tmp/x"}"#);
        let (_, warnings) = read(&path, Scope::Repo);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("user settings file"));
    }

    /// An absent file is the normal case and says nothing at all.
    #[test]
    fn a_file_that_is_not_there_produces_no_warnings() {
        let (_, warnings) = read(Path::new("/nonexistent/nao/settings.json"), Scope::User);
        assert!(warnings.is_empty());
    }

    /// The distinction a writer depends on: absent is fine, unparseable is
    /// not. `read` deliberately conflates them so a stray comma cannot fail
    /// `nao analyze`; `read_raw` must not, or the next save overwrites a file
    /// we never understood.
    #[test]
    fn read_raw_separates_an_absent_file_from_an_unreadable_one() {
        assert!(read_raw(Path::new("/nonexistent/nao/settings.json")).unwrap().is_empty());
        let dir = TempConfig::new("raw-malformed");
        assert!(read_raw(&dir.write("{ not json")).is_err());
    }

    #[test]
    fn a_written_file_reads_back_and_omits_what_nobody_set() {
        let dir = TempConfig::new("write-roundtrip");
        let path = dir.0.join(FILE_NAME);
        let settings = Settings { max_depth: Some(7), ..Default::default() };
        let serde_json::Value::Object(body) = serde_json::to_value(&settings).unwrap() else {
            unreachable!()
        };
        write(&path, &body).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("null"), "an unset key was written as null: {text}");
        assert!(text.ends_with('\n'), "a settings file should end with a newline");
        assert_eq!(values(&path, Scope::Repo).max_depth, Some(7));
    }

    /// Writing must never be able to re-emit a key the reader just refused.
    #[test]
    fn a_rejected_key_cannot_survive_a_read_and_write() {
        let dir = TempConfig::new("write-privileged");
        let path = dir.write(r#"{"allow_agent_spawn":true,"max_depth":4}"#);
        let settings = values(&path, Scope::Repo);
        let text = serde_json::to_string(&settings).unwrap();
        assert!(!text.contains("allow_agent_spawn"), "serializing resurrected it: {text}");
    }

    #[test]
    fn a_language_nao_cannot_parse_is_named() {
        let settings = Settings {
            language: Some(vec!["rust".into(), "cobol".into()]),
            ..Default::default()
        };
        assert_eq!(settings.unknown_languages(), vec!["cobol".to_string()]);
    }

    #[test]
    fn malformed_json_yields_defaults_rather_than_failing() {
        let dir = TempConfig::new("malformed");
        let path = dir.write("{ not json");
        let settings = values(&path, Scope::User);
        assert!(settings.ui_dir.is_none());
        assert!(settings.include_tests.is_none());
    }

    #[test]
    fn absent_file_is_not_an_error() {
        let settings = values(Path::new("/nonexistent/nao/settings.json"), Scope::User);
        assert!(settings.ui_dir.is_none());
    }

    #[test]
    fn repo_wins_over_user_and_patterns_concatenate() {
        let upper = Settings {
            port: Some(3300),
            exclude_patterns: vec!["**/repo/**".into()],
            ..Default::default()
        };
        let lower = Settings {
            port: Some(3000),
            debounce_ms: Some(50),
            exclude_patterns: vec!["**/user/**".into()],
            ..Default::default()
        };
        let merged = upper.over(lower);
        assert_eq!(merged.port, Some(3300));
        assert_eq!(merged.debounce_ms, Some(50));
        assert_eq!(
            merged.exclude_patterns,
            vec!["**/user/**".to_string(), "**/repo/**".to_string()]
        );
    }

    /// Extend, not replace: one extra ignore rule must not silently re-enable
    /// scanning `node_modules` and `target`.
    #[test]
    fn exclude_patterns_extend_the_defaults() {
        let mut config = Config::default();
        let before = config.analysis.exclude_patterns.len();
        let settings = Settings {
            exclude_patterns: vec!["**/generated/**".into()],
            ..Default::default()
        };
        settings.apply_to_config(&mut config);
        assert_eq!(config.analysis.exclude_patterns.len(), before + 1);
        assert!(
            config
                .analysis
                .exclude_patterns
                .iter()
                .any(|p| p.contains("node_modules"))
        );
        assert!(
            config
                .analysis
                .exclude_patterns
                .iter()
                .any(|p| p == "**/generated/**")
        );
    }

    /// CFG-005. The durable way to ask for assignment detail: `nao watch` and
    /// the VS Code extension are launched without anyone typing a flag, so a
    /// repo that wants locals has to be able to say so in the file.
    #[test]
    fn the_file_can_ask_for_local_assignments() {
        let mut config = Config::default();
        assert!(!config.analysis.include_locals, "locals are off by default");
        let settings = Settings {
            include_locals: Some(true),
            ..Default::default()
        };
        settings.apply_to_config(&mut config);
        assert!(config.analysis.include_locals);
    }

    /// Silence in the file leaves the default alone, the way every other
    /// widening switch here behaves.
    #[test]
    fn a_file_that_says_nothing_about_locals_leaves_them_off() {
        let mut config = Config::default();
        Settings::default().apply_to_config(&mut config);
        assert!(!config.analysis.include_locals);
    }

    /// The regression this pair exists for: `max_depth` was applied
    /// unconditionally, so `nao analyze --depth 7` on a repo whose file said
    /// 3 traversed 3. `spec_dir` and `languages` had tests for exactly this
    /// and the scalars did not, which is how it survived.
    #[test]
    fn a_depth_flag_outranks_the_file() {
        let mut config = Config::default();
        let settings = Settings { max_depth: Some(3), ..Default::default() };
        settings.apply_with(&mut config, Flags { max_depth: Some(7), ..Default::default() });
        assert_eq!(config.analysis.max_depth, 7);
    }

    #[test]
    fn the_file_depth_applies_when_no_flag_was_passed() {
        let mut config = Config::default();
        let settings = Settings { max_depth: Some(9), ..Default::default() };
        settings.apply_with(&mut config, Flags::default());
        assert_eq!(config.analysis.max_depth, 9);
    }

    /// Silence from both leaves whatever the command already chose — this is
    /// what keeps `nao deps`'s shallower default of 2 alive.
    #[test]
    fn neither_flag_nor_file_leaves_the_commands_own_depth() {
        let mut config = Config::default();
        config.analysis.max_depth = 2;
        Settings::default().apply_with(&mut config, Flags::default());
        assert_eq!(config.analysis.max_depth, 2);
    }

    /// `min_weight` has no flag yet. The guard is here so that adding one is
    /// a change to `main.rs` alone.
    #[test]
    fn a_min_weight_flag_would_outrank_the_file() {
        let mut config = Config::default();
        let settings = Settings { min_weight: Some(1), ..Default::default() };
        settings.apply_with(&mut config, Flags { min_weight: Some(5), ..Default::default() });
        assert_eq!(config.filters.min_weight, 5);
    }

    #[test]
    fn a_cli_language_filter_outranks_the_file() {
        let mut config = Config::default();
        config.analysis.languages.insert(Language::Rust);
        let settings = Settings {
            language: Some(vec!["python".into()]),
            ..Default::default()
        };
        settings.apply_to_config(&mut config);
        assert_eq!(config.analysis.languages.len(), 1);
        assert!(config.analysis.languages.contains(&Language::Rust));
    }

    #[test]
    fn the_file_language_applies_when_the_cli_named_none() {
        let mut config = Config::default();
        let settings = Settings {
            language: Some(vec!["python".into()]),
            ..Default::default()
        };
        settings.apply_to_config(&mut config);
        assert!(config.analysis.languages.contains(&Language::Python));
    }

    fn os(s: &str) -> Option<std::ffi::OsString> {
        Some(std::ffi::OsString::from(s))
    }

    #[test]
    fn config_root_prefers_the_explicit_override() {
        assert_eq!(
            config_root_from(os("/explicit"), os("/xdg"), os("/home")),
            Some(PathBuf::from("/explicit")),
        );
    }

    #[test]
    fn config_root_uses_xdg_before_home() {
        assert_eq!(
            config_root_from(None, os("/xdg"), os("/home")),
            Some(PathBuf::from("/xdg/nao")),
        );
    }

    /// With `XDG_CONFIG_HOME` set, `~/.config/nao` is not consulted at all.
    #[test]
    fn config_root_falls_back_to_dot_config_under_home() {
        assert_eq!(
            config_root_from(None, None, os("/home")),
            Some(PathBuf::from("/home/.config/nao")),
        );
    }

    /// No `HOME` — some CI and container environments. The answer is "no
    /// settings file", not a synthesized one; `cache_root` behaves the same.
    #[test]
    fn config_root_is_none_without_a_home() {
        assert_eq!(config_root_from(None, None, None), None);
    }

    /// An exported-but-empty variable is a shell accident, not a choice.
    #[test]
    fn config_root_treats_empty_as_unset() {
        assert_eq!(
            config_root_from(os(""), os(""), os("/home")),
            Some(PathBuf::from("/home/.config/nao")),
        );
    }
}
