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

use serde::Deserialize;

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
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Settings {
    // ---- user scope only ----
    /// Where the built browser UI lives. Installation property: see the
    /// module docs for why this is not repo-scoped.
    pub ui_dir: Option<PathBuf>,
    /// Fallback Educator content root.
    pub content_fallback: Option<PathBuf>,

    // ---- repo scope only ----
    /// Where `watch` writes the JSON it serves. Repo-relative by default
    /// (`ui/public`), so a machine-wide value would point every analyzed repo
    /// at one directory — a footgun rather than a feature.
    pub output_dir: Option<PathBuf>,

    // ---- both scopes ----
    pub language: Option<Vec<String>>,
    pub kind: Option<Vec<EntityKind>>,
    pub include_tests: Option<bool>,
    pub include_external: Option<bool>,
    pub max_depth: Option<usize>,
    pub min_weight: Option<u32>,
    pub port: Option<u16>,
    pub debounce_ms: Option<u64>,
    /// Extra ignore globs. **Extends** the built-in defaults rather than
    /// replacing them (ADR-0008): writing one extra rule should not silently
    /// re-enable scanning `node_modules`.
    pub exclude_patterns: Vec<String>,
    /// Extra include globs. Extends, for symmetry with `exclude_patterns`.
    pub include_patterns: Vec<String>,

    /// Everything serde did not recognise. Captured rather than dropped so a
    /// key can be named back to whoever wrote it — a silently ignored setting
    /// reads as "nao is broken" to an author who meant well, and tells a
    /// reader being attacked nothing at all.
    #[serde(flatten)]
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
    root.join(REPO_DIR).join(FILE_NAME)
}

/// Read and validate one settings file. Absent is the normal case and says
/// nothing; unreadable or malformed warns and yields defaults.
///
/// A bad settings file is never fatal. `nao analyze` does not need the UI
/// directory and must not die because of a stray comma in a file it barely
/// consults.
fn read(path: &Path, scope: Scope) -> Settings {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Settings::default();
    };
    match serde_json::from_str::<Settings>(&text) {
        Ok(mut settings) => {
            settings.validate(scope, path);
            settings
        }
        Err(e) => {
            eprintln!("   ⚠ {}: {e} — ignoring it.", path.display());
            Settings::default()
        }
    }
}

/// The user-scope settings alone. This is what `nao serve` gets: the
/// operator's own preferences apply, the submitted repo's do not.
pub fn user() -> Settings {
    user_path().map_or_else(Settings::default, |p| read(&p, Scope::User))
}

/// Both scopes, repo winning over user, for a trusted local root.
pub fn load(root: &Path) -> Settings {
    let repo = read(&repo_path(root), Scope::Repo);
    repo.over(user())
}

impl Settings {
    /// Name every key that will not be honoured, then clear it.
    fn validate(&mut self, scope: Scope, from: &Path) {
        self.report_rejected(from);
        if scope == Scope::Repo {
            self.clear_user_only(from);
        } else {
            self.clear_repo_only(from);
        }
    }

    /// Report unrecognized keys, calling out the privileged ones by the thing
    /// they would have granted.
    fn report_rejected(&self, from: &Path) {
        for key in self.unrecognized.keys() {
            match REJECTED.iter().find(|(name, _)| name == key) {
                Some((_, why)) => eprintln!(
                    "   ⚠ {}: `{key}` is never settable from a settings file — {why}. \
                     Pass it as a flag instead.",
                    from.display()
                ),
                None => eprintln!("   ⚠ {}: unknown setting `{key}` — ignoring it.", from.display()),
            }
        }
    }

    fn clear_user_only(&mut self, from: &Path) {
        for (key, present) in [
            ("ui_dir", self.ui_dir.is_some()),
            ("content_fallback", self.content_fallback.is_some()),
        ] {
            if present {
                eprintln!(
                    "   ⚠ {}: `{key}` describes this installation, not this repo — \
                     move it to the user settings file.",
                    from.display()
                );
            }
        }
        self.ui_dir = None;
        self.content_fallback = None;
    }

    fn clear_repo_only(&mut self, from: &Path) {
        if self.output_dir.is_some() {
            eprintln!(
                "   ⚠ {}: `output_dir` is repo-relative — set it in a repo's \
                 .nao/settings.json, not machine-wide.",
                from.display()
            );
        }
        self.output_dir = None;
    }

    /// `self` layered over `lower`: every scalar `self` sets wins, and the
    /// pattern lists concatenate so a user's machine-wide ignores survive a
    /// repo adding its own.
    fn over(self, lower: Settings) -> Settings {
        Settings {
            ui_dir: self.ui_dir.or(lower.ui_dir),
            content_fallback: self.content_fallback.or(lower.content_fallback),
            output_dir: self.output_dir.or(lower.output_dir),
            language: self.language.or(lower.language),
            kind: self.kind.or(lower.kind),
            include_tests: self.include_tests.or(lower.include_tests),
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
    pub fn apply_to_config(&self, config: &mut Config) {
        if let Some(v) = self.include_tests {
            config.analysis.include_tests |= v;
        }
        if let Some(v) = self.include_external {
            config.analysis.include_external |= v;
        }
        if let Some(v) = self.max_depth {
            config.analysis.max_depth = v;
        }
        if let Some(v) = self.min_weight {
            config.filters.min_weight = v;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let settings = read(&path, Scope::User);
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
        let settings = read(&path, Scope::Repo);
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
        let settings = read(&path, Scope::Repo);
        assert_eq!(settings.unrecognized.len(), 3);
    }

    #[test]
    fn repo_scope_drops_installation_keys() {
        let dir = TempConfig::new("repo-uidir");
        let path = dir.write(r#"{"ui_dir":"/tmp/x","content_fallback":"/tmp/y","language":["rust"]}"#);
        let settings = read(&path, Scope::Repo);
        assert_eq!(settings.ui_dir, None);
        assert_eq!(settings.content_fallback, None);
        assert_eq!(settings.language, Some(vec!["rust".to_string()]));
    }

    #[test]
    fn user_scope_drops_repo_relative_keys() {
        let dir = TempConfig::new("user-outdir");
        let path = dir.write(r#"{"output_dir":"ui/public"}"#);
        assert_eq!(read(&path, Scope::User).output_dir, None);
    }

    #[test]
    fn malformed_json_yields_defaults_rather_than_failing() {
        let dir = TempConfig::new("malformed");
        let path = dir.write("{ not json");
        let settings = read(&path, Scope::User);
        assert!(settings.ui_dir.is_none());
        assert!(settings.include_tests.is_none());
    }

    #[test]
    fn absent_file_is_not_an_error() {
        let settings = read(Path::new("/nonexistent/nao/settings.json"), Scope::User);
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
