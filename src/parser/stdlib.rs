//! One question any report can ask about a name mezz could not bind: is it
//! the language's own?
//!
//! Every parser already keeps a table of standard-library member names —
//! `collect`, `toString`, `push_back` — used at parse time to stop library
//! noise from becoming call edges. Those tables are the only place in the
//! tree that knows `collect` is Rust's and `putIfAbsent` is Dart's, so this
//! module dispatches to them by language rather than growing a second list
//! that could disagree with the first.
//!
//! Two kinds of knowledge the parse-time tables don't hold are added here,
//! because only a *reader* of the graph needs them. A parser filters a
//! bare member name; a report is handed a whole target — `Vec::push`,
//! `std::fs::write`, `fmt.Println` — and has to place the thing the name
//! hangs off. Hence [`is_stdlib_path`] for the namespaces a language
//! reserves and [`is_stdlib_owner`] for the types and modules it ships.
//!
//! Narrow on purpose, in the same direction as the parse-time tables: a
//! name missing from these lists is reported as third-party or unresolved,
//! which is a visible, checkable answer. A name wrongly *in* them hides a
//! real dependency behind the word "stdlib", which is not.

use crate::models::file_info::Language;

/// True when `member` — the last segment of a call target — is a name the
/// language's own library owns.
///
/// Straight through to the parser's own table, so this cannot drift from
/// what the parser filtered at parse time. Languages without a table
/// (Python's builtins live in the graph's ghost categoriser, and the rest
/// have no body-level call extraction) answer `false`.
pub(crate) fn is_stdlib_member(language: Language, member: &str) -> bool {
    match language {
        Language::Rust => super::rust::bodies::stdlib::is_stdlib_function(member),
        Language::TypeScript | Language::JavaScript | Language::Svelte => {
            super::typescript::bodies::stdlib::is_stdlib_function(member)
        }
        Language::Java => super::java::bodies::stdlib::is_stdlib_method(member),
        Language::Kotlin => super::kotlin::bodies::stdlib::is_stdlib_method(member),
        Language::Groovy => super::groovy::stdlib::is_stdlib_method(member),
        Language::Go => super::go::bodies::stdlib::is_builtin(member),
        Language::Dart => super::dart::bodies::stdlib::is_core_method(member),
        Language::Cpp | Language::C => super::cpp::bodies::stdlib::is_std_member(member),
        _ => false,
    }
}

/// True when a whole target name is rooted in a namespace the language
/// reserves for its own library — `std::fs::write`, `java.util.List.of`.
///
/// The cheapest and most certain of the three tests: the call site said so
/// itself, and no project can claim these roots.
pub(crate) fn is_stdlib_path(language: Language, qualified: &str) -> bool {
    roots(language)
        .iter()
        .any(|root| qualified.starts_with(root))
}

fn roots(language: Language) -> &'static [&'static str] {
    match language {
        Language::Rust => &["std::", "core::", "alloc::"],
        Language::Cpp | Language::C => &["std::"],
        Language::Java => &["java.", "javax.", "jdk."],
        Language::Kotlin => &["kotlin.", "java.", "javax."],
        Language::Groovy => &["groovy.", "java.", "javax."],
        Language::Scala => &["scala.", "java."],
        Language::CSharp => &["System."],
        Language::Swift => &["Swift.", "Foundation."],
        Language::Dart => &["dart:"],
        Language::Python => &["typing.", "collections.", "os.path."],
        _ => &[],
    }
}

/// True when `owner` — the module or type a call hangs off — is one the
/// language ships.
///
/// `Vec::push` and `fmt.Println` carry their answer in the *first* half,
/// which no parse-time member table can see: `push` and `Println` say
/// nothing on their own. Only the last segment is tested, so
/// `std::collections::HashMap::get` and a bare `HashMap::get` classify
/// alike.
pub(crate) fn is_stdlib_owner(language: Language, owner: &str) -> bool {
    let last = owner.rsplit("::").next().unwrap_or(owner);
    let last = last.rsplit('.').next().unwrap_or(last);
    owners(language).contains(&last)
}

fn owners(language: Language) -> &'static [&'static str] {
    match language {
        Language::Rust => RUST_OWNERS,
        Language::Go => GO_OWNERS,
        Language::Python => PYTHON_OWNERS,
        Language::TypeScript | Language::JavaScript | Language::Svelte => JS_OWNERS,
        Language::Java | Language::Kotlin | Language::Groovy | Language::Scala => JVM_OWNERS,
        Language::Cpp | Language::C => CPP_OWNERS,
        _ => &[],
    }
}

/// Rust's own modules, container types and constructors. The variants come
/// first because `Some(x)` and `Ok(v)` reach a graph as bare call targets
/// and are, by count, the commonest ghost in any Rust repo.
const RUST_OWNERS: &[&str] = &[
    // Constructors that arrive with no owner at all.
    "Some", "None", "Ok", "Err",
    // Modules.
    "std", "core", "alloc", "fs", "env", "io", "fmt", "mem", "ptr", "cmp", "iter", "process",
    "thread", "time", "sync", "net", "path", "collections", "slice", "str", "char", "num",
    "convert", "ops", "borrow", "rc", "cell", "panic", "hint",
    // Container, smart-pointer and primitive types.
    "Vec", "VecDeque", "String", "Box", "Rc", "Arc", "Cow", "Option", "Result", "HashMap",
    "HashSet", "BTreeMap", "BTreeSet", "BinaryHeap", "Path", "PathBuf", "OsStr", "OsString",
    "File", "Command", "Duration", "Instant", "SystemTime", "Mutex", "RwLock", "RefCell", "Cell",
    "Chars", "Iterator", "Default", "Ordering", "Range", "Entry", "Error", "Display", "Debug",
    "Formatter", "Write", "Read", "BufReader", "BufWriter",
];

/// Go's standard-library packages. A Go call outside its own package is
/// written `pkg.Name`, so the package name is the whole classification —
/// which is why this list is longer than the others.
const GO_OWNERS: &[&str] = &[
    "fmt", "os", "io", "ioutil", "bufio", "bytes", "strings", "strconv", "errors", "time",
    "context", "sync", "atomic", "sort", "math", "rand", "regexp", "reflect", "encoding", "json",
    "xml", "csv", "base64", "hex", "net", "http", "url", "path", "filepath", "log", "flag",
    "testing", "unicode", "utf8", "runtime", "signal", "exec", "template",
];

/// Python's standard-library modules and the builtin types a method is
/// most often called on. Python's *builtins* are classified by the graph's
/// ghost categoriser, which sees them as bare names; these are the dotted
/// receivers it cannot.
const PYTHON_OWNERS: &[&str] = &[
    "os", "sys", "re", "json", "time", "datetime", "date", "math", "random", "logging", "pathlib",
    "Path", "collections", "itertools", "functools", "typing", "subprocess", "shutil", "tempfile",
    "argparse", "unittest", "asyncio", "threading", "csv", "io", "copy", "uuid", "hashlib",
    "base64", "textwrap", "traceback", "warnings", "abc", "enum", "dataclasses", "str", "bytes",
    "list", "dict", "set", "tuple", "int", "float",
];

/// The JavaScript globals and Node built-in modules a member call hangs
/// off. The member half — `map`, `then`, `querySelector` — is the
/// TypeScript parser's table.
const JS_OWNERS: &[&str] = &[
    "console", "JSON", "Math", "Object", "Array", "String", "Number", "Boolean", "Date", "RegExp",
    "Promise", "Map", "Set", "WeakMap", "WeakSet", "Symbol", "Error", "Reflect", "Proxy",
    "globalThis", "window", "document", "process", "Buffer", "fs", "path", "util", "crypto", "os",
    "events", "stream", "url", "http", "https", "child_process",
];

/// The JDK types a receiver is most often inferred to, shared by the JVM
/// languages because the classes are.
const JVM_OWNERS: &[&str] = &[
    "String", "StringBuilder", "StringBuffer", "Integer", "Long", "Double", "Float", "Boolean",
    "Character", "Byte", "Short", "Math", "System", "Object", "Class", "Objects", "Optional",
    "List", "ArrayList", "LinkedList", "Map", "HashMap", "TreeMap", "LinkedHashMap", "Set",
    "HashSet", "TreeSet", "LinkedHashSet", "Collection", "Collections", "Arrays", "Stream",
    "Collectors", "Iterator", "Comparator", "Thread", "Exception", "RuntimeException", "Throwable",
    "File", "Files", "Paths", "Pattern", "Matcher", "LocalDate", "LocalDateTime", "Instant",
    "Duration", "BigDecimal", "BigInteger", "UUID",
];

/// The `std::` containers and streams, for the C++ call sites that name
/// the type without the namespace (`vector<T>::push_back` reduced to
/// `vector::push_back`).
const CPP_OWNERS: &[&str] = &[
    "std", "vector", "string", "map", "unordered_map", "set", "unordered_set", "deque", "list",
    "array", "pair", "tuple", "optional", "variant", "unique_ptr", "shared_ptr", "weak_ptr",
    "ostream", "istream", "stringstream", "ostringstream", "istringstream", "cout", "cerr", "cin",
    "size_t", "thread", "mutex",
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The point of dispatching rather than re-listing: the answer for a
    /// member name is the parser's own, and it is language-specific —
    /// `collect` is Rust's and `putIfAbsent` is Dart's, and neither table
    /// answers for the other.
    #[test]
    fn member_names_are_answered_by_the_parsers_own_table() {
        assert!(is_stdlib_member(Language::Rust, "collect"));
        assert!(!is_stdlib_member(Language::Dart, "collect"));
        assert!(is_stdlib_member(Language::Dart, "putIfAbsent"));
        assert!(!is_stdlib_member(Language::Rust, "putIfAbsent"));
        // A language with no body-level table answers no, rather than
        // borrowing another language's.
        assert!(!is_stdlib_member(Language::Python, "collect"));
    }

    /// A project cannot claim these roots, so the target name settles it
    /// on its own.
    #[test]
    fn a_reserved_root_settles_the_question() {
        assert!(is_stdlib_path(Language::Rust, "std::fs::write"));
        assert!(!is_stdlib_path(Language::Rust, "serde_json::to_string"));
        assert!(is_stdlib_path(Language::Java, "java.util.List"));
        assert!(!is_stdlib_path(Language::Java, "com.acme.Widget"));
    }

    /// `push` says nothing; `Vec::push` says everything. Only the last
    /// segment of the owner is read, so the qualified and the bare form of
    /// the same receiver classify alike.
    #[test]
    fn the_owner_carries_the_answer_the_member_cannot() {
        assert!(is_stdlib_owner(Language::Rust, "Vec"));
        assert!(is_stdlib_owner(Language::Rust, "std::collections::HashMap"));
        assert!(!is_stdlib_owner(Language::Rust, "Node"));
        assert!(is_stdlib_owner(Language::Go, "fmt"));
        assert!(!is_stdlib_owner(Language::Go, "chi"));
    }
}
