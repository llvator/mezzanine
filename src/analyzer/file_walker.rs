//! File system walker for discovering source files.

use crate::config::{Config, DEFAULT_EXCLUDE_PATTERNS};
use crate::models::file_info::Language;
use anyhow::Result;
use glob::Pattern;
use ignore::WalkBuilder;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::Cancelled;

/// The test-file heuristic used by `include_tests: false`, over a path
/// spelled the way [`PatternBase`] spells one — relative to the repo root.
///
/// The spelling is the whole rule. Matched against an *absolute* path, as
/// this was, it reads the directories above the checkout, which nobody
/// chose as part of the project's layout and whose intent mezz cannot see.
/// A byte-identical tree then analysed to 9 entities at `~/mezzrepro-plain`
/// and to **0** at `~/mezzrepro-tests` (field report, 2026-08-31), and with
/// `include_tests: false` every file in such a repo matched, so the walk
/// returned nothing and the Stop hook went silently green — quiet-when-blind
/// being byte-for-byte what quiet-when-clean looks like.
///
/// `contest` is the case that shows this was not a naming convention that
/// got slightly too eager: `inspect`, `prospect` and `spectrum` follow from
/// `spec`, and `latest`, `greatest` and `attestation` from `test`. A
/// checkout at `~/clients/prospect-app` or `~/work/latest/api` analysed to
/// nothing at all.
///
/// Still a substring match *within* the repo-relative spelling, and
/// deliberately: `tests/`, `_test.rs`, `spec/` and `__tests__/` are all
/// spellings a project chose for itself, and narrowing the rule to whole
/// path components is a separate change with its own blast radius over
/// every repo relying on the loose form today.
fn names_a_test(spelled: &str) -> bool {
    let lower = spelled.to_lowercase();
    lower.contains("test") || lower.contains("spec")
}

/// The test-file heuristic, held against the project it is a claim about.
///
/// Public so consumers that need to *classify* files (rather than exclude
/// them, e.g. the MCP `tests_for` tool) apply the identical rule.
///
/// Rooted rather than free-standing, because a free-standing one could not
/// be called correctly: the answer depends on where the project starts, and
/// the version that did not ask for a root silently emptied whole
/// repositories. Rooted at the *repo* root [`PatternBase`] resolves, so one
/// file is test code or not whichever subdirectory the analysis was pointed
/// at — `mezz analyze .` and `mezz analyze src` must not disagree about
/// `src/parser/tests.rs`.
pub struct TestPaths(PatternBase);

impl TestPaths {
    /// The heuristic as the project at `root` spells it.
    pub fn rooted_at(root: &Path) -> Self {
        Self(PatternBase::of(root))
    }

    /// Whether `path` is test code by the project's own naming.
    pub fn matches(&self, path: &Path) -> bool {
        names_a_test(&self.0.spell(path))
    }
}

/// What a walk is looking for. The spec directory may sit outside the
/// analyzed root, and walking it must not drag that whole tree's code into
/// the graph — `mezz watch services/api --spec-dir ../..` would otherwise
/// analyze the entire monorepo by accident.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Admit {
    /// Everything the filters allow. The root walk.
    Everything,
    /// Elevator files alone. The spec-directory walk.
    SpecOnly,
}

/// One configured glob, kept with what it takes to report on it.
///
/// A bare [`Pattern`] cannot say how it was spelled — `Pattern` has no
/// accessor for its source — and cannot say whether anyone chose it. Both
/// are needed the moment a walk has to describe a pattern back to whoever
/// wrote it (CFG-015).
struct Glob {
    pattern: Pattern,
    /// As the author spelled it, so a diagnostic quotes back the string they
    /// can search their settings file for.
    spelling: String,
    /// A shipped default rather than something somebody wrote. Exempt from
    /// the zero-match report — see [`DEFAULT_EXCLUDE_PATTERNS`].
    shipped: bool,
}

impl Glob {
    /// Compile one configured list, naming whatever it has to drop.
    ///
    /// Globs written in a settings file have already been checked by
    /// `Settings::clear_malformed_globs`, which can name the file each came
    /// from and is the better diagnostic. This is the backstop for a list
    /// that reached a [`Config`] some other way: it says the same thing
    /// minus the filename, rather than discarding the entry in silence the
    /// way `Pattern::new(p).ok()` used to.
    fn compile(key: &str, spellings: &[String], shipped: &[&str]) -> Vec<Glob> {
        spellings
            .iter()
            .filter_map(|spelling| match Pattern::new(spelling) {
                Ok(pattern) => Some(Glob {
                    pattern,
                    spelling: spelling.clone(),
                    shipped: shipped.contains(&spelling.as_str()),
                }),
                Err(e) => {
                    crate::activity::warn(
                        crate::activity::ANALYSIS,
                        format!("   ⚠ {key}: `{spelling}` is not a valid glob ({e}) — ignoring it."),
                    );
                    None
                }
            })
            .collect()
    }
}

/// What a configured glob is matched against: the walked path, restated
/// relative to the repo root.
///
/// Not the string the walk happens to be carrying. That one starts `./src/`
/// for `mezz analyze .`, `src/` for `mezz analyze src` and `/home/…/src/` for
/// an absolute root, so `src/contracts.d.ts` — the spelling an author would
/// write down, the one their editor and their `git status` both use — was the
/// one spelling that could never match, and the base was discoverable only by
/// experiment (CFG-013).
///
/// The *repo* root rather than the analyzed root, because that is where the
/// settings file holding the patterns is read from (CFG-012). The file and
/// the patterns inside it then agree about what "the repo" means, and one
/// spelling works from any subdirectory. Outside a checkout
/// [`crate::settings::repo_root`] answers with the analyzed root, so a loose
/// directory keeps the behaviour it had.
///
/// What this deliberately does *not* change is that `*` crosses `/`:
/// `Pattern::matches` uses default options, where `require_literal_separator`
/// is false, so `*.d.ts` matches `src/a.d.ts`. It is not the better
/// semantics — it makes `**` decorative — but every pattern that works today
/// relies on it, and a filter that quietly stops filtering is the failure
/// this ticket is about. Both halves are written down in the settings schema.
struct PatternBase {
    /// The repo root, absolute. `None` when a relative root had no working
    /// directory to resolve against — a process whose cwd has been deleted —
    /// which leaves every path spelled as the walk found it.
    root: Option<PathBuf>,
    /// The directory the walk's relative paths are relative to. Read once:
    /// `current_dir` is a syscall, and this question is asked per file.
    cwd: Option<PathBuf>,
}

impl PatternBase {
    /// The base for an analyzed root, resolved against the process working
    /// directory.
    fn of(analyzed_root: &Path) -> Self {
        Self::rooted(analyzed_root, std::env::current_dir().ok())
    }

    /// Split from [`Self::of`] so a test can ask the three invocations of one
    /// repo — `.`, `src`, and the absolute path to `src` — what they make of
    /// the same file. The alternative is `set_current_dir`, which is
    /// process-wide and racy under a threaded test runner.
    fn rooted(analyzed_root: &Path, cwd: Option<PathBuf>) -> Self {
        let root = Self::absolute(analyzed_root, cwd.as_deref())
            // Asked absolutely, so `repo_root` answers absolutely, and
            // without consulting an environment this already read.
            .map(|absolute| crate::settings::repo_root(&absolute));
        Self { root, cwd }
    }

    /// `path` made absolute without touching the filesystem — the file may
    /// have just been deleted, and resolving symlinks would answer for a
    /// directory nobody named.
    fn absolute(path: &Path, cwd: Option<&Path>) -> Option<PathBuf> {
        match path.is_absolute() {
            true => Some(path.to_path_buf()),
            false => cwd.map(|cwd| cwd.join(path)),
        }
    }

    /// `path` as a pattern sees it.
    ///
    /// Left as the walk spelled it for a path outside the repo: the spec
    /// directory of `--spec-dir ../docs` has no repo-relative spelling, and
    /// one built out of `..` would match nothing anybody wrote.
    fn spell(&self, path: &Path) -> String {
        let relative = self
            .root
            .as_deref()
            .zip(Self::absolute(path, self.cwd.as_deref()))
            .and_then(|(root, absolute)| Some(absolute.strip_prefix(root).ok()?.to_path_buf()));
        match relative {
            Some(relative) => relative.to_string_lossy().into_owned(),
            None => path.to_string_lossy().into_owned(),
        }
    }
}

/// Walks the file system to discover source code files.
pub struct FileWalker<'a> {
    config: &'a Config,
    exclude_patterns: Vec<Glob>,
    include_patterns: Vec<Glob>,
    /// The base every pattern is matched against — see [`PatternBase`].
    base: PatternBase,
    /// The resolved [`Config::spec_root`], once it has been confirmed to
    /// exist. `None` means "every `.elv` under the root is the spec" — the
    /// default, and also what a `spec_dir` pointing at nothing falls back
    /// to, having said so.
    spec_root: Option<PathBuf>,
}

impl<'a> FileWalker<'a> {
    pub fn new(config: &'a Config) -> Self {
        let exclude_patterns = Glob::compile(
            "exclude_patterns",
            &config.analysis.exclude_patterns,
            DEFAULT_EXCLUDE_PATTERNS,
        );
        // Nothing ships an include pattern: the default is "include
        // everything", spelled as an empty list. So every entry here is one
        // somebody wrote, and every one of them is worth reporting on.
        let include_patterns =
            Glob::compile("include_patterns", &config.analysis.include_patterns, &[]);

        Self {
            config,
            exclude_patterns,
            include_patterns,
            base: PatternBase::of(&config.root_path),
            spec_root: resolve_spec_root(config),
        }
    }

    /// Walk the directory tree and return all matching source files.
    ///
    /// Uses the `ignore` crate (same one ripgrep uses) so `.gitignore`,
    /// `.ignore`, the global gitignore, and hidden-file rules are honored
    /// out of the box. We then layer the mezz-specific filters on top
    /// (language, test files, explicit exclude patterns from Config).
    pub fn walk(&self, root: &Path) -> Result<Vec<PathBuf>> {
        let cancel = Arc::new(AtomicBool::new(false));
        self.walk_with_cancel(root, &cancel)
    }

    /// Same as `walk`, but bails out with `Cancelled` if `cancel` is set
    /// during the directory walk. Polled per-entry so the latency of
    /// aborting is bounded by one filesystem read.
    pub fn walk_with_cancel(&self, root: &Path, cancel: &Arc<AtomicBool>) -> Result<Vec<PathBuf>> {
        self.collect(root, cancel, Admit::Everything)
    }

    /// Every file an analysis should parse: the root, plus the spec directory
    /// when it lies outside the root. The walker rather than the analyzer
    /// decides how many roots that takes, because "where do specs come from"
    /// is a question about file selection.
    pub fn walk_all(&self, root: &Path, cancel: &Arc<AtomicBool>) -> Result<Vec<PathBuf>> {
        let mut files = self.collect(root, cancel, Admit::Everything)?;
        files.extend(self.walk_spec_with_cancel(cancel)?);
        Ok(files)
    }

    /// Walk the spec directory for `.elv` files alone, for the case where it
    /// lies outside the analyzed root — see [`Config::spec_is_outside_root`].
    /// Empty when there is no such directory, so the caller can always
    /// concatenate.
    pub fn walk_spec_with_cancel(&self, cancel: &Arc<AtomicBool>) -> Result<Vec<PathBuf>> {
        match self
            .spec_root
            .as_ref()
            .filter(|_| self.config.spec_is_outside_root())
        {
            Some(dir) => self.collect(&dir.clone(), cancel, Admit::SpecOnly),
            None => Ok(Vec::new()),
        }
    }

    /// The walk itself, shared by both entry points, with whatever the tally
    /// has to say said on stderr.
    ///
    /// stderr and never stdout: `mezz mcp` puts protocol JSON on stdout, and a
    /// diagnostic there is a parse error rather than a warning.
    fn collect(&self, root: &Path, cancel: &Arc<AtomicBool>, admit: Admit) -> Result<Vec<PathBuf>> {
        let (files, unmatched) = self.collect_counting(root, cancel, admit)?;
        for line in unmatched {
            crate::activity::warn(crate::activity::ANALYSIS, format!("   ⚠ {line}"));
        }
        Ok(files)
    }

    /// The walk, plus one line per configured pattern it never matched.
    ///
    /// Split from [`Self::collect`] so a test can read the report instead of
    /// scraping stderr — the report is the feature, and a feature nothing can
    /// assert on is one nothing pins.
    fn collect_counting(
        &self,
        root: &Path,
        cancel: &Arc<AtomicBool>,
        admit: Admit,
    ) -> Result<(Vec<PathBuf>, Vec<String>)> {
        let mut files = Vec::new();
        let mut tally = Tally::new(self, admit);
        let noun = if admit == Admit::SpecOnly {
            "spec"
        } else {
            "source"
        };

        let spinner = ProgressBar::new_spinner();
        spinner.set_draw_target(crate::activity::progress_target());
        spinner.set_style(
            ProgressStyle::with_template("{spinner:.cyan} {msg}")
                .unwrap()
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"),
        );
        spinner.set_message(format!("Discovering {noun} files..."));

        let walker = WalkBuilder::new(root)
            .follow_links(true)
            // Enable all the ignore sources by default.
            .git_ignore(true)
            .git_exclude(true)
            .git_global(true)
            .hidden(true)
            .parents(true)
            // Use a reasonable number of threads.
            .threads(0)
            .build();

        for entry in walker {
            if cancel.load(Ordering::Relaxed) {
                spinner.finish_and_clear();
                return Err(Cancelled.into());
            }
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue, // Skip unreadable entries silently
            };
            let path = entry.path();
            // Free — the walk read this off the directory entry already —
            // where `path.is_file()` would stat every path a second time,
            // including all the ones the excludes are about to drop.
            let is_file = entry.file_type().is_some_and(|t| t.is_file());

            // Spelled once and handed to everything that matches on it, so
            // the tally cannot count a pattern the filters read differently.
            let spelled = self.base.spell(path);

            // Before the filters, not after: a pattern's whole job is to
            // remove files, so the ones it removed are exactly the ones that
            // prove it works.
            if is_file {
                tally.record(&spelled);
            }

            // Apply mezz-specific exclude patterns (additive to .gitignore).
            if self.should_skip_dir(path, &spelled) {
                continue;
            }

            if is_file && self.should_include_file(path, &spelled) && self.admits(path, admit) {
                files.push(path.to_path_buf());
                spinner.set_message(format!("Discovered {} {noun} files...", files.len()));
            }
            spinner.tick();
        }

        spinner.finish_with_message(format!("Discovered {} {noun} files", files.len()));
        // `mirror`, not `step`: the spinner above has already drawn this line
        // in place, and printing it again would double it on the terminal.
        crate::activity::mirror(
            crate::activity::ANALYSIS,
            format!("Discovered {} {noun} files", files.len()),
        );
        Ok((files, tally.zero_matches()))
    }

    /// Whether a walk under this config would parse `path`.
    ///
    /// Public so the file watcher can ask the walk its own question instead
    /// of keeping a second, looser list of extensions beside it. The two must
    /// agree by construction: a watcher stricter than the walk stops
    /// refreshing a file that is in the graph — the graph goes stale and
    /// nothing says so — and a looser one re-analyzes the whole repo every
    /// time a build writes into `target/`.
    ///
    /// One rule is deliberately missing: `.gitignore`. The walk gets that
    /// from the `ignore` crate at walk time, and a matcher built once from a
    /// config would answer with the ignore rules as they were when it was
    /// built. The watcher pairs this with `git check-ignore`, which is never
    /// stale.
    ///
    /// Pure — no filesystem access — so it answers for a path that has just
    /// been deleted as readily as for one that exists.
    pub fn would_analyze(&self, path: &Path) -> bool {
        !self.under_hidden_dir(path)
            && self.should_include_file(path, &self.base.spell(path))
            && self.admits(path, Admit::Everything)
    }

    /// Whether any component below the analyzed root is a dot-directory,
    /// which is the walk's `hidden(true)` rule restated for a single path.
    /// Below the root, because the root itself may legitimately live under
    /// one — a checkout inside `~/.cache` is still a repo to analyze.
    fn under_hidden_dir(&self, path: &Path) -> bool {
        path.strip_prefix(&self.config.root_path)
            .unwrap_or(path)
            .components()
            .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
    }

    /// The two rules `spec_dir` adds on top of the ordinary filters.
    ///
    /// A spec walk admits Elevator files and nothing else, so pointing at a
    /// docs repo — or at the root of a monorepo — costs one directory
    /// traversal rather than a second analysis.
    ///
    /// And once a spec directory is named, it is the *only* place a spec can
    /// come from: an `.elv` fixture under `examples/` stops being a Feature
    /// in the graph. Without this half, `spec_dir` could add specs but never
    /// exclude any, and the repo with stray `.elv` files it was meant to
    /// help would be no better off.
    fn admits(&self, path: &Path, admit: Admit) -> bool {
        let is_spec = crate::parser::detect_language(path) == Language::Elevator;
        if admit == Admit::SpecOnly && !is_spec {
            return false;
        }
        match (&self.spec_root, is_spec) {
            (Some(root), true) => path.starts_with(root),
            _ => true,
        }
    }

    /// Check if a directory should be skipped. `spelled` is `path` as a
    /// pattern sees it — see [`PatternBase`].
    fn should_skip_dir(&self, path: &Path, spelled: &str) -> bool {
        // Check exclude patterns
        if self
            .exclude_patterns
            .iter()
            .any(|g| g.pattern.matches(spelled))
        {
            return true;
        }

        // Skip hidden directories
        if let Some(name) = path.file_name() {
            let name = name.to_string_lossy();
            if name.starts_with('.') && name != "." && name != ".." {
                return true;
            }
        }

        false
    }

    /// Check if a file should be included in analysis. `spelled` is `path`
    /// as a pattern sees it — see [`PatternBase`].
    fn should_include_file(&self, path: &Path, spelled: &str) -> bool {
        // Path-aware detection: ansible-deploy files are recognised by
        // repo layout, everything else by extension.
        let language = crate::parser::detect_language(path);

        // Skip unknown languages
        if matches!(language, Language::Unknown) {
            return false;
        }

        // Language filter, including the opt-in rule for Markdown. Asked of
        // the config rather than restated here, so the parse loop cannot
        // answer it differently — see `AnalysisConfig::accepts_language`.
        if !self.config.analysis.accepts_language(language) {
            return false;
        }

        // Check test files — except Elevator specs and Markdown docs.
        // `.elv` files are domain specs, not code, and legitimately live at
        // paths like `my-spec/` or `spec.elv` that the substring heuristic
        // would silently swallow. The same is true of a doc: `docs/testing.md`
        // is a document *about* tests, and dropping it would tear a hole in
        // the link graph that the notes still pointing at it cannot explain.
        // `spelled`, not `path`: the repo-relative spelling is what makes
        // this the project's own naming rather than its owner's directory
        // layout — see [`names_a_test`]. That string was already computed
        // and sitting in this argument list while the match read the
        // absolute path beside it.
        if !self.config.analysis.include_tests
            && language != Language::Elevator
            && language != Language::Markdown
            && names_a_test(spelled)
        {
            return false;
        }

        // Check exclude patterns
        if self
            .exclude_patterns
            .iter()
            .any(|g| g.pattern.matches(spelled))
        {
            return false;
        }

        // Check include patterns (if any specified, file must match at least one)
        if !self.include_patterns.is_empty() {
            let matches_any = self
                .include_patterns
                .iter()
                .any(|g| g.pattern.matches(spelled));
            if !matches_any {
                return false;
            }
        }

        true
    }
}

/// How many files each written-down pattern matched, across one walk.
///
/// A pattern that excludes nothing produces exactly the same graph as one
/// that is doing its job, so the only way to tell a typo from a working rule
/// used to be counting entities before and after. This counts instead
/// (CFG-015).
///
/// Counted here rather than inside the matchers, for two reasons. The
/// matchers stop at the first pattern that settles the answer, so a pattern
/// they never *reached* is not a pattern that matched nothing. And
/// [`FileWalker::would_analyze`] asks them about paths that are not part of
/// any walk, which would inflate a tally kept there.
///
/// The walk is a single loop over one iterator, so a plain `usize` per
/// pattern is enough: no lock ever enters the match.
struct Tally<'a> {
    counted: Vec<Counted<'a>>,
}

/// One pattern a report could name, and its running total.
struct Counted<'a> {
    /// The settings key it was written under, which is the half of the
    /// diagnostic that says where to go and fix it.
    key: &'static str,
    glob: &'a Glob,
    files: usize,
}

impl<'a> Tally<'a> {
    /// Only the patterns a report could name: somebody wrote them, so a zero
    /// is news. Empty in the ordinary repo that configures none, which is
    /// what keeps [`Self::record`] free there.
    fn new(walker: &'a FileWalker, admit: Admit) -> Self {
        // A spec walk admits `.elv` files alone, so every code pattern would
        // look inert in it. Only the walk that answers for the whole tree is
        // entitled to say a pattern matched nothing.
        if admit != Admit::Everything {
            return Self {
                counted: Vec::new(),
            };
        }
        let counted = [
            ("exclude_patterns", &walker.exclude_patterns),
            ("include_patterns", &walker.include_patterns),
        ]
        .into_iter()
        .flat_map(|(key, globs)| {
            globs
                .iter()
                .filter(|glob| !glob.shipped)
                .map(move |glob| Counted {
                    key,
                    glob,
                    files: 0,
                })
        })
        .collect();
        Self { counted }
    }

    /// `spelled` is the walked path as a pattern sees it, computed by the
    /// walk and passed in rather than recomputed: a tally that spelled a path
    /// differently from the filters would report a working pattern as inert.
    fn record(&mut self, spelled: &str) {
        if self.counted.is_empty() {
            return;
        }
        for counted in &mut self.counted {
            counted.files += usize::from(counted.glob.pattern.matches(spelled));
        }
    }

    /// One line per pattern the walk never matched, in the order they were
    /// configured.
    fn zero_matches(&self) -> Vec<String> {
        self.counted
            .iter()
            .filter(|counted| counted.files == 0)
            .map(|counted| {
                format!(
                    "{}: \"{}\" matched 0 files — check the spelling.",
                    counted.key, counted.glob.spelling
                )
            })
            .collect()
    }
}

/// [`Config::spec_root`], dropped with an explanation when it names a
/// directory that isn't there.
///
/// A typo'd `spec_dir` and a repo with no spec look identical from the
/// graph — both produce an empty spec layer — so the failure has to be said
/// out loud at the moment it is detected. Falling back to the default
/// (every `.elv` under the root) rather than to an empty spec is the
/// friendlier half of the same argument: a reader who mistyped a path still
/// gets the spec they had yesterday.
fn resolve_spec_root(config: &Config) -> Option<PathBuf> {
    let root = config.spec_root()?;
    if root.is_dir() {
        return Some(root);
    }
    crate::activity::warn(
        crate::activity::ANALYSIS,
        format!(
            "   ⚠ spec_dir: {} is not a directory — ignoring it, and treating \
             every .elv under the root as the spec.",
            root.display()
        ),
    );
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_language_detection() {
        assert_eq!(Language::from_extension("rs"), Language::Rust);
        assert_eq!(Language::from_extension("py"), Language::Python);
        assert_eq!(Language::from_extension("js"), Language::JavaScript);
        assert_eq!(Language::from_extension("ts"), Language::TypeScript);
        assert_eq!(Language::from_extension("java"), Language::Java);
        assert_eq!(Language::from_extension("go"), Language::Go);
    }

    /// A throwaway tree. Same shape as the one in `elevator_drift`, for the
    /// same reason: these tests are about which files a walk finds, and that
    /// cannot be demonstrated without real directories.
    struct TmpDir(PathBuf);

    impl TmpDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "mezz-walker-{}-{}-{}",
                name,
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0),
            ));
            fs::create_dir_all(&path).unwrap();
            TmpDir(path)
        }

        fn write(&self, rel: &str, body: &str) {
            let full = self.0.join(rel);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(&full, body).unwrap();
        }
    }

    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A repo with its spec in one place and a stray `.elv` in another.
    fn repo_with_a_stray_spec(tag: &str) -> TmpDir {
        let dir = TmpDir::new(tag);
        dir.write("src/main.rs", "fn main() {}\n");
        dir.write("docs/domain/mezz.elv", "c.library \"Library\"\n");
        dir.write("examples/tutorial/toy.elv", "c.toy \"Toy\"\n");
        dir
    }

    /// Not named `walk`: the complexity gate keys on file + function name, and
    /// a test helper sharing a name with the method above silently replaces
    /// its metrics in the comparison.
    fn walked_names(config: &Config) -> Vec<String> {
        let cancel = Arc::new(AtomicBool::new(false));
        FileWalker::new(config)
            .walk_all(&config.root_path, &cancel)
            .unwrap()
            .iter()
            .filter_map(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .collect()
    }

    /// The behaviour every repo without a `spec_dir` has always had, pinned
    /// so the new gate cannot quietly change it.
    #[test]
    fn without_a_spec_dir_every_elv_under_the_root_is_the_spec() {
        let dir = repo_with_a_stray_spec("no-spec-dir");
        let config = Config::for_path(&dir.0);
        let names = walked_names(&config);
        assert!(names.contains(&"mezz.elv".to_string()));
        assert!(names.contains(&"toy.elv".to_string()));
    }

    /// The narrowing half: naming a directory inside the root demotes the
    /// `.elv` files outside it back to ordinary files nobody parses.
    #[test]
    fn an_in_root_spec_dir_excludes_elv_files_outside_it() {
        let dir = repo_with_a_stray_spec("in-root");
        let mut config = Config::for_path(&dir.0);
        config.analysis.spec_dir = Some(PathBuf::from("docs/domain"));
        let names = walked_names(&config);
        assert!(names.contains(&"mezz.elv".to_string()));
        assert!(!names.contains(&"toy.elv".to_string()), "{names:?}");
        // Code is untouched — this filter is about the spec layer only.
        assert!(names.contains(&"main.rs".to_string()));
    }

    /// The widening half, and the case the setting mainly exists for: a spec
    /// that lives outside the analyzed tree entirely.
    #[test]
    fn an_out_of_root_spec_dir_is_walked_for_elv_files() {
        let outside = TmpDir::new("outside-spec");
        outside.write("api.elv", "c.api \"API\"\n");
        // Code in the spec repo must not come along: the point of a separate
        // walk is one directory traversal, not a second analysis.
        outside.write("tooling.rs", "fn helper() {}\n");

        let dir = repo_with_a_stray_spec("out-of-root");
        let mut config = Config::for_path(&dir.0);
        config.analysis.spec_dir = Some(outside.0.clone());
        let names = walked_names(&config);
        assert!(names.contains(&"api.elv".to_string()), "{names:?}");
        assert!(!names.contains(&"tooling.rs".to_string()), "{names:?}");
        // And the root's own `.elv` files are no longer the spec.
        assert!(!names.contains(&"mezz.elv".to_string()), "{names:?}");
        assert!(names.contains(&"main.rs".to_string()));
    }

    /// Every kind of path the watcher has to make a call about.
    const WATCHED: [&str; 7] = [
        "src/main.rs",
        "src/lib_test.rs",
        "target/debug/generated.rs",
        ".hidden/tool.rs",
        "docs/readme.md",
        "notes.txt",
        "spec/mezz.elv",
    ];

    /// `would_analyze` has to answer, for one path, exactly what a walk
    /// answers for all of them — asserted against a real walk rather than a
    /// list of expectations, because the failure being guarded against is the
    /// two drifting apart, and only the walk can say what it does.
    ///
    /// The direction that matters most is the strict one: a watcher that
    /// wrongly rejects a path stops re-analyzing a file that is in the graph,
    /// and nothing anywhere says the graph has gone stale.
    fn assert_predicate_matches_walk(dir: &TmpDir, config: &Config) {
        let cancel = Arc::new(AtomicBool::new(false));
        let walked: std::collections::HashSet<PathBuf> = FileWalker::new(config)
            .walk_all(&config.root_path, &cancel)
            .unwrap()
            .into_iter()
            .collect();

        let walker = FileWalker::new(config);
        let mut admitted = 0;
        for rel in WATCHED {
            let full = dir.0.join(rel);
            assert_eq!(
                walker.would_analyze(&full),
                walked.contains(&full),
                "the watcher and the walk disagree about {rel}",
            );
            admitted += usize::from(walked.contains(&full));
        }
        // Both answers have to appear, or the two agreeing says nothing: a
        // walk that found none of the fixture — a broken root, a fixture that
        // never got written — agrees with a predicate that rejects everything.
        assert!(
            admitted > 0 && admitted < WATCHED.len(),
            "{admitted} of {} admitted",
            WATCHED.len()
        );
    }

    fn repo_of_every_kind(tag: &str) -> TmpDir {
        let dir = TmpDir::new(tag);
        dir.write("src/main.rs", "fn main() {}\n");
        dir.write("src/lib_test.rs", "fn covered() {}\n");
        dir.write("target/debug/generated.rs", "fn built() {}\n");
        dir.write(".hidden/tool.rs", "fn hidden() {}\n");
        dir.write("docs/readme.md", "# Doc\n\nProse.\n");
        dir.write("notes.txt", "not source\n");
        dir.write("spec/mezz.elv", "c.library \"Library\"\n");
        dir
    }

    #[test]
    fn the_watcher_predicate_answers_what_the_walk_would() {
        let dir = repo_of_every_kind("predicate");
        assert_predicate_matches_walk(&dir, &Config::for_path(&dir.0));
    }

    /// And it follows the config rather than a fixed list — which is the
    /// point of asking the walker instead of keeping a second table of
    /// extensions beside it. Each of these flips at least one path's answer.
    #[test]
    fn the_watcher_predicate_follows_the_scope() {
        let dir = repo_of_every_kind("predicate-scope");
        for widen in [
            |c: &mut Config| c.analysis.include_docs = true,
            |c: &mut Config| c.analysis.include_tests = true,
            |c: &mut Config| c.analysis.exclude_patterns.clear(),
        ] {
            let mut config = Config::for_path(&dir.0);
            widen(&mut config);
            assert_predicate_matches_walk(&dir, &config);
        }
    }

    /// The walk's zero-match report, which stderr would otherwise be the
    /// only way to read.
    fn walk_report(config: &Config) -> Vec<String> {
        let cancel = Arc::new(AtomicBool::new(false));
        FileWalker::new(config)
            .collect_counting(&config.root_path, &cancel, Admit::Everything)
            .unwrap()
            .1
    }

    /// The distinction the tally exists for: a pattern that is working and a
    /// pattern that is a typo produce identical graphs, so the report is the
    /// only thing that can tell them apart.
    #[test]
    fn only_the_pattern_that_matched_nothing_is_reported() {
        let dir = repo_of_every_kind("zero-match");
        let mut config = Config::for_path(&dir.0);
        config
            .analysis
            .exclude_patterns
            .push("**/main.rs".to_string());
        config
            .analysis
            .exclude_patterns
            .push("**/*.d.ts".to_string());
        assert_eq!(
            walk_report(&config),
            vec![r#"exclude_patterns: "**/*.d.ts" matched 0 files — check the spelling."#]
        );
    }

    /// Every shipped default is inert in most repos by design. Warning about
    /// them on every run in every repo is how a reader learns to skip the
    /// line that matters.
    #[test]
    fn the_shipped_defaults_are_exempt() {
        let dir = repo_of_every_kind("defaults-exempt");
        // Six of the seven match nothing here — there is no `node_modules`,
        // no `vendor`, no `dist`.
        assert_eq!(walk_report(&Config::for_path(&dir.0)), Vec::<String>::new());
    }

    /// The same mechanism, and the worse failure: an include pattern that
    /// matches nothing does not narrow the analysis, it empties it.
    #[test]
    fn an_include_pattern_that_matched_nothing_is_reported() {
        let dir = repo_of_every_kind("zero-match-include");
        let mut config = Config::for_path(&dir.0);
        // Repo-relative and still matching nothing: this fixture has no
        // `lib/`. Before CFG-013 a repo-relative include pattern matched
        // nothing *whatever* it named, which is what made this test's
        // original `src/**/*.rs` an accidental pass.
        config
            .analysis
            .include_patterns
            .push("lib/**/*.rs".to_string());
        let cancel = Arc::new(AtomicBool::new(false));
        let (files, report) = FileWalker::new(&config)
            .collect_counting(&config.root_path, &cancel, Admit::Everything)
            .unwrap();
        assert!(files.is_empty(), "{files:?}");
        assert_eq!(
            report,
            vec![r#"include_patterns: "lib/**/*.rs" matched 0 files — check the spelling."#]
        );
    }

    /// The other half, and the reason the test above had to change: an
    /// include pattern spelled the way an author would write it now selects
    /// the files it names instead of emptying the analysis (CFG-013).
    #[test]
    fn a_repo_relative_include_pattern_selects_its_files() {
        let dir = repo_of_every_kind("include-repo-relative");
        let mut config = Config::for_path(&dir.0);
        config
            .analysis
            .include_patterns
            .push("src/**/*.rs".to_string());
        let cancel = Arc::new(AtomicBool::new(false));
        let (files, report) = FileWalker::new(&config)
            .collect_counting(&config.root_path, &cancel, Admit::Everything)
            .unwrap();
        assert_eq!(
            files
                .iter()
                .filter_map(|p| p.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .collect::<Vec<_>>(),
            vec!["main.rs".to_string()],
        );
        assert_eq!(report, Vec::<String>::new());
    }

    /// A repo with a checkout at its root, so `repo_root` has something to
    /// find and the analyzed root and the repo root can differ.
    fn checkout_with_a_generated_file(tag: &str) -> TmpDir {
        let dir = TmpDir::new(tag);
        fs::create_dir_all(dir.0.join(".git")).unwrap();
        dir.write("src/main.rs", "fn main() {}\n");
        dir.write("src/contracts.d.ts", "export type Id = string;\n");
        dir
    }

    /// The bug CFG-013 is: `src/contracts.d.ts` is the spelling an author
    /// reads off their editor, and it was the one spelling that could never
    /// work. It now works, and works the same from the repo root and from
    /// inside the subdirectory it names.
    #[test]
    fn a_repo_relative_pattern_excludes_from_any_analyzed_root() {
        let dir = checkout_with_a_generated_file("repo-relative");
        for root in [dir.0.clone(), dir.0.join("src")] {
            let mut config = Config::for_path(&root);
            config
                .analysis
                .exclude_patterns
                .push("src/contracts.d.ts".to_string());
            let names = walked_names(&config);
            assert!(
                !names.contains(&"contracts.d.ts".to_string()),
                "analyzing {}: {names:?}",
                root.display()
            );
            assert!(names.contains(&"main.rs".to_string()), "{names:?}");
        }
    }

    /// The third invocation, and the two the walk cannot be asked without
    /// `set_current_dir`. All three name the same file, so all three have to
    /// hand a pattern the same string.
    #[test]
    fn every_invocation_of_one_repo_spells_a_file_the_same_way() {
        let dir = checkout_with_a_generated_file("invocations");
        let inside = Some(dir.0.clone());
        // `mezz analyze .`, whose walk carries `./src/contracts.d.ts`.
        let dot = PatternBase::rooted(Path::new("."), inside.clone());
        assert_eq!(
            dot.spell(Path::new("./src/contracts.d.ts")),
            "src/contracts.d.ts"
        );
        // `mezz analyze src`, whose walk carries `src/contracts.d.ts`.
        let sub = PatternBase::rooted(Path::new("src"), inside);
        assert_eq!(
            sub.spell(Path::new("src/contracts.d.ts")),
            "src/contracts.d.ts"
        );
        // `mezz analyze /abs/repo/src`, from a working directory that has
        // nothing to do with the repo.
        let absolute = PatternBase::rooted(&dir.0.join("src"), Some(PathBuf::from("/")));
        assert_eq!(
            absolute.spell(&dir.0.join("src/contracts.d.ts")),
            "src/contracts.d.ts"
        );
    }

    /// A path with no repo-relative spelling — the spec directory of
    /// `--spec-dir ../docs` — keeps the one the walk gave it. A base built
    /// out of `..` would match nothing anybody wrote.
    #[test]
    fn a_path_outside_the_repo_keeps_the_spelling_the_walk_gave_it() {
        let dir = checkout_with_a_generated_file("outside");
        let base = PatternBase::rooted(&dir.0, Some(PathBuf::from("/")));
        let outside = Path::new("/elsewhere/docs/domain/api.elv");
        assert_eq!(base.spell(outside), outside.to_string_lossy());
    }

    /// The regression the base change has to survive: every shipped default
    /// is `**/`-anchored and excluded these before, so no repo needs
    /// migrating and no default moves.
    #[test]
    fn the_shipped_defaults_exclude_what_they_always_excluded() {
        let dir = TmpDir::new("defaults-hold");
        fs::create_dir_all(dir.0.join(".git")).unwrap();
        dir.write("src/main.rs", "fn main() {}\n");
        dir.write("node_modules/dep/index.js", "module.exports = {};\n");
        dir.write("target/debug/build.rs", "fn built() {}\n");
        dir.write("vendor/dep/lib.py", "def vendored():\n    pass\n");
        dir.write("dist/bundle.js", "console.log(1);\n");
        dir.write("build/out.go", "package main\n");
        assert_eq!(walked_names(&Config::for_path(&dir.0)), vec!["main.rs"]);
    }

    /// Field report, 2026-08-31: a byte-identical tree analysed to 9
    /// entities at `~/mezzrepro-plain` and to **0** at `~/mezzrepro-tests`,
    /// because the heuristic read the whole absolute path. With
    /// `include_tests: false` every file in such a repo matched, the walk
    /// returned nothing, and the Stop hook emitted nothing and exited 0 —
    /// which is byte-for-byte what a clean review looks like.
    ///
    /// The directory names are the reporter's own table. `contest` is the
    /// one that shows this was never a naming convention that got slightly
    /// too eager.
    #[test]
    fn a_containing_directory_named_test_does_not_empty_the_repo() {
        for tag in ["plain", "tests-leaf", "foo-test", "testing", "contest", "prospect"] {
            let dir = TmpDir::new(tag);
            dir.write("src/main.rs", "fn main() {}\n");
            // The walk is rooted at a directory named after `tag`, which is
            // exactly what the reporter varied.
            let names = walked_names(&Config::for_path(&dir.0));
            assert!(
                names.contains(&"main.rs".to_string()),
                "a repo under a directory named `{tag}` analysed to nothing: {names:?}",
            );
        }
    }

    /// The other direction, and the half that must not move: a project's
    /// own `tests/` directory is still the project saying "these are
    /// tests", and `include_tests: false` still means it.
    #[test]
    fn the_projects_own_test_paths_are_still_excluded() {
        let dir = TmpDir::new("own-tests");
        dir.write("src/main.rs", "fn main() {}\n");
        dir.write("tests/it.rs", "fn covered() {}\n");
        dir.write("src/parser_test.rs", "fn also_covered() {}\n");
        let names = walked_names(&Config::for_path(&dir.0));
        assert_eq!(names, vec!["main.rs".to_string()], "{names:?}");
    }

    /// One file is test code or not whichever subdirectory the analysis was
    /// pointed at. Rooting at the *repo* root rather than the analyzed root
    /// is what buys that: `mezz analyze .` and `mezz analyze src` must not
    /// disagree about `src/tests/helper.rs`.
    #[test]
    fn every_root_of_one_repo_agrees_about_which_files_are_tests() {
        let dir = TmpDir::new("roots-agree");
        fs::create_dir_all(dir.0.join(".git")).unwrap();
        dir.write("src/main.rs", "fn main() {}\n");
        dir.write("src/tests/helper.rs", "fn helper() {}\n");
        for root in [dir.0.clone(), dir.0.join("src")] {
            let names = walked_names(&Config::for_path(&root));
            assert_eq!(
                names,
                vec!["main.rs".to_string()],
                "analyzing {}",
                root.display()
            );
        }
    }

    /// A path that isn't there is a typo, and a typo must not read as "this
    /// repo has no spec".
    #[test]
    fn a_missing_spec_dir_falls_back_to_the_whole_root() {
        let dir = repo_with_a_stray_spec("missing");
        let mut config = Config::for_path(&dir.0);
        config.analysis.spec_dir = Some(PathBuf::from("nowhere"));
        let names = walked_names(&config);
        assert!(names.contains(&"mezz.elv".to_string()));
        assert!(names.contains(&"toy.elv".to_string()));
    }
}
