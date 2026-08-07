//! File system walker for discovering source files.

use crate::config::Config;
use crate::models::file_info::Language;
use anyhow::Result;
use glob::Pattern;
use ignore::WalkBuilder;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::Cancelled;

/// The test-file heuristic used by `include_tests: false`. Public so
/// consumers that need to *classify* files (rather than exclude them,
/// e.g. the MCP `tests_for` tool) apply the identical rule.
pub fn is_test_path(path: &Path) -> bool {
    let path_str = path.to_string_lossy().to_lowercase();
    path_str.contains("test") || path_str.contains("spec")
}

/// What a walk is looking for. The spec directory may sit outside the
/// analyzed root, and walking it must not drag that whole tree's code into
/// the graph — `nao watch services/api --spec-dir ../..` would otherwise
/// analyze the entire monorepo by accident.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Admit {
    /// Everything the filters allow. The root walk.
    Everything,
    /// Elevator files alone. The spec-directory walk.
    SpecOnly,
}

/// Walks the file system to discover source code files.
pub struct FileWalker<'a> {
    config: &'a Config,
    exclude_patterns: Vec<Pattern>,
    include_patterns: Vec<Pattern>,
    /// The resolved [`Config::spec_root`], once it has been confirmed to
    /// exist. `None` means "every `.elv` under the root is the spec" — the
    /// default, and also what a `spec_dir` pointing at nothing falls back
    /// to, having said so.
    spec_root: Option<PathBuf>,
}

impl<'a> FileWalker<'a> {
    pub fn new(config: &'a Config) -> Self {
        let exclude_patterns = config
            .analysis
            .exclude_patterns
            .iter()
            .filter_map(|p| Pattern::new(p).ok())
            .collect();

        let include_patterns = config
            .analysis
            .include_patterns
            .iter()
            .filter_map(|p| Pattern::new(p).ok())
            .collect();

        Self {
            config,
            exclude_patterns,
            include_patterns,
            spec_root: resolve_spec_root(config),
        }
    }

    /// Walk the directory tree and return all matching source files.
    ///
    /// Uses the `ignore` crate (same one ripgrep uses) so `.gitignore`,
    /// `.ignore`, the global gitignore, and hidden-file rules are honored
    /// out of the box. We then layer the nao-specific filters on top
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
        match self.spec_root.as_ref().filter(|_| self.config.spec_is_outside_root()) {
            Some(dir) => self.collect(&dir.clone(), cancel, Admit::SpecOnly),
            None => Ok(Vec::new()),
        }
    }

    /// The walk itself, shared by both entry points.
    fn collect(&self, root: &Path, cancel: &Arc<AtomicBool>, admit: Admit) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        let noun = if admit == Admit::SpecOnly { "spec" } else { "source" };

        let spinner = ProgressBar::new_spinner();
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

            // Apply nao-specific exclude patterns (additive to .gitignore).
            if self.should_skip_dir(path) { continue; }

            if path.is_file() && self.should_include_file(path) && self.admits(path, admit) {
                files.push(path.to_path_buf());
                spinner.set_message(format!("Discovered {} {noun} files...", files.len()));
            }
            spinner.tick();
        }

        spinner.finish_with_message(format!("Discovered {} {noun} files", files.len()));
        Ok(files)
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
            && self.should_include_file(path)
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
    
    /// Check if a directory should be skipped
    fn should_skip_dir(&self, path: &Path) -> bool {
        let path_str = path.to_string_lossy();
        
        // Check exclude patterns
        for pattern in &self.exclude_patterns {
            if pattern.matches(&path_str) {
                return true;
            }
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
    
    /// Check if a file should be included in analysis
    fn should_include_file(&self, path: &Path) -> bool {
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
        if !self.config.analysis.include_tests
            && language != Language::Elevator
            && language != Language::Markdown
            && is_test_path(path)
        {
            return false;
        }
        
        let path_str = path.to_string_lossy();
        
        // Check exclude patterns
        for pattern in &self.exclude_patterns {
            if pattern.matches(&path_str) {
                return false;
            }
        }
        
        // Check include patterns (if any specified, file must match at least one)
        if !self.include_patterns.is_empty() {
            let matches_any = self.include_patterns.iter().any(|p| p.matches(&path_str));
            if !matches_any {
                return false;
            }
        }
        
        true
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
    eprintln!(
        "   ⚠ spec_dir: {} is not a directory — ignoring it, and treating \
         every .elv under the root as the spec.",
        root.display()
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
                "nao-walker-{}-{}-{}",
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
        dir.write("docs/domain/nao.elv", "c.library \"Library\"\n");
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
        assert!(names.contains(&"nao.elv".to_string()));
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
        assert!(names.contains(&"nao.elv".to_string()));
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
        assert!(!names.contains(&"nao.elv".to_string()), "{names:?}");
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
        "spec/nao.elv",
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
        assert!(admitted > 0 && admitted < WATCHED.len(), "{admitted} of {} admitted", WATCHED.len());
    }

    fn repo_of_every_kind(tag: &str) -> TmpDir {
        let dir = TmpDir::new(tag);
        dir.write("src/main.rs", "fn main() {}\n");
        dir.write("src/lib_test.rs", "fn covered() {}\n");
        dir.write("target/debug/generated.rs", "fn built() {}\n");
        dir.write(".hidden/tool.rs", "fn hidden() {}\n");
        dir.write("docs/readme.md", "# Doc\n\nProse.\n");
        dir.write("notes.txt", "not source\n");
        dir.write("spec/nao.elv", "c.library \"Library\"\n");
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

    /// A path that isn't there is a typo, and a typo must not read as "this
    /// repo has no spec".
    #[test]
    fn a_missing_spec_dir_falls_back_to_the_whole_root() {
        let dir = repo_with_a_stray_spec("missing");
        let mut config = Config::for_path(&dir.0);
        config.analysis.spec_dir = Some(PathBuf::from("nowhere"));
        let names = walked_names(&config);
        assert!(names.contains(&"nao.elv".to_string()));
        assert!(names.contains(&"toy.elv".to_string()));
    }
}
