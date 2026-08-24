//! The on-disk shape of Educator content. Both content kinds — rules under
//! `content/<lang>/rules/` and lessons under `content/<lang>/lessons/` — are
//! markdown files with a `---`-delimited YAML frontmatter, sitting in
//! semantic-category subfolders. Finding them and splitting them is one
//! format, so it is one implementation, and both loaders read it from here.
//!
//! This is deliberately separate from [`super::scan`], which walks a *Java*
//! AST. The two walks share a verb and nothing else.

use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};

/// Recursively collect every `.md` file under `root`, sorted by full path so
/// the load order is stable across runs. Shared by [`super::rules::load_all`]
/// and [`super::lessons::load_all`] so semantic-category subfolders
/// (`rules/control-flow/`, `lessons/oop/`, …) work for both kinds of content
/// with identical traversal semantics.
pub(super) fn walk_markdown_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    walk_markdown_files_inner(root, &mut out)?;
    out.sort();
    Ok(out)
}

/// Split a `---`-delimited frontmatter from the body. Shared by both loaders
/// so the one wire format stays one implementation.
pub(super) fn split_frontmatter(raw: &str) -> Result<(&str, &str)> {
    let raw = raw.trim_start_matches('\u{feff}');
    let raw = raw.trim_start_matches('\n');
    let rest = raw
        .strip_prefix("---")
        .ok_or_else(|| anyhow!("file must start with `---` frontmatter delimiter"))?;
    let rest = rest.trim_start_matches('\n');
    let end = rest
        .find("\n---")
        .ok_or_else(|| anyhow!("frontmatter must end with `---` on its own line"))?;
    let frontmatter = &rest[..end];
    let after = &rest[end + 4..];
    let body = after.trim_start_matches('\n');
    Ok((frontmatter, body))
}

fn walk_markdown_files_inner(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let entries = std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))?;
    for entry in entries {
        let path = match entry {
            Ok(e) => e.path(),
            Err(_) => continue,
        };
        if path.is_dir() {
            walk_markdown_files_inner(&path, out)?;
        } else if path.extension().and_then(|s| s.to_str()) == Some("md") {
            out.push(path);
        }
    }
    Ok(())
}
