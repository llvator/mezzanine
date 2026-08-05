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

/// Walks the file system to discover source code files.
pub struct FileWalker<'a> {
    config: &'a Config,
    exclude_patterns: Vec<Pattern>,
    include_patterns: Vec<Pattern>,
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
        let mut files = Vec::new();

        let spinner = ProgressBar::new_spinner();
        spinner.set_style(
            ProgressStyle::with_template("{spinner:.cyan} {msg}")
                .unwrap()
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"),
        );
        spinner.set_message("Discovering source files...");

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

            if path.is_file() && self.should_include_file(path) {
                files.push(path.to_path_buf());
                spinner.set_message(format!("Discovered {} source files...", files.len()));
            }
            spinner.tick();
        }

        spinner.finish_with_message(format!("Discovered {} source files", files.len()));
        Ok(files)
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

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_language_detection() {
        assert_eq!(Language::from_extension("rs"), Language::Rust);
        assert_eq!(Language::from_extension("py"), Language::Python);
        assert_eq!(Language::from_extension("js"), Language::JavaScript);
        assert_eq!(Language::from_extension("ts"), Language::TypeScript);
        assert_eq!(Language::from_extension("java"), Language::Java);
        assert_eq!(Language::from_extension("go"), Language::Go);
    }
}
