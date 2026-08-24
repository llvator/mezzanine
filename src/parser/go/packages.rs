//! What a Go file imports, and which of those imports are the standard
//! library.
//!
//! Both questions are asked by call extraction rather than by the import
//! list itself. A Go call to anything outside its own package is spelled
//! `pkg.Func()`, where `pkg` is a *local* name — the last segment of the
//! import path, or the alias the file chose. So before a callee like
//! `u.Fetch()` means anything, the walk has to know that this file said
//! `import u "example.com/team/users"`, which makes the call
//! `users.Fetch()` and lines it up with the `qualified_name` the users
//! package's own entities carry.
//!
//! The stdlib half is the filter. `fmt.Println` is noise in a dependency
//! graph for the same reason Java's `println` is, and the honest way to
//! recognise it is not to guess from the name but to look at what the file
//! imported: an import path that is a standard-library path makes every
//! call through its local name a standard-library call. Third-party
//! imports are deliberately *not* filtered — an edge to `github.com/…`
//! resolves to nothing and becomes a ghost, which is the accurate picture.

use std::collections::HashMap;

/// The local names a file's imports introduce, mapped to the package name
/// entities are qualified by, plus whether the import is standard library.
#[derive(Debug, Default)]
pub(super) struct Imports {
    /// Local name (alias, or the path's last segment) → package name.
    by_local_name: HashMap<String, String>,
    /// Local names bound to a standard-library path.
    stdlib: std::collections::HashSet<String>,
}

impl Imports {
    /// Record one import. `alias` is the explicit name when the file gave
    /// one (`import u "…/users"`), which is also what the code writes.
    pub(super) fn insert(&mut self, path: &str, alias: Option<&str>) {
        let package = package_name(path);
        let local = alias.unwrap_or(package).to_string();
        // `import . "x"` and `import _ "x"` introduce no qualifier: the
        // first dumps the package's names into this file's scope, the
        // second imports purely for side effects. Neither can appear as
        // the operand of a selector, so neither belongs in the table.
        if local == "." || local == "_" {
            return;
        }
        if is_stdlib_path(path) {
            self.stdlib.insert(local.clone());
        }
        self.by_local_name.insert(local, package.to_string());
    }

    /// The package name behind a local qualifier, when this file imported
    /// one under that name. `None` means the operand is not an import —
    /// a variable, a field, a receiver — and the caller must not treat it
    /// as a package.
    pub(super) fn package_for(&self, local_name: &str) -> Option<&str> {
        self.by_local_name.get(local_name).map(String::as_str)
    }

    /// Whether a local qualifier names a standard-library package.
    pub(super) fn is_stdlib_qualifier(&self, local_name: &str) -> bool {
        self.stdlib.contains(local_name)
    }
}

/// The name a package is referred to by, given its import path. Go allows
/// the declared package name to differ from the final path segment, and
/// nothing in *this* file records when it does — the truth lives in the
/// imported package's own source. The last segment is right almost always
/// and wrong quietly, which is the same bet `go` tooling makes before it
/// reads the dependency.
fn package_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Whether an import path is part of the Go standard library.
///
/// Membership is by exact path against the table below rather than by a
/// shape rule. The tempting rule — "no dot in the first segment means
/// stdlib", since third-party paths start with a domain — misfires on
/// exactly the imports that matter most: `go mod init myapp` makes every
/// internal import (`myapp/internal/store`) dotless, and silently
/// filtering those would erase a project's own edges.
fn is_stdlib_path(path: &str) -> bool {
    STDLIB_PACKAGES.contains(&path)
}

/// Standard-library import paths. Not exhaustive — the standard library
/// has some 200 packages and this is the set a dependency graph actually
/// trips over. A stdlib path that is missing costs one ghost node, not a
/// wrong edge, so the list is kept to what is worth reading.
const STDLIB_PACKAGES: &[&str] = &[
    "bufio",
    "bytes",
    "compress/gzip",
    "container/heap",
    "container/list",
    "context",
    "crypto",
    "crypto/aes",
    "crypto/hmac",
    "crypto/md5",
    "crypto/rand",
    "crypto/sha1",
    "crypto/sha256",
    "crypto/tls",
    "crypto/x509",
    "database/sql",
    "embed",
    "encoding/base64",
    "encoding/binary",
    "encoding/csv",
    "encoding/hex",
    "encoding/json",
    "encoding/xml",
    "errors",
    "flag",
    "fmt",
    "hash",
    "hash/fnv",
    "html",
    "html/template",
    "io",
    "io/fs",
    "io/ioutil",
    "log",
    "log/slog",
    "maps",
    "math",
    "math/big",
    "math/bits",
    "math/rand",
    "mime",
    "mime/multipart",
    "net",
    "net/http",
    "net/http/httptest",
    "net/url",
    "os",
    "os/exec",
    "os/signal",
    "path",
    "path/filepath",
    "reflect",
    "regexp",
    "runtime",
    "runtime/debug",
    "slices",
    "sort",
    "strconv",
    "strings",
    "sync",
    "sync/atomic",
    "syscall",
    "testing",
    "text/template",
    "time",
    "unicode",
    "unicode/utf8",
    "unsafe",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_import_is_known_by_its_last_segment() {
        let mut imports = Imports::default();
        imports.insert("example.com/team/users", None);
        assert_eq!(imports.package_for("users"), Some("users"));
        assert!(!imports.is_stdlib_qualifier("users"));
    }

    #[test]
    fn an_alias_is_the_name_the_code_writes() {
        let mut imports = Imports::default();
        imports.insert("example.com/team/users", Some("u"));
        assert_eq!(imports.package_for("u"), Some("users"));
        assert_eq!(imports.package_for("users"), None);
    }

    #[test]
    fn stdlib_paths_are_recognised_under_their_local_name() {
        let mut imports = Imports::default();
        imports.insert("net/http", None);
        imports.insert("encoding/json", Some("j"));
        assert!(imports.is_stdlib_qualifier("http"));
        assert!(imports.is_stdlib_qualifier("j"));
    }

    /// The reason membership is a table and not a shape rule.
    #[test]
    fn a_dotless_module_path_is_not_the_stdlib() {
        let mut imports = Imports::default();
        imports.insert("myapp/internal/store", None);
        assert!(!imports.is_stdlib_qualifier("store"));
        assert_eq!(imports.package_for("store"), Some("store"));
    }

    #[test]
    fn dot_and_blank_imports_introduce_no_qualifier() {
        let mut imports = Imports::default();
        imports.insert("example.com/team/users", Some("."));
        imports.insert("github.com/lib/pq", Some("_"));
        assert_eq!(imports.package_for("."), None);
        assert_eq!(imports.package_for("_"), None);
    }
}
