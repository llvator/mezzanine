//! What a call does to the world, read from the name it calls (MCP-043).
//!
//! Every other view mezz offers describes the call graph and the metrics
//! over it. None of them says that a function writes to disk, opens a
//! socket, spawns a process or reads the environment — and those are where
//! regressions concentrate. An agent asked to change a function could not
//! learn from mezz that the "pure-looking" helper three hops down issues a
//! network call.
//!
//! ## The four classes
//!
//! Decided before the tables were written, and deliberately small. Each is
//! a thing a reader changes their plan for, and each can be recognised from
//! a name alone:
//!
//! | Class  | What it means |
//! |--------|---------------|
//! | `fs`   | Touches the filesystem — reads, writes, stats, lists, deletes. |
//! | `net`  | Opens a socket or issues a request over one. |
//! | `proc` | Starts, signals or exits a process. |
//! | `env`  | Reads or writes the process environment, including its arguments. |
//!
//! ## What this is not
//!
//! **Not dataflow.** Taint tracking, value propagation and
//! inter-procedural state analysis are a different engine from an
//! entity/relationship graph. What this module answers is an *effect
//! surface*: which of four classes are reachable from an entity, and
//! through which call. Async and event boundaries are excluded for the
//! same reason — they need control-flow analysis, not name classification.
//!
//! **Not a purity proof.** A name missing from these tables is a name
//! these tables do not know, which is not the same as a call that does
//! nothing. The report says "none found", never "none", and a language
//! with no table here says *that* rather than reporting zero — see
//! [`has_table`].
//!
//! ## Why the member lists are mostly explicit
//!
//! A rule with no members claims every call on its owner, which is right
//! for an owner whose name is itself proof — `subprocess`, `child_process`,
//! `reqwest`, `TcpStream`. It is wrong for a short common word. A target
//! reaches this module only when the graph could not bind it, so
//! `env.lookup(k)` on a local variable called `env` arrives as `env::lookup`
//! with exactly the shape `std::env::var` has. Owners like `env`, `os` and
//! `process` therefore name their members, and the member lists are what
//! keeps a local variable from being reported as an environment read. This
//! is the same discipline the stdlib tables next door are written to:
//! narrow, so that what is missing is visible rather than wrong.

use crate::models::file_info::Language;

/// One of the four things a call can do to the world.
///
/// Ordered as the report prints them, most-consequential first: a write to
/// disk or a socket changes what the next run sees; reading the
/// environment usually only changes what this one does.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum Effect {
    Fs,
    Net,
    Proc,
    Env,
}

impl Effect {
    /// The short tag the report groups rows under.
    pub(crate) fn tag(self) -> &'static str {
        match self {
            Effect::Fs => "fs",
            Effect::Net => "net",
            Effect::Proc => "proc",
            Effect::Env => "env",
        }
    }

    /// The sentence the tag stands for, for the one legend line that has to
    /// carry all four.
    pub(crate) fn meaning(self) -> &'static str {
        match self {
            Effect::Fs => "touches the filesystem",
            Effect::Net => "opens or uses a socket",
            Effect::Proc => "starts, signals or exits a process",
            Effect::Env => "reads or writes the process environment",
        }
    }

    /// Every class, so a caller can print the legend without knowing how
    /// many there are.
    pub(crate) fn all() -> [Effect; 4] {
        [Effect::Fs, Effect::Net, Effect::Proc, Effect::Env]
    }
}

/// One name-to-effect rule.
///
/// `owner` is the module or type the call hangs off, matched against the
/// tail of the target's own qualifier so `fs`, `std::fs` and `tokio::fs`
/// all answer alike. An `owner` of `""` matches a target that arrived with
/// no qualifier at all — Python's `open`, JavaScript's `fetch`.
///
/// `members` empty means every call on that owner carries the effect. See
/// the module header for when that is safe.
struct Rule {
    owner: &'static str,
    members: &'static [&'static str],
    effect: Effect,
}

/// Whether mezz carries an effect table for this language.
///
/// The one question the report has to ask before printing a count: a
/// language with no table must say so, because "no effects found" and "mezz
/// cannot classify effects here" are opposite facts and a zero looks like
/// the first.
pub(crate) fn has_table(language: Language) -> bool {
    !rules(language).is_empty()
}

/// The languages that have one, in reading order — so the sentence a silent
/// language prints can name what *is* covered rather than leaving the reader
/// to guess.
pub(crate) const COVERED: &str =
    "Rust, Python, TypeScript/JavaScript (and Svelte), Go, Java, Kotlin, Groovy, Dart, and C/C++";

/// What `owner::member` does to the world, or `None` when these tables do
/// not recognise it.
///
/// `owner` is the whole qualifier the target arrived with — `std::fs`, not
/// `fs` — because a rule may need the qualified form to be safe:
/// `process.env` is the Node global and a bare `env` is anybody's local.
pub(crate) fn classify(language: Language, owner: Option<&str>, member: &str) -> Option<Effect> {
    let owner = owner.unwrap_or("");
    rules(language)
        .iter()
        .find(|rule| {
            owner_matches(owner, rule.owner)
                && (rule.members.is_empty() || rule.members.contains(&member))
        })
        .map(|rule| rule.effect)
}

/// True when `owner` is `want`, or is `want` under some qualifier —
/// `std::fs` matches `fs`, and `myenv` does not match `env`.
///
/// Separator-agnostic on purpose: it is asked about `::` and `.` languages
/// alike, and the only thing it needs to know is that a segment boundary
/// sits where the suffix begins.
fn owner_matches(owner: &str, want: &str) -> bool {
    owner == want
        || owner
            .strip_suffix(want)
            .is_some_and(|head| head.ends_with(['.', ':']))
}

fn rules(language: Language) -> &'static [Rule] {
    match language {
        Language::Rust => RUST,
        Language::Python => PYTHON,
        Language::TypeScript | Language::JavaScript | Language::Svelte => JS,
        Language::Go => GO,
        // The JVM three share a table because they share the JDK; each
        // adds the names its own library spells differently.
        Language::Java | Language::Kotlin | Language::Groovy => JVM,
        Language::Dart => DART,
        Language::Cpp | Language::C => CPP,
        _ => &[],
    }
}

/// `std::fs` and `std::process` are whole modules with one effect each, so
/// they claim every member. `env` and `process` name theirs: both are
/// plausible local variables, and a target only reaches here because the
/// graph could not bind it.
const RUST: &[Rule] = &[
    Rule { owner: "fs", members: &[], effect: Effect::Fs },
    Rule { owner: "File", members: &[], effect: Effect::Fs },
    Rule { owner: "OpenOptions", members: &[], effect: Effect::Fs },
    Rule { owner: "DirBuilder", members: &[], effect: Effect::Fs },
    Rule { owner: "ReadDir", members: &[], effect: Effect::Fs },
    Rule { owner: "DirEntry", members: &[], effect: Effect::Fs },
    Rule { owner: "TempDir", members: &[], effect: Effect::Fs },
    // A `Path` is a string until something asks the filesystem about it.
    // `join`, `parent` and `extension` are pure and absent here.
    Rule {
        owner: "Path",
        members: &[
            "exists", "try_exists", "metadata", "symlink_metadata", "read_dir", "canonicalize",
            "is_file", "is_dir", "is_symlink", "read_link",
        ],
        effect: Effect::Fs,
    },
    Rule {
        owner: "PathBuf",
        members: &["exists", "try_exists", "metadata", "read_dir", "canonicalize"],
        effect: Effect::Fs,
    },
    Rule { owner: "net", members: &[], effect: Effect::Net },
    Rule { owner: "TcpStream", members: &[], effect: Effect::Net },
    Rule { owner: "TcpListener", members: &[], effect: Effect::Net },
    Rule { owner: "UdpSocket", members: &[], effect: Effect::Net },
    Rule { owner: "UnixStream", members: &[], effect: Effect::Net },
    Rule { owner: "UnixListener", members: &[], effect: Effect::Net },
    Rule { owner: "reqwest", members: &[], effect: Effect::Net },
    Rule { owner: "hyper", members: &[], effect: Effect::Net },
    Rule { owner: "ureq", members: &[], effect: Effect::Net },
    Rule { owner: "Command", members: &[], effect: Effect::Proc },
    Rule { owner: "Child", members: &[], effect: Effect::Proc },
    Rule {
        owner: "process",
        members: &["exit", "abort", "id", "Command"],
        effect: Effect::Proc,
    },
    Rule {
        owner: "env",
        members: &[
            "var", "var_os", "vars", "vars_os", "set_var", "remove_var", "args", "args_os",
            "current_dir", "set_current_dir", "temp_dir", "current_exe", "home_dir",
        ],
        effect: Effect::Env,
    },
];

/// Python's `os` is three effects in one module, so it appears three times
/// with disjoint member lists. `open` is the one bare builtin worth
/// claiming — it is the language's whole filesystem story.
const PYTHON: &[Rule] = &[
    Rule { owner: "", members: &["open"], effect: Effect::Fs },
    Rule {
        owner: "os",
        members: &[
            "remove", "unlink", "rename", "renames", "replace", "mkdir", "makedirs", "rmdir",
            "removedirs", "listdir", "scandir", "walk", "stat", "lstat", "chmod", "chown", "link",
            "symlink", "readlink", "truncate", "utime", "access", "getcwd", "chdir", "fdopen",
        ],
        effect: Effect::Fs,
    },
    Rule { owner: "shutil", members: &[], effect: Effect::Fs },
    Rule { owner: "pathlib", members: &[], effect: Effect::Fs },
    Rule { owner: "tempfile", members: &[], effect: Effect::Fs },
    Rule { owner: "glob", members: &[], effect: Effect::Fs },
    Rule {
        owner: "Path",
        members: &[
            "open", "read_text", "read_bytes", "write_text", "write_bytes", "mkdir", "rmdir",
            "unlink", "touch", "rename", "replace", "iterdir", "glob", "rglob", "exists",
            "is_file", "is_dir", "stat", "resolve", "symlink_to", "chmod",
        ],
        effect: Effect::Fs,
    },
    Rule { owner: "requests", members: &[], effect: Effect::Net },
    Rule { owner: "httpx", members: &[], effect: Effect::Net },
    Rule { owner: "aiohttp", members: &[], effect: Effect::Net },
    Rule { owner: "socket", members: &[], effect: Effect::Net },
    Rule { owner: "urllib", members: &[], effect: Effect::Net },
    Rule { owner: "smtplib", members: &[], effect: Effect::Net },
    Rule { owner: "ftplib", members: &[], effect: Effect::Net },
    Rule { owner: "websockets", members: &[], effect: Effect::Net },
    Rule { owner: "", members: &["urlopen"], effect: Effect::Net },
    Rule { owner: "subprocess", members: &[], effect: Effect::Proc },
    Rule { owner: "multiprocessing", members: &[], effect: Effect::Proc },
    Rule {
        owner: "os",
        members: &[
            "system", "popen", "fork", "execv", "execve", "execl", "execlp", "execvp", "spawnv",
            "spawnl", "kill", "waitpid", "abort", "_exit",
        ],
        effect: Effect::Proc,
    },
    Rule { owner: "sys", members: &["exit"], effect: Effect::Proc },
    Rule { owner: "os.environ", members: &[], effect: Effect::Env },
    Rule { owner: "environ", members: &[], effect: Effect::Env },
    Rule {
        owner: "os",
        members: &["getenv", "putenv", "unsetenv", "environ", "environb"],
        effect: Effect::Env,
    },
    Rule { owner: "sys", members: &["argv"], effect: Effect::Env },
];

/// `fetch` is the bare name worth claiming here, as `open` is in Python.
/// `process.env` is written qualified so a local called `env` is not
/// reported as an environment read.
const JS: &[Rule] = &[
    Rule { owner: "fs", members: &[], effect: Effect::Fs },
    Rule { owner: "fsPromises", members: &[], effect: Effect::Fs },
    Rule { owner: "fs.promises", members: &[], effect: Effect::Fs },
    Rule {
        owner: "Deno",
        members: &[
            "readTextFile", "writeTextFile", "readFile", "writeFile", "open", "create", "remove",
            "mkdir", "readDir", "stat", "lstat", "rename", "copyFile",
        ],
        effect: Effect::Fs,
    },
    Rule { owner: "", members: &["fetch"], effect: Effect::Net },
    Rule { owner: "axios", members: &[], effect: Effect::Net },
    Rule { owner: "XMLHttpRequest", members: &[], effect: Effect::Net },
    Rule { owner: "WebSocket", members: &[], effect: Effect::Net },
    Rule { owner: "EventSource", members: &[], effect: Effect::Net },
    Rule { owner: "http", members: &[], effect: Effect::Net },
    Rule { owner: "https", members: &[], effect: Effect::Net },
    Rule { owner: "net", members: &[], effect: Effect::Net },
    Rule { owner: "navigator", members: &["sendBeacon"], effect: Effect::Net },
    Rule { owner: "child_process", members: &[], effect: Effect::Proc },
    Rule { owner: "childProcess", members: &[], effect: Effect::Proc },
    Rule {
        owner: "process",
        members: &["exit", "kill", "abort", "spawn"],
        effect: Effect::Proc,
    },
    Rule { owner: "process.env", members: &[], effect: Effect::Env },
    Rule {
        owner: "process",
        members: &["env", "argv", "argv0", "execPath", "cwd", "chdir"],
        effect: Effect::Env,
    },
];

/// Go writes every out-of-package call as `pkg.Name`, so the package is
/// most of the classification. `os` is the exception that proves the rule:
/// it holds the filesystem, the environment and `Exit`, and is split three
/// ways here.
const GO: &[Rule] = &[
    Rule {
        owner: "os",
        members: &[
            "Open", "OpenFile", "Create", "CreateTemp", "ReadFile", "WriteFile", "Remove",
            "RemoveAll", "Rename", "Mkdir", "MkdirAll", "MkdirTemp", "Stat", "Lstat", "ReadDir",
            "Chmod", "Chown", "Truncate", "Symlink", "Readlink", "Link", "Getwd", "Chdir",
        ],
        effect: Effect::Fs,
    },
    Rule { owner: "ioutil", members: &[], effect: Effect::Fs },
    Rule {
        owner: "filepath",
        members: &["Walk", "WalkDir", "Glob", "EvalSymlinks"],
        effect: Effect::Fs,
    },
    Rule { owner: "File", members: &["Read", "Write", "Close", "Sync", "Seek"], effect: Effect::Fs },
    Rule { owner: "http", members: &[], effect: Effect::Net },
    Rule { owner: "net", members: &[], effect: Effect::Net },
    Rule { owner: "rpc", members: &[], effect: Effect::Net },
    Rule { owner: "smtp", members: &[], effect: Effect::Net },
    Rule { owner: "exec", members: &[], effect: Effect::Proc },
    Rule { owner: "syscall", members: &["Exec", "Kill", "ForkExec"], effect: Effect::Proc },
    Rule { owner: "os", members: &["Exit", "StartProcess", "FindProcess"], effect: Effect::Proc },
    Rule {
        owner: "os",
        members: &[
            "Getenv", "Setenv", "LookupEnv", "Unsetenv", "Clearenv", "Environ", "ExpandEnv", "Args",
        ],
        effect: Effect::Env,
    },
];

/// The JDK's own names, shared by Java, Kotlin and Groovy because the
/// classes are. Kotlin's and Groovy's filesystem sugar (`File.readText`,
/// `File.text`) lands on the same `File` owner.
const JVM: &[Rule] = &[
    Rule { owner: "Files", members: &[], effect: Effect::Fs },
    Rule { owner: "File", members: &[], effect: Effect::Fs },
    Rule { owner: "FileInputStream", members: &[], effect: Effect::Fs },
    Rule { owner: "FileOutputStream", members: &[], effect: Effect::Fs },
    Rule { owner: "FileReader", members: &[], effect: Effect::Fs },
    Rule { owner: "FileWriter", members: &[], effect: Effect::Fs },
    Rule { owner: "RandomAccessFile", members: &[], effect: Effect::Fs },
    Rule { owner: "Paths", members: &["get"], effect: Effect::Fs },
    Rule { owner: "URL", members: &["openStream", "openConnection", "getContent"], effect: Effect::Net },
    Rule { owner: "HttpClient", members: &[], effect: Effect::Net },
    Rule { owner: "HttpURLConnection", members: &[], effect: Effect::Net },
    Rule { owner: "URLConnection", members: &[], effect: Effect::Net },
    Rule { owner: "Socket", members: &[], effect: Effect::Net },
    Rule { owner: "ServerSocket", members: &[], effect: Effect::Net },
    Rule { owner: "DatagramSocket", members: &[], effect: Effect::Net },
    Rule { owner: "InetAddress", members: &[], effect: Effect::Net },
    Rule { owner: "OkHttpClient", members: &[], effect: Effect::Net },
    Rule { owner: "RestTemplate", members: &[], effect: Effect::Net },
    Rule { owner: "ProcessBuilder", members: &[], effect: Effect::Proc },
    Rule { owner: "Runtime", members: &["exec", "halt", "exit", "addShutdownHook"], effect: Effect::Proc },
    Rule { owner: "Process", members: &["waitFor", "destroy", "destroyForcibly", "exitValue"], effect: Effect::Proc },
    Rule { owner: "System", members: &["exit"], effect: Effect::Proc },
    Rule {
        owner: "System",
        members: &["getenv", "getProperty", "setProperty", "getProperties", "clearProperty"],
        effect: Effect::Env,
    },
];

/// Dart splits the filesystem and the network across `dart:io`, whose types
/// are the classification.
const DART: &[Rule] = &[
    Rule { owner: "File", members: &[], effect: Effect::Fs },
    Rule { owner: "Directory", members: &[], effect: Effect::Fs },
    Rule { owner: "Link", members: &[], effect: Effect::Fs },
    Rule { owner: "FileSystemEntity", members: &[], effect: Effect::Fs },
    Rule { owner: "RandomAccessFile", members: &[], effect: Effect::Fs },
    Rule { owner: "HttpClient", members: &[], effect: Effect::Net },
    Rule { owner: "HttpServer", members: &[], effect: Effect::Net },
    Rule { owner: "Socket", members: &[], effect: Effect::Net },
    Rule { owner: "ServerSocket", members: &[], effect: Effect::Net },
    Rule { owner: "RawDatagramSocket", members: &[], effect: Effect::Net },
    Rule { owner: "WebSocket", members: &[], effect: Effect::Net },
    Rule { owner: "http", members: &[], effect: Effect::Net },
    Rule { owner: "dio", members: &[], effect: Effect::Net },
    Rule { owner: "Process", members: &[], effect: Effect::Proc },
    Rule { owner: "Isolate", members: &["spawn", "spawnUri", "kill"], effect: Effect::Proc },
    Rule {
        owner: "Platform",
        members: &["environment", "executable", "executableArguments", "script"],
        effect: Effect::Env,
    },
];

/// C and C++ share a table: the POSIX and C-library names are the same, and
/// a `.h` in a C++ tree is parsed as either.
const CPP: &[Rule] = &[
    Rule {
        owner: "",
        members: &[
            "fopen", "fclose", "fread", "fwrite", "fprintf", "fscanf", "fgets", "fputs", "fseek",
            "remove", "rename", "mkdir", "rmdir", "opendir", "readdir", "stat", "unlink", "chmod",
        ],
        effect: Effect::Fs,
    },
    Rule { owner: "fstream", members: &[], effect: Effect::Fs },
    Rule { owner: "ifstream", members: &[], effect: Effect::Fs },
    Rule { owner: "ofstream", members: &[], effect: Effect::Fs },
    Rule { owner: "filesystem", members: &[], effect: Effect::Fs },
    Rule {
        owner: "",
        members: &[
            "socket", "connect", "bind", "listen", "accept", "send", "recv", "sendto", "recvfrom",
            "getaddrinfo", "gethostbyname",
        ],
        effect: Effect::Net,
    },
    Rule {
        owner: "",
        members: &[
            "fork", "execv", "execve", "execl", "execlp", "execvp", "system", "popen", "waitpid",
            "kill", "exit", "_exit", "abort",
        ],
        effect: Effect::Proc,
    },
    Rule {
        owner: "",
        members: &["getenv", "setenv", "unsetenv", "putenv"],
        effect: Effect::Env,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape the whole table depends on: a rule's owner matches the
    /// tail of a qualifier, at a segment boundary and nowhere else.
    #[test]
    fn an_owner_matches_under_a_qualifier_and_not_inside_a_word() {
        assert!(owner_matches("fs", "fs"));
        assert!(owner_matches("std::fs", "fs"));
        assert!(owner_matches("tokio::fs", "fs"));
        assert!(owner_matches("os.path", "os.path"));
        // The failure this exists to prevent: a local called `myenv`, or a
        // field called `self.env`, is not `std::env`.
        assert!(!owner_matches("myenv", "env"));
        assert!(!owner_matches("environment", "env"));
    }

    /// The four classes, each recognised through the form its language
    /// actually writes.
    #[test]
    fn each_class_is_recognised_in_its_own_language() {
        let rust = |q: &str, m: &str| classify(Language::Rust, Some(q), m);
        assert_eq!(rust("std::fs", "write"), Some(Effect::Fs));
        assert_eq!(rust("TcpStream", "connect"), Some(Effect::Net));
        assert_eq!(rust("Command", "spawn"), Some(Effect::Proc));
        assert_eq!(rust("std::env", "var"), Some(Effect::Env));

        assert_eq!(
            classify(Language::Go, Some("os"), "ReadFile"),
            Some(Effect::Fs)
        );
        assert_eq!(
            classify(Language::Go, Some("os"), "Getenv"),
            Some(Effect::Env)
        );
        assert_eq!(classify(Language::Go, Some("exec"), "Command"), Some(Effect::Proc));
    }

    /// The two bare names worth claiming: a language whose whole filesystem
    /// or network story is one unqualified builtin.
    #[test]
    fn a_bare_builtin_is_claimed_where_it_is_the_whole_story() {
        assert_eq!(classify(Language::Python, None, "open"), Some(Effect::Fs));
        assert_eq!(classify(Language::TypeScript, None, "fetch"), Some(Effect::Net));
        // …and only there. A bare `open` in Rust is somebody's method.
        assert_eq!(classify(Language::Rust, None, "open"), None);
    }

    /// `os` is three effects in one module, and the member lists are what
    /// tell them apart. A member in none of them is not claimed at all.
    #[test]
    fn one_module_with_several_effects_is_split_by_member() {
        let py = |m: &str| classify(Language::Python, Some("os"), m);
        assert_eq!(py("makedirs"), Some(Effect::Fs));
        assert_eq!(py("system"), Some(Effect::Proc));
        assert_eq!(py("getenv"), Some(Effect::Env));
        assert_eq!(py("cpu_count"), None);
    }

    /// The module header's central claim: a short common word only carries
    /// an effect when the member agrees, because a target reaches here
    /// precisely when the graph could not bind it.
    #[test]
    fn a_local_variable_shaped_like_a_module_is_not_claimed() {
        // `env.lookup(k)` on somebody's config struct.
        assert_eq!(classify(Language::Rust, Some("env"), "lookup"), None);
        // `process.stdout` is not the environment, and `env` alone is not
        // `process.env`.
        assert_eq!(classify(Language::TypeScript, Some("process"), "stdout"), None);
        assert_eq!(classify(Language::TypeScript, Some("env"), "get"), None);
        assert_eq!(
            classify(Language::TypeScript, Some("process.env"), "DATABASE_URL"),
            Some(Effect::Env)
        );
    }

    /// A pure path operation is not a filesystem touch. `join` builds a
    /// string; `exists` asks the disk.
    #[test]
    fn a_path_is_a_string_until_it_asks_the_disk() {
        assert_eq!(classify(Language::Rust, Some("Path"), "join"), None);
        assert_eq!(classify(Language::Rust, Some("Path"), "extension"), None);
        assert_eq!(classify(Language::Rust, Some("Path"), "exists"), Some(Effect::Fs));
        assert_eq!(classify(Language::Go, Some("filepath"), "Join"), None);
        assert_eq!(classify(Language::Go, Some("filepath"), "Walk"), Some(Effect::Fs));
    }

    /// Silence has to be distinguishable from a finding of nothing, and
    /// [`has_table`] is the only thing that can tell a report which it has.
    #[test]
    fn a_language_without_a_table_is_reported_as_one() {
        assert!(has_table(Language::Rust));
        assert!(has_table(Language::Kotlin));
        assert!(!has_table(Language::Ruby));
        assert!(!has_table(Language::Scala));
        assert_eq!(classify(Language::Ruby, Some("File"), "read"), None);
    }

    /// The JVM three share the table because they share the JDK.
    #[test]
    fn the_jvm_languages_answer_alike() {
        for language in [Language::Java, Language::Kotlin, Language::Groovy] {
            assert_eq!(
                classify(language, Some("java.nio.file.Files"), "readAllBytes"),
                Some(Effect::Fs),
                "{language:?}"
            );
            assert_eq!(
                classify(language, Some("System"), "getenv"),
                Some(Effect::Env),
                "{language:?}"
            );
        }
    }
}
