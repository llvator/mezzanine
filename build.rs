//! Fingerprint the code that decides what a parse result contains, and hand
//! it to the crate as `MEZZ_PARSE_FINGERPRINT`.
//!
//! The parse store (`src/analyzer/parse_store.rs`) caches one `ParsedFile`
//! per source file, keyed by content hash. That is safe only while "same
//! content + same parser ⇒ same parse" holds. The parser half of that used to
//! be a hand-bumped constant, and hand-bumping failed: `c2bcb85` ("Walk
//! dotted receivers through declared field types") changed six parser files
//! without touching the salt, so every machine with a warm cache kept being
//! served the *previous* parser's output. The improvement was real in a cold
//! run and invisible in every other one — measured at 975 differing call
//! targets on this repo.
//!
//! So the generation is derived instead of remembered. Anything that can
//! change a `ParsedFile` for unchanged source goes into the hash:
//!
//!   * `src/parser/**/*.rs` — the parsers themselves.
//!   * `src/models/**/*.rs` — the types a `ParsedFile` is made of, so a field
//!     added or renamed cannot be silently filled from an older entry.
//!   * the resolved `tree-sitter*` versions in `Cargo.lock` — a grammar bump
//!     changes the tree the parsers walk without changing a line of our code.
//!   * `vendor/tree-sitter-dart/src/{parser,scanner}.c` — the one grammar
//!     that is not in `Cargo.lock` to be read, so regenerating it would
//!     otherwise leave every warm cache serving the old grammar's trees.
//!
//! It also compiles that vendored grammar, which is the other reason this
//! script exists. See `vendor/tree-sitter-dart/README.md`.
//!
//! Analyzer stages downstream of the merge are deliberately *not* hashed:
//! they run after every cache read, so their output is never stored.

use std::path::{Path, PathBuf};

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));

    compile_dart_grammar(&root);

    // Cargo re-runs this script when any of these change. Directories are
    // walked recursively, so a new parser file counts without listing it.
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/parser");
    println!("cargo:rerun-if-changed=src/models");
    println!("cargo:rerun-if-changed=Cargo.lock");

    let mut hasher = blake3::Hasher::new();
    for dir in ["src/parser", "src/models"] {
        hash_rust_sources(&root, &root.join(dir), &mut hasher);
    }
    hash_grammar_versions(&root.join("Cargo.lock"), &mut hasher);
    hash_vendored_grammar(&root, &mut hasher);

    // Twelve hex chars: this only has to separate cache generations on one
    // machine, and it ends up in a directory name a human reads.
    let fingerprint = &hasher.finalize().to_hex()[..12];
    println!("cargo:rustc-env=MEZZ_PARSE_FINGERPRINT={fingerprint}");

    println!("cargo:rustc-env=MEZZ_GIT_COMMIT={}", git_commit(&root));
}

/// Build the vendored Dart grammar into the crate.
///
/// It is C rather than a crates.io dependency because no published
/// `tree-sitter-dart` both understands Dart 3 and targets a grammar ABI our
/// `tree-sitter` can load — `vendor/tree-sitter-dart/README.md` has the
/// whole story. `src/parser/dart/mod.rs` declares the `tree_sitter_dart`
/// symbol this produces.
fn compile_dart_grammar(root: &Path) {
    let dir = root.join("vendor/tree-sitter-dart/src");
    println!("cargo:rerun-if-changed=vendor/tree-sitter-dart/src");

    cc::Build::new()
        .include(&dir)
        .file(dir.join("parser.c"))
        // The external scanner carries Dart's string and comment states,
        // which the generated tables cannot express on their own.
        .file(dir.join("scanner.c"))
        // Generated C: upstream's warnings are not ours to fix, and they
        // would drown every real one in the build log.
        .warnings(false)
        .compile("tree-sitter-dart");
}

/// Fold the vendored grammar into the fingerprint.
///
/// Every other grammar reaches the hash through its `Cargo.lock` version;
/// this one has no version to read, so its compiled sources are hashed
/// directly. `grammar.js` is deliberately excluded — it is an input to
/// generation, not to the build, so a comment-only edit there must not read
/// as a new parse generation.
fn hash_vendored_grammar(root: &Path, hasher: &mut blake3::Hasher) {
    for name in ["parser.c", "scanner.c"] {
        let path = root.join("vendor/tree-sitter-dart/src").join(name);
        match std::fs::read(&path) {
            Ok(bytes) => hasher.update(&bytes),
            // Same reasoning as `hash_rust_sources`: a fingerprint computed
            // from a partial read keys a cache generation nothing agrees with.
            Err(e) => panic!(
                "cannot read {} for the parse fingerprint: {e}",
                path.display()
            ),
        };
    }
}

/// Short commit of the build, for the version stamp `/api/hello` reports.
///
/// The crate version alone cannot answer "is the engine I'm talking to the
/// one I just built" — it stays `1.0.0` across every rebuild, which is
/// exactly when the question gets asked.
///
/// `unknown` rather than a build failure when there is no git: the public
/// release snapshot is a plain directory, and a stamp is not worth refusing
/// to compile over.
fn git_commit(root: &Path) -> String {
    // Cargo caches build-script output, so without these the stamp would
    // freeze at whatever commit was checked out the first time. `HEAD` covers
    // switching branches; `index` covers committing on the same branch, which
    // leaves `HEAD` byte-identical.
    for path in [".git/HEAD", ".git/index"] {
        if root.join(path).exists() {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(root)
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Fold every `.rs` file under `dir` into `hasher`, path-relative and in a
/// fixed order so the fingerprint is a property of the source rather than of
/// the machine or the directory-read order.
fn hash_rust_sources(root: &Path, dir: &Path, hasher: &mut blake3::Hasher) {
    let mut files = Vec::new();
    collect_rust_files(dir, &mut files);
    files.sort();
    for file in files {
        // The relative path is hashed too: moving a parser between files
        // changes behaviour even when the bytes are unchanged in aggregate.
        let relative = file.strip_prefix(root).unwrap_or(&file);
        hasher.update(relative.to_string_lossy().as_bytes());
        match std::fs::read(&file) {
            Ok(bytes) => {
                hasher.update(&bytes);
            }
            // Unreadable source is a broken checkout, not something to paper
            // over: a fingerprint computed from a partial read would key a
            // cache generation that no other build agrees with.
            Err(e) => panic!(
                "cannot read {} for the parse fingerprint: {e}",
                file.display()
            ),
        }
    }
}

fn collect_rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Fold in the resolved version of every `tree-sitter*` package.
///
/// Read from `Cargo.lock` rather than `Cargo.toml` because the manifest
/// carries ranges (`"0.21"`) and the lock carries what actually gets linked —
/// a patch bump inside the range changes the parse tree and must invalidate.
fn hash_grammar_versions(lock: &Path, hasher: &mut blake3::Hasher) {
    let Ok(text) = std::fs::read_to_string(lock) else {
        // No lockfile (a `cargo package` sanity build, say). The source hash
        // above still covers our own code; grammars fall back to the manual
        // salt. Not worth failing the build over.
        return;
    };
    let mut versions: Vec<String> = Vec::new();
    let mut name: Option<&str> = None;
    for line in text.lines() {
        let line = line.trim();
        if line == "[[package]]" {
            name = None;
        } else if let Some(value) = line.strip_prefix("name = ") {
            let value = value.trim_matches('"');
            name = value.starts_with("tree-sitter").then_some(value);
        } else if let Some(value) = line.strip_prefix("version = ") {
            if let Some(name) = name.take() {
                versions.push(format!("{name} {}", value.trim_matches('"')));
            }
        }
    }
    versions.sort();
    for entry in versions {
        hasher.update(entry.as_bytes());
    }
}
