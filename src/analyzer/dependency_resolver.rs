//! Dependency resolution between code entities.

use crate::config::Config;
use crate::models::file_info::Language;
use crate::models::{CodeEntity, FileInfo, ImportSite, Relationship, RelationshipKind};
use crate::parser::language_parser::ImportInfo;
use anyhow::Result;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

/// Resolves dependencies between code entities.
pub struct DependencyResolver<'a> {
    config: &'a Config,
    entities: &'a HashMap<String, CodeEntity>,
    imports: &'a HashMap<PathBuf, Vec<ImportInfo>>,
    /// Lookup: qualified_name -> every entity ID sharing that
    /// qualified name. Most language parsers emit the bare class name
    /// (e.g. `"Rectangle"`) as the qualified name, so this map collides
    /// across languages the same way `name_to_id` does — `Vec<String>`
    /// plus locality filtering in `find_entity_by_name_near` is what
    /// keeps a Python `Square(Rectangle)` from binding to a Java
    /// `Rectangle`.
    qualified_to_id: HashMap<String, Vec<String>>,
    /// Lookup: simple name -> every entity ID sharing that name. See
    /// `find_entity_by_name_near` for the disambiguation rules (same
    /// file → same directory → deepest shared path → first).
    name_to_id: HashMap<String, Vec<String>>,
    /// Lookup: file path -> entity ID (for File entities only)
    file_path_to_id: HashMap<PathBuf, String>,
    /// Lookup: the path an import specifier would spell -> the analysed
    /// file it names. See [`specifier_keys`] for the spellings accepted.
    file_by_specifier: HashMap<PathBuf, PathBuf>,
}

/// Stems that make a file its directory's entry point, so `./settings`
/// names `settings/index.ts` as well as `settings.ts`. One list across
/// languages: a Rust `mod.rs` and a Python `__init__.py` are the same
/// thing, and a stem from the wrong language cannot collide with a real
/// file that is not one of these.
const ENTRY_STEMS: [&str; 3] = ["index", "mod", "__init__"];

/// Every path an import specifier could spell for one analysed file: the
/// file itself, the file without its extension, and — when the file is an
/// entry point — the directory holding it.
fn specifier_keys(path: &Path) -> Vec<PathBuf> {
    let mut keys = vec![path.to_path_buf()];
    if path.extension().is_some() {
        keys.push(path.with_extension(""));
    }
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    if ENTRY_STEMS.contains(&stem) {
        if let Some(dir) = path.parent() {
            keys.push(dir.to_path_buf());
        }
    }
    keys
}

/// One relative import specifier as the path it names.
///
/// TypeScript, JavaScript and Dart spell the relation with separators
/// (`./settings/settings`, `../vocab`) and need no translation. Python
/// spells it with dots — `.settings`, `..vocab.tokens` — where the
/// leading run is how many levels up and the rest are separators, and
/// `from . import x` names the package directory itself.
///
/// A specifier already holding a separator is already a path, which is
/// how the two spellings are told apart without asking the language.
fn as_path(specifier: &str) -> PathBuf {
    if specifier.contains('/') || !specifier.starts_with('.') {
        return PathBuf::from(specifier);
    }
    let rest = specifier.trim_start_matches('.');
    let mut out = PathBuf::new();
    // One dot is "here", so only the dots past the first go up.
    for _ in 1..(specifier.len() - rest.len()) {
        out.push("..");
    }
    for segment in rest.split('.').filter(|s| !s.is_empty()) {
        out.push(segment);
    }
    out
}

/// `./a/b/../c` → `a/c`. Purely lexical: the path may name a file this
/// analysis never walked, and asking the filesystem instead would make
/// resolution depend on what else is on the machine.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Could this fragment be a type name in *some* language?
///
/// Deliberately permissive: it accepts primitives (`str`, `int`), generics
/// (`T`), and qualified names (`typing.Optional`, `crate::model::User`). It
/// exists only to reject fragments that no language could name a type —
/// punctuation, and anything carrying leftover syntax the split didn't
/// remove.
///
/// It must not become an uppercase filter. That was tried, and it dropped
/// every Python primitive, so `def f() -> str` lost its `Returns` edge.
fn is_plausible_type_name(fragment: &str) -> bool {
    // `::` is a path separator; a lone `:` is punctuation that leaked out of
    // something like a dict-shaped annotation.
    !fragment.is_empty() && fragment.split("::").all(is_dotted_identifier)
}

/// One `::`-separated segment: an identifier, or a dot-joined run of them
/// (`typing.Optional`).
fn is_dotted_identifier(segment: &str) -> bool {
    !segment.is_empty()
        && segment.split('.').all(|part| {
            part.chars()
                .next()
                .is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')
                && part
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
        })
}

/// One `Imports` edge, carrying what the parser recorded about the
/// statement that made it.
///
/// A free function rather than three lines in `resolve_imports` because
/// every fact an import edge knows has to arrive here: the span (AN-024)
/// joined the path, then the type-only flag (AN-022). Growing one builder
/// is how those land without `resolve_imports` growing a copied field
/// apiece.
fn import_edge(source_id: String, target_id: String, import: &ImportInfo) -> Relationship {
    let mut rel = Relationship::new(source_id, target_id, RelationshipKind::Imports)
        .with_span(import.span);
    rel.label = Some(import.path.clone());
    rel.metadata.extend(import_flags(import));
    rel
}

/// The facts an import edge carries beyond its path and its span.
///
/// A table rather than a branch apiece in [`import_edge`], so the next
/// thing an import learns about itself is a row here.
///
/// `type_only` is keyed for the category, not for the keyword that spelled
/// it: the same key answers TypeScript's `import type` and Python's
/// `TYPE_CHECKING` (AN-022), and is where a Rust or Java answer would land.
fn import_flags(import: &ImportInfo) -> Vec<(String, String)> {
    let mut flags = Vec::new();
    for (key, set) in [
        ("reexport", import.is_reexport),
        ("type_only", import.is_type_only),
    ] {
        if set {
            flags.push((key.to_string(), "true".to_string()));
        }
    }
    flags
}

impl<'a> DependencyResolver<'a> {
    pub fn new(
        config: &'a Config,
        entities: &'a HashMap<String, CodeEntity>,
        imports: &'a HashMap<PathBuf, Vec<ImportInfo>>,
        files: &'a HashMap<PathBuf, FileInfo>,
    ) -> Self {
        let mut qualified_to_id: HashMap<String, Vec<String>> =
            HashMap::with_capacity(entities.len());
        let mut name_to_id: HashMap<String, Vec<String>> = HashMap::with_capacity(entities.len());
        let mut file_path_to_id = HashMap::new();

        for entity in entities.values() {
            qualified_to_id
                .entry(entity.qualified_name.clone())
                .or_default()
                .push(entity.id.clone());
            name_to_id
                .entry(entity.name.clone())
                .or_default()
                .push(entity.id.clone());
            if entity.kind == crate::models::EntityKind::File {
                file_path_to_id.insert(entity.file_path.clone(), entity.id.clone());
            }
        }

        // Candidates were pushed in HashMap iteration order, which varies
        // per process. Every "first match" / fallback in the lookups below
        // must be stable across runs (AN-002), so pin the order.
        for ids in qualified_to_id.values_mut() {
            ids.sort();
        }
        for ids in name_to_id.values_mut() {
            ids.sort();
        }

        // Sorted before insertion so a specifier two files answer to —
        // `a.ts` and `a.js` both spelled `./a` — resolves to the same one
        // on every run (AN-002).
        let mut walked: Vec<&PathBuf> = files.keys().collect();
        walked.sort();
        let mut file_by_specifier = HashMap::with_capacity(files.len() * 2);
        for path in walked {
            let normalised = lexical(path);
            for key in specifier_keys(&normalised) {
                file_by_specifier.entry(key).or_insert(normalised.clone());
            }
        }

        Self {
            config,
            entities,
            imports,
            qualified_to_id,
            name_to_id,
            file_path_to_id,
            file_by_specifier,
        }
    }

    /// Where each cross-file import was written (AN-024) — one entry per
    /// statement, not per resolved edge.
    ///
    /// Separate from [`Self::resolve`] because it answers a different
    /// question. `resolve` asks which *entity* an import names, and builds
    /// an edge only where the importing file is itself a node — which is
    /// Groovy scripts and nothing else (see [`Self::resolve_imports`]).
    /// This asks which *file* the specifier names, which is the granularity
    /// a folder drawing works in, the only one at which "where was this
    /// arrow written" has an answer, and the one every language gets.
    pub fn import_sites(&self) -> Vec<ImportSite> {
        let mut sites = Vec::new();
        for (file_path, file_imports) in self.imports {
            let from = lexical(file_path);
            for import in file_imports {
                let Some(to) = self.resolve_import_file(&from, import) else {
                    continue;
                };
                if to == from {
                    continue;
                }
                sites.push(ImportSite {
                    from: from.clone(),
                    to,
                    line: import.span.start.line,
                    is_reexport: import.is_reexport,
                    is_type_only: import.is_type_only,
                });
            }
        }
        // `imports` is a HashMap, so the walk order is per-process. Every
        // answer built from this list has to be stable across runs.
        sites.sort();
        sites.dedup();
        sites
    }

    /// The analysed file an import specifier names, or `None`.
    ///
    /// Relative specifiers only. A bare `react` or `crate::models` names a
    /// package or a module tree rather than a path, and guessing a file
    /// for it would attribute an edge to a line that does not create it —
    /// which is the defect this whole pass exists to end.
    fn resolve_import_file(&self, from: &Path, import: &ImportInfo) -> Option<PathBuf> {
        if !import.is_relative {
            return None;
        }
        let dir = from.parent().unwrap_or(Path::new(""));
        self.file_by_specifier
            .get(&lexical(&dir.join(as_path(&import.path))))
            .cloned()
    }

    /// Resolve all dependencies and return relationships
    pub fn resolve(&self) -> Result<Vec<Relationship>> {
        let mut relationships = Vec::new();

        // Resolve import-based dependencies
        relationships.extend(self.resolve_imports());

        // Resolve containment relationships
        relationships.extend(self.resolve_containment());

        // Resolve inheritance/implementation relationships
        relationships.extend(self.resolve_inheritance());

        // Resolve trait method implementations (impl methods -> trait method signatures)
        relationships.extend(self.resolve_trait_methods());

        // Resolve return type relationships
        relationships.extend(self.resolve_return_types());

        Ok(relationships)
    }

    /// Resolve dependencies from import statements.
    ///
    /// An import edge runs from the importing *file*, so it can only exist
    /// where that file is a node. Only a Groovy script carries a `File`
    /// entity today ([`crate::parser`]'s groovy module builds one so a build
    /// script's top-level statements have somewhere to attribute), and for
    /// every other language this loop finds nothing and emits nothing.
    ///
    /// It used to invent a `file:src/main.ts` source instead. That string is
    /// not a node either, so `DependencyGraph::from_analysis` dropped the
    /// edge on arrival and discarded the `false` that said so — the whole
    /// pass looked alive from the counts and was dead from the graph (AN-026).
    /// Declining here says the same thing where it can be read.
    fn resolve_imports(&self) -> Vec<Relationship> {
        let mut relationships = Vec::new();

        for (file_path, file_imports) in self.imports {
            let Some(source_id) = self.find_file_entity(file_path) else {
                continue;
            };

            for import in file_imports {
                // Try to find the target of the import
                if let Some(target_id) = self.resolve_import_target(import) {
                    relationships.push(import_edge(source_id.clone(), target_id, import));
                }
            }
        }

        relationships
    }

    /// Resolve containment relationships (parent contains child)
    fn resolve_containment(&self) -> Vec<Relationship> {
        let mut relationships = Vec::new();

        for entity in self.entities.values() {
            if let Some(parent_id) = &entity.parent_id {
                // Check if parent exists
                if self.entities.contains_key(parent_id) || self.is_type_name(parent_id) {
                    let rel = Relationship::new(
                        parent_id.clone(),
                        entity.id.clone(),
                        RelationshipKind::Contains,
                    );
                    relationships.push(rel);
                }
            }
        }

        relationships
    }

    /// Resolve inheritance and implementation relationships
    fn resolve_inheritance(&self) -> Vec<Relationship> {
        let mut relationships = Vec::new();

        for entity in self.entities.values() {
            // Handle extends (inheritance — can be multiple, e.g. Java interface extends I1, I2)
            for extends in &entity.extends {
                if let Some(base_id) = self.find_entity_by_name_near(extends, &entity.file_path) {
                    let rel =
                        Relationship::new(entity.id.clone(), base_id, RelationshipKind::Inherits);
                    relationships.push(rel);
                } else {
                    // External type
                    let rel = Relationship::new(
                        entity.id.clone(),
                        extends.clone(),
                        RelationshipKind::Inherits,
                    )
                    .with_metadata("external", "true");
                    relationships.push(rel);
                }
            }

            // Handle implements (interfaces/traits)
            for interface in &entity.implements {
                if let Some(interface_id) =
                    self.find_entity_by_name_near(interface, &entity.file_path)
                {
                    let rel = Relationship::new(
                        entity.id.clone(),
                        interface_id,
                        RelationshipKind::Implements,
                    );
                    relationships.push(rel);
                } else {
                    // External interface
                    let rel = Relationship::new(
                        entity.id.clone(),
                        interface.clone(),
                        RelationshipKind::Implements,
                    )
                    .with_metadata("external", "true");
                    relationships.push(rel);
                }
            }
        }

        relationships
    }

    /// Resolve trait method implementations: link impl methods to trait method signatures.
    /// When `impl TraitName for Struct`, each method in the impl block should link
    /// to the corresponding method signature in the trait definition.
    fn resolve_trait_methods(&self) -> Vec<Relationship> {
        let mut relationships = Vec::new();

        // Collect trait entity IDs
        let trait_ids: std::collections::HashSet<&str> = self
            .entities
            .values()
            .filter(|e| e.kind == crate::models::EntityKind::Trait)
            .map(|e| e.id.as_str())
            .collect();

        // Build a map: (trait_entity_id, method_name) -> method_signature_entity_id
        let mut trait_method_sigs: HashMap<(&str, &str), &str> = HashMap::new();
        for entity in self.entities.values() {
            if entity.kind == crate::models::EntityKind::Method {
                if let Some(ref parent_id) = entity.parent_id {
                    if trait_ids.contains(parent_id.as_str()) {
                        trait_method_sigs
                            .insert((parent_id.as_str(), entity.name.as_str()), &entity.id);
                    }
                }
            }
        }

        // For each impl method tagged trait_impl, find the matching trait method signature
        for entity in self.entities.values() {
            if !entity.tags.contains("trait_impl") {
                continue;
            }
            if !entity.kind.is_callable() {
                continue;
            }

            // The entity.implements contains the trait names it implements
            for trait_name in &entity.implements {
                // Find the trait entity by name
                if let Some(trait_id) = self.find_entity_by_name_near(trait_name, &entity.file_path)
                {
                    // Look up the trait's method signature with the same name
                    if let Some(sig_id) =
                        trait_method_sigs.get(&(trait_id.as_str(), entity.name.as_str()))
                    {
                        let rel = Relationship::new(
                            entity.id.clone(),
                            sig_id.to_string(),
                            RelationshipKind::Implements,
                        );
                        relationships.push(rel);
                    }
                }
            }
        }

        relationships
    }

    /// Resolve return type relationships (function -> return type entity).
    ///
    /// Emits a Returns edge for every declared return-type name, whether
    /// or not a user-defined entity with that name exists. Unresolved
    /// targets (builtin types like `str`/`int`, stdlib like `Path`,
    /// third-party libraries) fall through to `build()`'s ghost
    /// synthesis the same way unresolved Calls do — otherwise
    /// `def f() -> str` silently loses its return edge because `str`
    /// isn't a user class.
    fn resolve_return_types(&self) -> Vec<Relationship> {
        let mut relationships = Vec::new();

        for entity in self.entities.values() {
            if !entity.kind.is_callable() {
                continue;
            }
            if let Some(ref return_type) = entity.return_type {
                let type_names =
                    Self::extract_type_names(return_type, Language::from_path(&entity.file_path));
                for type_name in type_names {
                    // Prefer a real user entity when one exists so the
                    // edge lands on the actual class node instead of a
                    // ghost of the same name.
                    let target_id = self
                        .find_entity_by_name_near(&type_name, &entity.file_path)
                        .unwrap_or_else(|| type_name.clone());
                    if target_id == entity.id {
                        continue; // skip self-reference
                    }
                    let rel =
                        Relationship::new(entity.id.clone(), target_id, RelationshipKind::Returns);
                    relationships.push(rel);
                }
            }
        }

        relationships
    }

    /// Extract concrete type names from a declared return type.
    ///
    /// TS-004 made this language-aware. The original splitter handles the
    /// syntax it was written against — Rust and Java, where a type is
    /// identifiers separated by `<>,()` and space — and stays the default.
    /// Languages whose type grammar uses other punctuation get their own arm;
    /// without one, their syntax leaks through as entity names (`{`, `|`,
    /// `string;` were 42% of the ghosts on this repo's own `ui/` tree).
    fn extract_type_names(type_str: &str, language: Language) -> Vec<String> {
        match language {
            Language::TypeScript | Language::JavaScript | Language::Svelte => {
                Self::extract_web_type_names(type_str)
            }
            Language::Python => Self::extract_python_type_names(type_str),
            Language::Go => Self::extract_go_type_names(type_str),
            _ => Self::extract_delimited_type_names(type_str),
        }
    }

    /// Python type grammar (PY-028).
    ///
    /// `[` subscripts and PEP 604 `|` unions are structural, and neither was
    /// a delimiter the Rust/Java splitter knew — so `Optional[User]` survived
    /// the split as one token, matched nothing, and the real `User` class was
    /// never reached. `Optional[X]` and `List[X]` are the two most common
    /// return annotations in typed Python, so that dropped a large share of a
    /// Python project's `Returns` edges.
    ///
    /// Two things this deliberately does *not* do:
    ///
    /// - Split on `.`. `datetime.datetime` is one name, not two.
    /// - Filter on case. An uppercase-only rule was removed precisely
    ///   because of Python: it dropped every primitive, so `def f() -> str`
    ///   lost its edge entirely. `str`, `int` and `bool` must still resolve.
    ///
    /// Quotes come off because a forward reference (`-> "Polygon"`) arrives
    /// as a literal quoted string.
    fn extract_python_type_names(type_str: &str) -> Vec<String> {
        let mut names = Vec::new();
        for part in type_str.split(|c: char| "<>,()[]| \t".contains(c)) {
            let trimmed = part.trim().trim_matches(|c| c == '"' || c == '\'');
            // Whatever the delimiters missed is punctuation, not a name:
            // `...` from `Callable[..., int]`, a stray `*`. Unresolved
            // *names* are still supposed to become ghosts — that is how
            // stdlib and third-party types show up — so the guard is on
            // shape, not on resolvability.
            if !is_plausible_type_name(trimmed) {
                continue;
            }
            names.push(trimmed.to_string());
        }
        names
    }

    /// TypeScript / JavaScript / Svelte type grammar.
    ///
    /// Splits on every punctuation character TypeScript's type syntax uses —
    /// generics, unions, intersections, arrays, tuples, object-literal
    /// members, indexed access, optionality, function arrows — and keeps only
    /// the fragments that are plausible identifiers.
    ///
    /// That last guard is the whole fix. Unresolved names are *supposed* to
    /// become ghosts (`Promise`, `HTMLElement`, a third-party type), so the
    /// filter is on shape, not on resolvability, and emphatically not on
    /// case: `Promise<User | null>` must still reach `null`, exactly as
    /// Python's `-> str` must still reach `str`.
    fn extract_web_type_names(type_str: &str) -> Vec<String> {
        const TYPE_OPERATORS: &[&str] = &[
            "readonly", "keyof", "typeof", "infer", "extends", "in", "is", "asserts", "new",
            "import", "out", "const",
        ];
        let mut names = Vec::new();
        for part in type_str.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$')) {
            if part.is_empty() || TYPE_OPERATORS.contains(&part) {
                continue;
            }
            // A fragment starting with a digit is an array length, a numeric
            // literal type, or the tail of something we split — never a name.
            if part.starts_with(|c: char| c.is_ascii_digit()) {
                continue;
            }
            if !names.iter().any(|n| n == part) {
                names.push(part.to_string());
            }
        }
        names
    }

    /// The original Rust/Java-shaped splitter, unchanged, and still the
    /// default for every language without an arm of its own.
    /// Handles wrappers like Option<T>, Result<T, E>, Vec<T>, &T, Box<T>, etc.
    /// Go's type grammar, whose punctuation the default splitter does not
    /// know: `*` for pointers, `[]` and `[8]` for slices and arrays,
    /// `map[K]V`, `chan T`, and `...T` for a variadic.
    ///
    /// Without an arm here every one of those leaks through as part of the
    /// name — `func New() *Server` produced a ghost called `*Server`
    /// sitting beside the real `Server` it should have pointed at, and
    /// `[]Row` beside `Row`. The same failure TS-004 fixed for TypeScript,
    /// in a different alphabet.
    ///
    /// A qualified type keeps only its final segment: `store.Row` is
    /// looked up by name, and the entity it should find is named `Row`.
    fn extract_go_type_names(type_str: &str) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for part in type_str.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '.') {
            let trimmed = part.trim_matches('.');
            // Type constructors, not types. `map`, `chan` and friends name
            // no entity, and an array length is a number.
            if trimmed.is_empty()
                || matches!(trimmed, "map" | "chan" | "func" | "struct" | "interface")
                || trimmed.chars().all(|c| c.is_ascii_digit())
            {
                continue;
            }
            let name = trimmed.rsplit('.').next().unwrap_or(trimmed).to_string();
            if !names.contains(&name) {
                names.push(name);
            }
        }
        names
    }

    fn extract_delimited_type_names(type_str: &str) -> Vec<String> {
        let mut names = Vec::new();
        // Strip references and lifetimes (Rust syntax)
        let cleaned = type_str
            .replace("&'_ ", "")
            .replace("&mut ", "")
            .replace("& ", "")
            .replace("&", "");

        // Split on generic delimiters and common separators
        for part in cleaned.split(|c: char| "<>,() ".contains(c)) {
            // Strip Python forward-reference quotes: `-> "Polygon"` comes
            // through as the literal string `"Polygon"`, which otherwise
            // ends up as a fresh ghost instead of resolving to the real
            // class.
            let trimmed = part.trim().trim_matches(|c| c == '"' || c == '\'');
            if trimmed.is_empty() {
                continue;
            }
            // Skip pure keywords that are never types — these appeared in
            // the previous skip list and still belong. `Self`, `self`,
            // `mut`, `dyn`, `impl` are Rust trait-object / reference
            // modifiers, not types we want to emit edges to. `def` and
            // `var` are Groovy's dynamic-typing keywords — the *absence*
            // of a declared type (GR-015). Left unfiltered, `def` — the
            // default in idiomatic Groovy and universal in Gradle scripts
            // — accumulated inbound edges from most of a codebase onto
            // one ghost node that means nothing, distorting every
            // centrality and fan-in reading of the graph.
            //
            // The list stays keyword-specific rather than becoming a
            // blanket lowercase filter: an uppercase-only rule was
            // removed deliberately because it dropped every Python
            // primitive, and `def` being lowercase is exactly why the
            // reverse rule is no fix either.
            if matches!(
                trimmed,
                "Self" | "self" | "mut" | "dyn" | "impl" | "def" | "var"
            ) {
                continue;
            }
            // Previously we required uppercase and kept a big skip list
            // for stdlib wrappers — the combination dropped every Python
            // primitive (`str`, `int`, `bool`, …) so `def f() -> str`
            // never produced a Returns edge. Emit the name unconditionally
            // now; the graph builder's ghost-categorisation handles the
            // "is this a stdlib thing?" question, and the UI's stdlib-
            // ghost toggle controls visibility.
            names.push(trimmed.to_string());
        }
        names
    }

    /// Find the file entity for a given path (O(1) lookup)
    fn find_file_entity(&self, path: &PathBuf) -> Option<String> {
        self.file_path_to_id.get(path).cloned()
    }

    /// Try to resolve an import to an entity ID (O(1) lookups)
    fn resolve_import_target(&self, import: &ImportInfo) -> Option<String> {
        // Try qualified name first, then simple name
        if let Some(ids) = self.qualified_to_id.get(&import.path) {
            if let Some(first) = ids.first() {
                return Some(first.clone());
            }
        }
        if let Some(ids) = self.name_to_id.get(&import.path) {
            if let Some(first) = ids.first() {
                return Some(first.clone());
            }
        }

        // Try the last segment of the module path
        if let Some(module_name) = import.path.split("::").last() {
            if let Some(ids) = self.name_to_id.get(module_name) {
                if let Some(first) = ids.first() {
                    return Some(first.clone());
                }
            }
        }

        // If include_external is enabled, create an external reference
        if self.config.analysis.include_external {
            Some(format!("external:{}", import.path))
        } else {
            None
        }
    }

    /// Find an entity by name, preferring candidates near `from_file`
    /// before falling back to any match.
    ///
    /// Previously both `qualified_to_id` and `name_to_id` were
    /// `HashMap<String, String>` (first-seen wins) populated in
    /// non-deterministic HashMap iteration order. For a class name like
    /// `Rectangle` — which appears in 16 files across Python, Java,
    /// Kotlin and Rust — that routinely resolved `Square(Rectangle)` in
    /// Python to a `Rectangle` in a completely unrelated language.
    /// Both maps now keep every ID sharing a key, and this lookup picks
    /// the one whose file is closest to the caller:
    ///   1. exact same file
    ///   2. same directory
    ///   3. deepest shared path prefix with `from_file`
    ///   4. lexicographically smallest id (candidate lists are sorted at
    ///      construction, so this fallback is stable across runs)
    fn find_entity_by_name_near(&self, name: &str, from_file: &Path) -> Option<String> {
        let mut combined: Vec<&String> = Vec::new();
        if let Some(ids) = self.qualified_to_id.get(name) {
            combined.extend(ids.iter());
        }
        if let Some(ids) = self.name_to_id.get(name) {
            for id in ids {
                if !combined.iter().any(|c| *c == id) {
                    combined.push(id);
                }
            }
        }
        // AN-014: the `Rectangle` case above is only half locality. Even
        // ranked perfectly, a name with no definition in the caller's
        // language still binds to whatever else carries it — a TypeScript
        // `SessionItem` to a Rust struct. Drop those before ranking; if
        // nothing survives, the caller ghosts the name.
        let from_language = Language::from_path(from_file);
        combined.retain(|id| {
            self.entities
                .get(*id)
                .is_none_or(|e| from_language.interoperates_with(Language::from_path(&e.file_path)))
        });
        if combined.len() <= 1 {
            return combined.first().map(|s| (*s).clone());
        }

        let from_dir = from_file.parent();

        // Pass 1: same file
        for id in &combined {
            if let Some(entity) = self.entities.get(*id) {
                if entity.file_path == from_file {
                    return Some((*id).clone());
                }
            }
        }
        // Pass 2: same directory
        if let Some(dir) = from_dir {
            for id in &combined {
                if let Some(entity) = self.entities.get(*id) {
                    if entity.file_path.parent() == Some(dir) {
                        return Some((*id).clone());
                    }
                }
            }
        }
        // Pass 3: deepest shared prefix with `from_file`
        let from_components: Vec<_> = from_file.components().collect();
        let mut best: Option<(usize, &String)> = None;
        for id in &combined {
            if let Some(entity) = self.entities.get(*id) {
                let shared = entity
                    .file_path
                    .components()
                    .zip(from_components.iter())
                    .take_while(|(a, b)| a == *b)
                    .count();
                if best.map_or(true, |(d, _)| shared > d) {
                    best = Some((shared, *id));
                }
            }
        }
        if let Some((_, id)) = best {
            return Some(id.clone());
        }
        combined.first().map(|s| (*s).clone())
    }

    #[cfg(test)]
    fn find_entity_by_name(&self, name: &str) -> Option<String> {
        self.find_entity_by_name_near(name, Path::new(""))
    }

    /// Check if a string looks like a type name (for parent resolution)
    fn is_type_name(&self, name: &str) -> bool {
        // Type names typically start with uppercase
        name.chars()
            .next()
            .map(|c| c.is_uppercase())
            .unwrap_or(false)
    }
}

/// Utility to compute transitive dependencies up to a certain depth.
pub struct TransitiveDependencyResolver<'a> {
    entities: &'a HashMap<String, CodeEntity>,
    relationships: &'a [Relationship],
}

impl<'a> TransitiveDependencyResolver<'a> {
    pub fn new(
        entities: &'a HashMap<String, CodeEntity>,
        relationships: &'a [Relationship],
    ) -> Self {
        Self {
            entities,
            relationships,
        }
    }

    /// Get all dependencies up to a certain depth
    pub fn get_dependencies_at_depth(
        &self,
        entity_id: &str,
        max_depth: usize,
    ) -> HashMap<usize, Vec<String>> {
        let mut result: HashMap<usize, Vec<String>> = HashMap::new();
        let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut current_level = vec![entity_id.to_string()];

        for depth in 1..=max_depth {
            let mut next_level = Vec::new();

            for current_id in &current_level {
                if visited.contains(current_id) {
                    continue;
                }
                visited.insert(current_id.clone());

                // Find direct dependencies
                for rel in self.relationships {
                    if rel.source_id == *current_id && rel.kind.is_dependency() {
                        if !visited.contains(&rel.target_id) {
                            next_level.push(rel.target_id.clone());
                        }
                    }
                }
            }

            if !next_level.is_empty() {
                result.insert(depth, next_level.clone());
            }
            current_level = next_level;

            if current_level.is_empty() {
                break;
            }
        }

        result
    }

    /// Get all reverse dependencies (what depends on this entity)
    pub fn get_dependents_at_depth(
        &self,
        entity_id: &str,
        max_depth: usize,
    ) -> HashMap<usize, Vec<String>> {
        let mut result: HashMap<usize, Vec<String>> = HashMap::new();
        let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut current_level = vec![entity_id.to_string()];

        for depth in 1..=max_depth {
            let mut next_level = Vec::new();

            for current_id in &current_level {
                if visited.contains(current_id) {
                    continue;
                }
                visited.insert(current_id.clone());

                // Find reverse dependencies
                for rel in self.relationships {
                    if rel.target_id == *current_id && rel.kind.is_dependency() {
                        if !visited.contains(&rel.source_id) {
                            next_level.push(rel.source_id.clone());
                        }
                    }
                }
            }

            if !next_level.is_empty() {
                result.insert(depth, next_level.clone());
            }
            current_level = next_level;

            if current_level.is_empty() {
                break;
            }
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{EntityKind, Span};

    // --------------------------------------------------------------
    //  Where an edge was written (AN-024)
    // --------------------------------------------------------------

    /// A resolver's four inputs, owned, so a test can hand out the
    /// borrows `DependencyResolver::new` wants.
    struct Fixture {
        config: Config,
        entities: HashMap<String, CodeEntity>,
        imports: HashMap<PathBuf, Vec<ImportInfo>>,
        files: HashMap<PathBuf, FileInfo>,
    }

    impl Fixture {
        fn new(files: &[&str]) -> Self {
            let files = files
                .iter()
                .map(|f| {
                    let path = PathBuf::from(f);
                    let info = FileInfo {
                        path: path.clone(),
                        language: Language::from_extension(
                            path.extension().and_then(|e| e.to_str()).unwrap_or(""),
                        ),
                        size: 0,
                        line_count: 0,
                        content_hash: None,
                        documentation: None,
                    };
                    (path, info)
                })
                .collect();
            Self {
                config: Config::default(),
                entities: HashMap::new(),
                imports: HashMap::new(),
                files,
            }
        }

        /// One entity declared at `line` of `file`.
        fn declaring(mut self, file: &str, name: &str, line: usize) -> Self {
            let span = Span::from_positions(line, 0, line, 1);
            let entity = CodeEntity::new(name, EntityKind::TypeAlias, file, span);
            self.entities.insert(entity.id.clone(), entity);
            self
        }

        /// `file` is a script, so it carries a `File` entity of its own
        /// and can be the *source* of an import edge. Groovy is the one
        /// parser that emits these; the resolver only asks whether the
        /// entity is there, which is why a test can stand one up without
        /// going through a parser (AN-026).
        fn scripted(mut self, file: &str) -> Self {
            let name = Path::new(file)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("script");
            let entity = CodeEntity::new(name, EntityKind::File, file, Span::default());
            self.entities.insert(entity.id.clone(), entity);
            self
        }

        /// One import statement written at `line` of `file`.
        fn importing(mut self, file: &str, path: &str, line: usize) -> Self {
            self.push(file, import_at(path, line));
            self
        }

        /// One re-export written at `line` of `file`.
        fn reexporting(mut self, file: &str, path: &str, line: usize) -> Self {
            self.push(file, import_at(path, line).reexport());
            self
        }

        /// One `import type` written at `line` of `file`.
        fn importing_type(mut self, file: &str, path: &str, line: usize) -> Self {
            self.push(file, import_at(path, line).type_only());
            self
        }

        fn push(&mut self, file: &str, import: ImportInfo) {
            self.imports
                .entry(PathBuf::from(file))
                .or_default()
                .push(import);
        }

        fn resolver(&self) -> DependencyResolver<'_> {
            DependencyResolver::new(&self.config, &self.entities, &self.imports, &self.files)
        }
    }

    fn import_at(path: &str, line: usize) -> ImportInfo {
        let import = ImportInfo::new(path, Span::from_positions(line, 0, line, 20));
        if path.starts_with('.') {
            import.relative()
        } else {
            import
        }
    }

    /// The regression the ticket asks for: an import at the foot of one
    /// file, of a symbol declared at the head of another. Before AN-024
    /// the only line an edge could offer was the declaration's, and the
    /// two coincide often enough for that to read as attribution.
    #[test]
    fn an_import_edge_is_located_where_it_was_written_not_where_its_target_lives() {
        let fx = Fixture::new(&["src/main.rs", "src/vocab.rs"])
            .scripted("src/main.rs")
            .declaring("src/vocab.rs", "Token", 0)
            .importing("src/main.rs", "crate::vocab::Token", 40);
        let edges = fx.resolver().resolve_imports();

        let edge = edges
            .iter()
            .find(|r| r.kind == RelationshipKind::Imports)
            .expect("the import resolves to the declared type");
        assert_eq!(edge.span.map(|s| s.start.line), Some(40));
        let target = &fx.entities[&edge.target_id];
        assert_eq!(target.span.start.line, 0, "the two lines really differ");
    }

    #[test]
    fn an_import_edge_still_carries_the_path_as_its_label() {
        let fx = Fixture::new(&["src/main.rs", "src/vocab.rs"])
            .scripted("src/main.rs")
            .declaring("src/vocab.rs", "Token", 0)
            .importing("src/main.rs", "crate::vocab::Token", 3);
        let edges = fx.resolver().resolve_imports();
        assert_eq!(edges[0].label.as_deref(), Some("crate::vocab::Token"));
    }

    #[test]
    fn a_type_only_import_marks_the_edge_it_makes() {
        let fx = Fixture::new(&["src/a.ts", "src/vocab.ts"])
            .scripted("src/a.ts")
            .declaring("src/vocab.ts", "Token", 0)
            .importing_type("src/a.ts", "Token", 0);
        let edges = fx.resolver().resolve_imports();
        assert_eq!(
            edges[0].metadata.get("type_only").map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn an_ordinary_import_leaves_the_edge_unmarked() {
        let fx = Fixture::new(&["src/a.ts", "src/vocab.ts"])
            .scripted("src/a.ts")
            .declaring("src/vocab.ts", "Token", 0)
            .importing("src/a.ts", "Token", 0);
        let edges = fx.resolver().resolve_imports();
        assert!(!edges[0].metadata.contains_key("type_only"));
    }

    /// The site is the granularity a folder drawing reads, so the fact has
    /// to arrive there too and not only on the edge.
    #[test]
    fn a_type_only_import_marks_the_site_it_was_written_at() {
        let fx = Fixture::new(&["src/a.ts", "src/vocab.ts"])
            .importing_type("src/a.ts", "./vocab", 0)
            .importing("src/a.ts", "./vocab", 1);
        let marks: Vec<bool> = fx
            .resolver()
            .import_sites()
            .iter()
            .map(|s| s.is_type_only)
            .collect();
        assert_eq!(marks, vec![true, false], "one statement each, marked apart");
    }

    #[test]
    fn a_re_export_marks_the_edge_it_makes() {
        let fx = Fixture::new(&["src/a.ts", "src/b.ts"])
            .scripted("src/a.ts")
            .declaring("src/b.ts", "Token", 0)
            .reexporting("src/a.ts", "Token", 7);
        let edges = fx.resolver().resolve_imports();
        assert_eq!(edges[0].metadata.get("reexport").map(String::as_str), Some("true"));
    }

    // --------------------------------------------------------------
    //  Which files can be the source of one (AN-026)
    // --------------------------------------------------------------

    /// The same import, once from a file that is a node and once from a
    /// file that is not. The second used to produce an edge sourced at the
    /// invented string `file:src/a.ts`, which `add_relationship` refused
    /// and threw away without a word. The two fixtures differ in one call.
    #[test]
    fn only_a_file_that_is_a_node_can_be_the_source_of_an_import_edge() {
        let files = ["src/a.ts", "src/vocab.ts"];
        let importing = |fx: Fixture| {
            fx.declaring("src/vocab.ts", "Token", 0)
                .importing("src/a.ts", "Token", 0)
        };

        let with_node = importing(Fixture::new(&files).scripted("src/a.ts"));
        let sources: Vec<String> = with_node
            .resolver()
            .resolve_imports()
            .iter()
            .map(|r| r.source_id.clone())
            .collect();
        assert_eq!(
            sources,
            vec!["src/a.ts:0:a".to_string()],
            "sourced at the file's own node"
        );

        let without_node = importing(Fixture::new(&files));
        assert!(
            without_node.resolver().resolve_imports().is_empty(),
            "no node to hang it on, so no edge — not a `file:…` one either"
        );
    }

    /// Declining the edge must not cost the site, which is what every
    /// language actually reads. `src/a.ts` has no `File` entity here.
    #[test]
    fn a_file_that_is_not_a_node_still_reports_where_it_imported_from() {
        let fx = Fixture::new(&["src/a.ts", "src/vocab.ts"]).importing("src/a.ts", "./vocab", 4);
        assert_eq!(
            sites(&fx),
            vec![("src/a.ts".into(), "src/vocab.ts".into(), 4, false)]
        );
    }

    // --------------------------------------------------------------
    //  Import sites — the file-granular half
    // --------------------------------------------------------------

    fn sites(fx: &Fixture) -> Vec<(String, String, usize, bool)> {
        fx.resolver()
            .import_sites()
            .into_iter()
            .map(|s| {
                (
                    s.from.display().to_string(),
                    s.to.display().to_string(),
                    s.line,
                    s.is_reexport,
                )
            })
            .collect()
    }

    #[test]
    fn a_relative_specifier_resolves_to_the_file_it_names() {
        let fx = Fixture::new(&["src/main.ts", "src/settings/settings.ts"])
            .importing("src/main.ts", "./settings/settings", 0);
        assert_eq!(
            sites(&fx),
            vec![(
                "src/main.ts".to_string(),
                "src/settings/settings.ts".to_string(),
                0,
                false
            )]
        );
    }

    #[test]
    fn a_specifier_naming_a_directory_resolves_to_its_entry_point() {
        for entry in ["index.ts", "mod.rs", "__init__.py"] {
            let target = format!("src/settings/{entry}");
            let fx = Fixture::new(&["src/main.ts", &target])
                .importing("src/main.ts", "./settings", 2);
            assert_eq!(sites(&fx).first().map(|s| s.1.clone()), Some(target));
        }
    }

    #[test]
    fn a_specifier_walking_back_up_resolves_the_same_way() {
        let fx = Fixture::new(&["src/deep/inner/main.ts", "src/vocab.ts"])
            .importing("src/deep/inner/main.ts", "../../vocab", 1);
        assert_eq!(sites(&fx).first().map(|s| s.1.clone()), Some("src/vocab.ts".to_string()));
    }

    #[test]
    fn a_python_dotted_specifier_names_the_same_file_a_slashed_one_would() {
        let fx = Fixture::new(&["pkg/main.py", "pkg/settings.py"])
            .importing("pkg/main.py", ".settings", 0);
        assert_eq!(
            sites(&fx).first().map(|s| s.1.clone()),
            Some("pkg/settings.py".to_string())
        );
    }

    #[test]
    fn python_leading_dots_past_the_first_walk_up() {
        let fx = Fixture::new(&["pkg/deep/main.py", "pkg/vocab/tokens.py"])
            .importing("pkg/deep/main.py", "..vocab.tokens", 0);
        assert_eq!(
            sites(&fx).first().map(|s| s.1.clone()),
            Some("pkg/vocab/tokens.py".to_string())
        );
    }

    /// `from . import x` names the package itself, which is its
    /// `__init__.py`.
    #[test]
    fn a_bare_dot_names_the_package_it_sits_in() {
        let fx = Fixture::new(&["pkg/main.py", "pkg/__init__.py"])
            .importing("pkg/main.py", ".", 0);
        assert_eq!(
            sites(&fx).first().map(|s| s.1.clone()),
            Some("pkg/__init__.py".to_string())
        );
    }

    /// A package name is not a path. Guessing a file for `react` would
    /// attribute an edge to a line that does not create it, which is the
    /// defect the whole pass exists to end.
    #[test]
    fn a_package_specifier_resolves_to_no_site() {
        let fx = Fixture::new(&["src/main.ts", "src/react.ts"])
            .importing("src/main.ts", "react", 0);
        assert!(sites(&fx).is_empty());
    }

    #[test]
    fn a_re_export_site_says_so() {
        let fx = Fixture::new(&["src/settings/settings.ts", "src/settings/defaults.ts"])
            .reexporting("src/settings/settings.ts", "./defaults", 10);
        assert_eq!(
            sites(&fx),
            vec![(
                "src/settings/settings.ts".to_string(),
                "src/settings/defaults.ts".to_string(),
                10,
                true
            )]
        );
    }

    /// Three imports of one module are three places to change, so the
    /// list keeps all three rather than collapsing to the pair.
    #[test]
    fn every_statement_gets_its_own_site() {
        let fx = Fixture::new(&["src/main.ts", "src/vocab.ts"])
            .importing("src/main.ts", "./vocab", 0)
            .importing("src/main.ts", "./vocab", 4)
            .importing("src/main.ts", "./vocab", 9);
        assert_eq!(
            sites(&fx).iter().map(|s| s.2).collect::<Vec<_>>(),
            vec![0, 4, 9]
        );
    }

    #[test]
    fn a_file_importing_itself_is_not_a_cross_file_site() {
        let fx = Fixture::new(&["src/main.ts"]).importing("src/main.ts", "./main", 0);
        assert!(sites(&fx).is_empty());
    }

    /// `imports` is a HashMap, so the walk order is per-process (AN-002).
    #[test]
    fn sites_come_back_in_the_same_order_every_run() {
        let fx = Fixture::new(&["src/a.ts", "src/b.ts", "src/c.ts"])
            .importing("src/a.ts", "./b", 1)
            .importing("src/b.ts", "./c", 2)
            .importing("src/c.ts", "./a", 3);
        let first = fx.resolver().import_sites();
        for _ in 0..8 {
            assert_eq!(fx.resolver().import_sites(), first);
        }
    }


    /// TS-004: TypeScript's type grammar used to leak through the Rust/Java
    /// splitter as entity names — `{`, `}`, `|`, `string;` were 42% of the
    /// ghosts on this repo's own `ui/` tree.
    fn ts(type_str: &str) -> Vec<String> {
        DependencyResolver::extract_type_names(type_str, Language::TypeScript)
    }

    fn rust(type_str: &str) -> Vec<String> {
        DependencyResolver::extract_type_names(type_str, Language::Rust)
    }

    fn go(type_str: &str) -> Vec<String> {
        DependencyResolver::extract_type_names(type_str, Language::Go)
    }

    /// `func New() *Server` used to produce a ghost called `*Server`
    /// sitting beside the real `Server` it should have pointed at.
    #[test]
    fn go_pointers_and_slices_are_not_part_of_the_name() {
        assert_eq!(go("*Server"), vec!["Server"]);
        assert_eq!(go("[]Row"), vec!["Row"]);
        assert_eq!(go("[8]byte"), vec!["byte"]);
        assert_eq!(go("...string"), vec!["string"]);
    }

    #[test]
    fn a_go_multi_value_return_reaches_every_value() {
        assert_eq!(go("(*Row, error)"), vec!["Row", "error"]);
    }

    #[test]
    fn go_type_constructors_name_no_entity() {
        assert_eq!(go("map[string]*Row"), vec!["string", "Row"]);
        assert_eq!(go("chan Job"), vec!["Job"]);
    }

    /// A qualified type is looked up by name, and the entity it should
    /// find is named for its last segment.
    #[test]
    fn a_qualified_go_type_is_found_by_its_last_segment() {
        assert_eq!(go("*store.Repository"), vec!["Repository"]);
    }

    #[test]
    fn a_union_inside_a_wrapper_reaches_both_arms() {
        assert_eq!(ts("Promise<User | null>"), vec!["Promise", "User", "null"]);
    }

    #[test]
    fn an_array_type_reaches_its_element() {
        assert_eq!(ts("Task[]"), vec!["Task"]);
    }

    #[test]
    fn an_object_literal_type_yields_only_its_members() {
        // "nothing, or the members" — never `{`, `}` or `string;`.
        let names = ts("{ id: string; n: number }");
        assert_eq!(names, vec!["id", "string", "n", "number"]);
    }

    #[test]
    fn no_punctuation_fragment_survives() {
        for type_str in [
            "Promise<User | null>",
            "{ id: string; n: number }",
            "Task[]",
            "ScopeRow['tiers']",
            "(a: number) => void",
            "Record<string, number[]>",
            "A & B",
            "Foo?",
        ] {
            for name in ts(type_str) {
                assert!(
                    name.chars()
                        .next()
                        .is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')
                        && name
                            .chars()
                            .all(|c| c.is_alphanumeric() || c == '_' || c == '$'),
                    "`{name}` from `{type_str}` is not an identifier"
                );
            }
        }
    }

    #[test]
    fn an_indexed_access_reaches_the_indexed_type() {
        assert_eq!(ts("ScopeRow['tiers']"), vec!["ScopeRow", "tiers"]);
    }

    #[test]
    fn type_operators_are_not_types() {
        assert_eq!(ts("readonly Task[]"), vec!["Task"]);
        assert_eq!(ts("keyof Config"), vec!["Config"]);
    }

    #[test]
    fn primitives_are_kept_the_way_pythons_are() {
        // Do not reinstate an uppercase-only filter: it dropped every
        // Python primitive, so `def f() -> str` lost its Returns edge.
        assert_eq!(ts("string"), vec!["string"]);
        assert_eq!(ts("void"), vec!["void"]);
    }

    #[test]
    fn names_are_deduplicated_within_one_type() {
        assert_eq!(ts("Map<Key, Key>"), vec!["Map", "Key"]);
    }

    #[test]
    fn rust_and_java_keep_the_original_splitter() {
        assert_eq!(
            rust("Result<Vec<Widget>, Error>"),
            vec!["Result", "Vec", "Widget", "Error"]
        );
        assert_eq!(rust("&mut Config"), vec!["Config"]);
        // `Self`/`mut`/`dyn`/`impl` stay filtered on the original path.
        assert!(rust("impl Iterator").iter().all(|n| n != "impl"));
        assert_eq!(rust("Self"), Vec::<String>::new());
        assert_eq!(rust("dyn Renderer"), vec!["Renderer"]);
    }

    fn groovy(type_str: &str) -> Vec<String> {
        DependencyResolver::extract_type_names(type_str, Language::Groovy)
    }

    /// GR-015: `def` is Groovy's dynamic-type keyword — the absence of a
    /// type, not a type named "def". It used to become one ghost node that
    /// most of a Groovy codebase pointed at.
    #[test]
    fn groovy_dynamic_keywords_are_not_types() {
        assert_eq!(groovy("def"), Vec::<String>::new());
        assert_eq!(groovy("var"), Vec::<String>::new());
    }

    #[test]
    fn a_declared_groovy_type_still_produces_its_name() {
        assert_eq!(groovy("Release"), vec!["Release"]);
        assert_eq!(groovy("List<Artifact>"), vec!["List", "Artifact"]);
    }

    fn python(type_str: &str) -> Vec<String> {
        DependencyResolver::extract_type_names(type_str, Language::Python)
    }

    /// PY-028's acceptance table. Python's `[]` subscript is structural, so
    /// the inner type has to survive as its own name — otherwise the whole
    /// annotation is one ghost and the real class is never reached.
    #[test]
    fn python_subscripted_generics_split_to_their_inner_types() {
        assert_eq!(python("Optional[User]"), vec!["Optional", "User"]);
        assert_eq!(python("List[User]"), vec!["List", "User"]);
        assert_eq!(python("Dict[str, Any]"), vec!["Dict", "str", "Any"]);
        assert_eq!(
            python("List[Dict[str, Any]]"),
            vec!["List", "Dict", "str", "Any"]
        );
    }

    /// PEP 604 unions are 3.10+ syntax that hits the same `|` problem
    /// TS-004 describes.
    #[test]
    fn python_pep604_unions_split_on_the_pipe() {
        assert_eq!(python("User | None"), vec!["User", "None"]);
        assert_eq!(
            python("Optional[User] | list[Order]"),
            vec!["Optional", "User", "list", "Order"]
        );
    }

    /// The trap this function was written to avoid: an uppercase-only filter
    /// dropped every Python primitive, so `def f() -> str` lost its edge.
    #[test]
    fn python_primitives_still_produce_a_name() {
        for primitive in ["str", "int", "bool", "float", "bytes"] {
            assert_eq!(python(primitive), vec![primitive]);
        }
    }

    #[test]
    fn python_forward_references_keep_resolving() {
        assert_eq!(python("\"Polygon\""), vec!["Polygon"]);
        assert_eq!(python("Optional[\"User\"]"), vec!["Optional", "User"]);
    }

    /// A dotted name is one name — splitting it would point the edge at a
    /// module instead of a type.
    #[test]
    fn python_dotted_names_stay_whole() {
        assert_eq!(python("datetime.datetime"), vec!["datetime.datetime"]);
    }

    /// No fragment that isn't a possible identifier may become a name.
    #[test]
    fn python_punctuation_never_becomes_a_name() {
        for type_str in [
            "Optional[User]",
            "Dict[str, Any]",
            "User | None",
            "Callable[..., int]",
            "tuple[int, ...]",
        ] {
            for name in python(type_str) {
                assert!(
                    is_plausible_type_name(&name),
                    "`{name}` from `{type_str}` is not an identifier"
                );
            }
        }
    }
}
