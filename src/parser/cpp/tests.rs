//! Integration tests for the C++ parser.
//!
//! One test per criterion of `docs/agents/parser-completeness.md` that C++
//! meets, written against `ParseResult` rather than against internals, so
//! a refactor of the walk cannot quietly change the output.
//!
//! Everything here goes through the full `parse` entry point. What breaks
//! first in this parser is a seam — a declarator that unwraps to the wrong
//! leaf, a receiver reaching call resolution before the type index has
//! seen the class — and neither misbehaves anywhere but end to end.

use super::CppParser;
use crate::models::file_info::Language;
use crate::models::{CodeEntity, EntityKind, RelationshipKind, Visibility};
use crate::parser::language_parser::{ImportCondition, LanguageParser, ParseResult};
use std::path::Path;

fn parse(src: &str) -> ParseResult {
    CppParser::new()
        .parse(Path::new("order.cpp"), src)
        .expect("parse")
}

fn entity<'a>(result: &'a ParseResult, name: &str) -> &'a CodeEntity {
    result
        .entities
        .iter()
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("entity `{name}` not found"))
}

fn names_of_kind(result: &ParseResult, kind: EntityKind) -> Vec<String> {
    let mut names: Vec<String> = result
        .entities
        .iter()
        .filter(|e| e.kind == kind)
        .map(|e| e.name.clone())
        .collect();
    names.sort();
    names
}

/// Sorted, de-duplicated targets of one relationship kind out of `source`.
fn targets(result: &ParseResult, source: &str, kind: RelationshipKind) -> Vec<String> {
    let ids: Vec<&str> = result
        .entities
        .iter()
        .filter(|e| e.name == source)
        .map(|e| e.id.as_str())
        .collect();
    assert!(!ids.is_empty(), "entity `{source}` not found");
    let mut found: Vec<String> = result
        .relationships
        .iter()
        .filter(|r| r.kind == kind && ids.contains(&r.source_id.as_str()))
        .map(|r| r.target_id.clone())
        .collect();
    found.sort();
    found.dedup();
    found
}

fn calls(result: &ParseResult, source: &str) -> Vec<String> {
    targets(result, source, RelationshipKind::Calls)
}

fn tags(result: &ParseResult, name: &str) -> Vec<String> {
    entity(result, name).tags.iter().cloned().collect()
}

// --- A. What the parser says about the file --------------------------------

/// §A1. The parser claims the extensions `Language::Cpp` lists, and the C
/// one claims C's — one grammar, two languages, each answering for itself.
#[test]
fn claims_the_extensions_its_language_lists() {
    let cpp = CppParser::new();
    assert_eq!(cpp.language(), Language::Cpp);
    assert!(cpp.can_parse(Path::new("a/order.cpp")));
    assert!(cpp.can_parse(Path::new("a/order.hpp")));
    assert!(!cpp.can_parse(Path::new("a/order.rs")));

    let c = CppParser::for_language(Language::C);
    assert_eq!(c.language(), Language::C);
    assert!(c.can_parse(Path::new("a/order.h")));
}

/// §A2. An include says whether it is relative, a using-declaration says
/// which name it binds, a using-directive says it took the lot, and an
/// alias says what it renamed.
#[test]
fn imports_carry_every_fact_they_can() {
    let result = parse(
        r#"
#include <vector>
#include "store/repository.h"
using namespace std;
using app::Order;
namespace fs = std::filesystem;
"#,
    );
    let by_path = |path: &str| {
        result
            .imports
            .iter()
            .find(|i| i.path == path)
            .unwrap_or_else(|| panic!("import `{path}` not found"))
    };
    assert!(!by_path("vector").is_relative);
    assert!(by_path("store/repository.h").is_relative);
    assert!(by_path("std").is_wildcard);
    assert_eq!(by_path("app::Order").items, vec!["Order".to_string()]);
    assert_eq!(
        by_path("std::filesystem").alias.as_deref(),
        Some("fs")
    );
}

/// §A2, the conditional half. An include behind a feature gate is a
/// weaker claim than an unconditional one — and a header guard, which
/// wraps every declaration in the file, is not a gate.
#[test]
fn an_include_behind_a_feature_gate_is_conditional() {
    let result = parse(
        r#"
#ifndef ORDER_H
#define ORDER_H
#include "always.h"
#ifdef WITH_METRICS
#include "metrics.h"
#endif
#endif
"#,
    );
    let condition = |path: &str| {
        result
            .imports
            .iter()
            .find(|i| i.path == path)
            .and_then(|i| i.condition)
    };
    assert_eq!(condition("always.h"), None);
    assert_eq!(condition("metrics.h"), Some(ImportCondition::Guarded));
}

/// §A3. The header comment belongs to the file, not to whatever happens
/// to be declared first.
#[test]
fn the_file_header_comment_travels_as_file_documentation() {
    let result = parse(
        r#"// Orders, and the repository they save through.
//
// Everything in the order domain.

void run();
"#,
    );
    let doc = result.file_documentation.clone().expect("file documentation");
    assert!(doc.starts_with("Orders, and the repository"));
    assert!(entity(&result, "run").documentation.is_none());
}

/// §A3, the other way. A comment touching the first declaration documents
/// that declaration, and claiming it for the file too would put one
/// comment in two places.
#[test]
fn a_comment_touching_the_first_declaration_is_not_the_files() {
    let result = parse(
        r#"// Runs the thing.
void run();
"#,
    );
    assert_eq!(result.file_documentation, None);
    assert_eq!(
        entity(&result, "run").documentation.as_deref(),
        Some("Runs the thing.")
    );
}

/// §A4. tree-sitter always returns a tree; the walk's job is to get
/// through it without unwrapping something an error node made absent.
/// Partial output beats no output, so what the parser recovered is kept.
#[test]
fn a_file_that_does_not_parse_produces_a_result() {
    let result = CppParser::new()
        .parse(
            Path::new("order.cpp"),
            "void before() { keep(); }\n??? %%% ;;;\nvoid after() { alsoKeep(); }",
        )
        .expect("a broken file still parses");
    assert_eq!(calls(&result, "before"), vec!["keep"]);
    assert_eq!(calls(&result, "after"), vec!["alsoKeep"]);
}

// --- B. What the file declares ---------------------------------------------

/// §B1. Every declaration kind C++ has reaches an `EntityKind`.
#[test]
fn every_declaration_kind_maps_to_an_entity_kind() {
    let result = parse(
        r#"
namespace app {
struct Point { int x; };
class Order { public: void save(); };
class Shape { public: virtual double area() const = 0; };
union Value { int i; double d; };
enum class Color { Red };
using Id = long;
typedef unsigned int Uint;
int freeFunction(int a) { return a; }
const int LIMIT = 3;
int counter = 0;
}
#define MAX_ITEMS 100
"#,
    );
    assert_eq!(names_of_kind(&result, EntityKind::Module), vec!["app"]);
    assert_eq!(names_of_kind(&result, EntityKind::Class), vec!["Order"]);
    assert_eq!(
        names_of_kind(&result, EntityKind::AbstractClass),
        vec!["Shape"]
    );
    assert_eq!(names_of_kind(&result, EntityKind::Struct), vec!["Point", "Value"]);
    assert_eq!(names_of_kind(&result, EntityKind::Enum), vec!["Color"]);
    assert_eq!(names_of_kind(&result, EntityKind::TypeAlias), vec!["Id", "Uint"]);
    assert_eq!(
        names_of_kind(&result, EntityKind::Function),
        vec!["freeFunction"]
    );
    assert_eq!(names_of_kind(&result, EntityKind::Constant), vec!["LIMIT"]);
    assert_eq!(names_of_kind(&result, EntityKind::Variable), vec!["counter"]);
    assert_eq!(names_of_kind(&result, EntityKind::Macro), vec!["MAX_ITEMS"]);
}

/// §B2. Members hang off their container, and a namespace is a container
/// like any other.
#[test]
fn members_are_parented_not_flattened() {
    let result = parse(
        r#"
namespace app {
class Order {
public:
    void save();
    int id_;
};
}
"#,
    );
    let namespace = entity(&result, "app");
    let class = entity(&result, "Order");
    assert_eq!(class.parent_id.as_deref(), Some(namespace.id.as_str()));
    assert_eq!(
        entity(&result, "save").parent_id.as_deref(),
        Some(class.id.as_str())
    );
    assert_eq!(
        entity(&result, "id_").parent_id.as_deref(),
        Some(class.id.as_str())
    );
    assert_eq!(class.qualified_name, "app::Order");
    assert_eq!(entity(&result, "save").qualified_name, "app::Order::save");
}

/// §B2, the out-of-line half. `double Order::total()` is a sibling of the
/// class, and the declarator is what binds the two.
#[test]
fn an_out_of_line_definition_attaches_to_its_class() {
    let result = parse(
        r#"
namespace app {
class Order { public: double total() const; };
double Order::total() const { return 0; }
}
"#,
    );
    let class = entity(&result, "Order");
    let definitions: Vec<&CodeEntity> = result
        .entities
        .iter()
        .filter(|e| e.name == "total")
        .collect();
    assert_eq!(definitions.len(), 2, "declaration and definition both count");
    for definition in definitions {
        assert_eq!(definition.parent_id.as_deref(), Some(class.id.as_str()));
        assert_eq!(definition.qualified_name, "app::Order::total");
    }
}

/// §B2 again, for the case a `.cpp` actually presents: the class is
/// declared in a header this parse never saw, so the parent is the bare
/// type name — which `graph.rs` accepts and registers `Order::total` from.
#[test]
fn a_definition_whose_class_is_elsewhere_parents_by_name() {
    let result = parse("double Order::total() const { return 0; }");
    assert_eq!(entity(&result, "total").parent_id.as_deref(), Some("Order"));
}

/// §B3. Parameters, return type, generics, visibility, documentation.
#[test]
fn signature_detail_is_filled() {
    let result = parse(
        r#"
template <typename T, int N>
class Cache {
private:
    /// Look one up.
    T* find(const std::string& key, int limit = 10) const;
};
"#,
    );
    let cache = entity(&result, "Cache");
    assert_eq!(cache.generics, vec!["typename T", "int N"]);
    let find = entity(&result, "find");
    assert_eq!(find.visibility, Visibility::Private);
    assert_eq!(find.return_type.as_deref(), Some("T*"));
    assert_eq!(find.documentation.as_deref(), Some("Look one up."));
    let names: Vec<&str> = find.parameters.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["key", "limit"]);
    assert_eq!(
        find.parameters[0].type_name.as_deref(),
        Some("const std::string&")
    );
    assert_eq!(find.parameters[1].default_value.as_deref(), Some("10"));
}

/// §B4. What a type holds, and what it derives from. Every base lands in
/// `extends` — C++ draws no line between inheriting an implementation and
/// inheriting a contract.
#[test]
fn type_structure_is_filled() {
    let result = parse(
        r#"
class Order : public Entity, private Serializable {
public:
    int id_;
private:
    double total_;
};
enum class Color { Red, Green = 2 };
"#,
    );
    let order = entity(&result, "Order");
    assert_eq!(order.extends, vec!["Entity", "Serializable"]);
    let fields: Vec<(&str, Option<Visibility>)> = order
        .fields
        .iter()
        .map(|f| (f.name.as_str(), f.visibility))
        .collect();
    assert_eq!(
        fields,
        vec![
            ("id_", Some(Visibility::Public)),
            ("total_", Some(Visibility::Private))
        ]
    );
    let color = entity(&result, "Color");
    let variants: Vec<&str> = color.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(variants, vec!["Red", "Green"]);
    assert_eq!(color.fields[1].default_value.as_deref(), Some("2"));
}

/// §B5. Language-specific facts are tags, not new fields.
#[test]
fn language_specific_facts_are_tags() {
    let result = parse(
        r#"
class Shape {
public:
    explicit Shape(int n);
    virtual ~Shape();
    virtual double area() const = 0;
    static Shape* make();
};
template <typename T> class Box {};
"#,
    );
    let constructor = result
        .entities
        .iter()
        .find(|e| e.name == "Shape" && e.kind == EntityKind::Method)
        .expect("constructor entity");
    assert!(constructor.tags.contains("constructor"));
    assert!(constructor.tags.contains("explicit"));
    assert!(tags(&result, "~Shape").contains(&"destructor".to_string()));
    assert!(tags(&result, "area").contains(&"pure_virtual".to_string()));
    assert!(tags(&result, "area").contains(&"abstract".to_string()));
    assert!(tags(&result, "make").contains(&"static".to_string()));
    assert!(tags(&result, "Box").contains(&"template".to_string()));
}

/// §B5, the access section. C++ has no modifier per member: a specifier
/// opens a section and everything after it inherits that access.
#[test]
fn access_sections_carry_forward_until_the_next_one() {
    let result = parse(
        r#"
class Order {
    void hidden();
public:
    void open();
    void alsoOpen();
protected:
    void guarded();
};
struct Plain { void free(); };
"#,
    );
    assert_eq!(entity(&result, "hidden").visibility, Visibility::Private);
    assert_eq!(entity(&result, "open").visibility, Visibility::Public);
    assert_eq!(entity(&result, "alsoOpen").visibility, Visibility::Public);
    assert_eq!(entity(&result, "guarded").visibility, Visibility::Protected);
    assert_eq!(entity(&result, "free").visibility, Visibility::Public);
}

/// §B6. One `Branch` per arm and one `Loop` per loop, addressed by a path
/// that restarts per body, with every edge inside an arm naming it.
#[test]
fn control_flow_becomes_entities() {
    let result = parse(
        r#"
void run(int n) {
    if (n > 0) { first(); } else if (n < 0) { second(); } else { third(); }
    for (int i = 0; i < n; ++i) { inLoop(); }
    switch (n) { case 1: one(); break; default: other(); }
    try { risky(); } catch (const Error& e) { recover(); }
}
"#,
    );
    let arms: Vec<String> = result
        .entities
        .iter()
        .filter(|e| matches!(e.kind, EntityKind::Branch | EntityKind::Loop))
        .map(|e| e.name.clone())
        .collect();
    assert_eq!(
        arms,
        vec!["c1", "c2", "c3", "l1", "c4", "c5", "c6", "c7"],
        "three if arms, one loop, two cases, a try body and a catch"
    );

    let branch_of = |callee: &str| {
        result
            .relationships
            .iter()
            .find(|r| r.target_id == callee)
            .and_then(|r| r.metadata.get("branch").cloned())
    };
    assert_eq!(branch_of("first"), Some("c1".to_string()));
    assert_eq!(branch_of("third"), Some("c3".to_string()));
    assert_eq!(branch_of("inLoop"), Some("l1".to_string()));
    assert_eq!(branch_of("one"), Some("c4".to_string()));
    assert_eq!(branch_of("recover"), Some("c7".to_string()));
}

/// §B6, the metadata an arm carries: what a case matched, what a catch
/// caught.
#[test]
fn arms_record_what_they_matched_and_caught() {
    let result = parse(
        r#"
void run(int n) {
    switch (n) { case 7: seven(); }
    try { risky(); } catch (const std::runtime_error& e) { recover(); }
}
"#,
    );
    let attributes: Vec<&str> = result
        .entities
        .iter()
        .flat_map(|e| e.attributes.iter())
        .map(String::as_str)
        .collect();
    assert!(attributes.contains(&"pattern:7"), "{attributes:?}");
    assert!(
        attributes
            .iter()
            .any(|a| a.starts_with("caught:") && a.contains("runtime_error")),
        "{attributes:?}"
    );
}

/// §B7. A lambda owns its scope: the calls inside it are its calls, and
/// the enclosing body names it without making them.
#[test]
fn a_lambda_is_an_entity_of_its_own() {
    let result = parse(
        r#"
void run() {
    auto f = [](int v) { return helper(v); };
    outer();
}
"#,
    );
    let lambda = result
        .entities
        .iter()
        .find(|e| e.name.starts_with("<lambda@"))
        .expect("lambda entity");
    assert!(lambda.tags.contains("lambda"));
    assert_eq!(lambda.parent_id.as_deref(), Some(entity(&result, "run").id.as_str()));
    assert_eq!(lambda.metrics.param_count, Some(1));

    let inner: Vec<&str> = result
        .relationships
        .iter()
        .filter(|r| r.source_id == lambda.id && r.kind == RelationshipKind::Calls)
        .map(|r| r.target_id.as_str())
        .collect();
    assert_eq!(inner, vec!["helper"]);
    assert_eq!(calls(&result, "run"), vec!["outer"]);
}

// --- C. What one thing does to another -------------------------------------

/// §C1. Every call site, in source order, with the ordinal taken before
/// any filtering so a dropped call still burns its number.
#[test]
fn calls_are_ordered_and_attributed() {
    let result = parse(
        r#"
void run() {
    first();
    xs.push_back(1);
    second();
}
"#,
    );
    let ordered: Vec<(&str, &str)> = result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
        .map(|r| {
            (
                r.target_id.as_str(),
                r.metadata.get("order").map(String::as_str).unwrap_or(""),
            )
        })
        .collect();
    assert_eq!(
        ordered,
        vec![("first", "1"), ("second", "3")],
        "the filtered push_back keeps its ordinal"
    );
}

/// §C2. `Instantiates` wherever C++ spells construction — and a stack
/// object is construction as much as a `new` is.
#[test]
fn construction_is_recorded_however_it_is_spelled() {
    let result = parse(
        r#"
void run() {
    Payment* p = new Payment(1);
    Widget w{2};
    Order o(3);
    Report r;
}
"#,
    );
    let mut built = targets(&result, "run", RelationshipKind::Instantiates);
    built.sort();
    assert_eq!(built, vec!["Order", "Payment", "Report", "Widget"]);
}

/// §C2 and §E4 together. `Matrix m = Matrix::identity();` is one
/// dependency, and recording the declared type as a construction *and*
/// the factory as a call would double-count it.
#[test]
fn a_declaration_initialised_by_a_call_is_not_also_a_construction() {
    let result = parse("void run() { Matrix m = Matrix::identity(); }");
    assert_eq!(
        targets(&result, "run", RelationshipKind::Instantiates),
        Vec::<String>::new()
    );
    assert_eq!(calls(&result, "run"), vec!["Matrix::identity"]);
}

/// §C3. Types named in a signature are dependencies of the callable, so a
/// type used only as a parameter still gains a dependent.
#[test]
fn uses_type_comes_from_every_type_position() {
    let result = parse(
        r#"
class Order {
public:
    Receipt settle(const LineItem& item, int count);
private:
    Repository* repo_;
};
"#,
    );
    let mut used = targets(&result, "settle", RelationshipKind::UsesType);
    used.sort();
    assert_eq!(used, vec!["LineItem", "Receipt"]);
    assert_eq!(
        targets(&result, "repo_", RelationshipKind::UsesType),
        vec!["Repository"]
    );
}

/// §C3, the filter. `std::vector<LineItem>` names two types and only one
/// of them is a dependency worth drawing.
#[test]
fn standard_library_types_are_not_dependencies() {
    let result = parse("class C { public: void take(std::vector<LineItem> xs, int n); };");
    assert_eq!(
        targets(&result, "take", RelationshipKind::UsesType),
        vec!["LineItem"]
    );
}

/// §C4. A `SCREAMING_CASE` name read without being called is a
/// dependency of full weight — change the constant and the reader changes
/// with it — and reading it twice is still one dependency.
#[test]
fn a_constant_read_without_being_called_is_a_dependency() {
    let result = parse(
        r#"
void run(int n) {
    if (n > MAX_ITEMS) { report(MAX_ITEMS); }
    int local = n;
    use(local);
}
"#,
    );
    assert_eq!(
        targets(&result, "run", RelationshipKind::UsesValue),
        vec!["MAX_ITEMS"]
    );
}

/// §C5. A write into a field is how you find the methods that make a
/// class stateful.
#[test]
fn a_field_write_is_recorded() {
    let result = parse(
        r#"
class Order {
public:
    void reset() {
        this->total_ = 0;
        count_ = 1;
        int local = 2;
    }
private:
    double total_;
    int count_;
};
"#,
    );
    let mut written = targets(&result, "reset", RelationshipKind::WritesTo);
    written.sort();
    assert_eq!(written, vec!["Order::count_", "Order::total_"]);
}

/// §C6. A thrown type is mentioned, not depended on — `References` is
/// deliberately outside the coupling tallies.
#[test]
fn a_thrown_type_is_referenced_not_called() {
    let result = parse(r#"void run() { throw ValidationError("bad"); }"#);
    assert_eq!(
        targets(&result, "run", RelationshipKind::References),
        vec!["ValidationError"]
    );
}

// --- D. What the entity scores ---------------------------------------------

/// §D1 and §D2. Every entity has `loc`; every callable has the three
/// complexity numbers, and a bodyless one scores `1 / 0 / 0` rather than
/// `None`.
#[test]
fn every_callable_is_measured_even_without_a_body() {
    let result = parse(
        r#"
class Shape {
public:
    virtual double area() const = 0;
    double describe(int n) const {
        if (n > 0 && n < 10) {
            for (int i = 0; i < n; ++i) { step(i); }
        }
        return 0;
    }
};
"#,
    );
    let area = entity(&result, "area");
    assert_eq!(area.metrics.cyclomatic, Some(1));
    assert_eq!(area.metrics.cognitive_complexity, Some(0));
    assert_eq!(area.metrics.max_nesting, Some(0));

    let describe = entity(&result, "describe");
    // 1 + if + `&&` + for
    assert_eq!(describe.metrics.cyclomatic, Some(4));
    assert!(describe.metrics.cognitive_complexity.unwrap() >= 3);
    assert_eq!(describe.metrics.max_nesting, Some(2));
    assert!(describe.metrics.loc >= 5);
}

/// §D3. Parameters, locals and the working set, through the one helper
/// every parser shares.
#[test]
fn the_working_set_counts_parameters_locals_and_fields() {
    let result = parse(
        r#"
class Order {
public:
    double total(int a, int b) {
        double sum = a + b;
        double scaled = sum * 2;
        return scaled + this->rate_;
    }
private:
    double rate_;
};
"#,
    );
    let total = entity(&result, "total");
    assert_eq!(total.metrics.param_count, Some(2));
    assert_eq!(total.metrics.local_count, Some(2));
    // 2 parameters + 2 locals + 1 field reached through `this->`
    assert_eq!(total.metrics.working_set, Some(5));
}

/// §D4. A lambda holding real logic must not rank as a body-less node.
#[test]
fn a_lambda_carries_the_same_metric_set() {
    let result = parse(
        r#"
void run() {
    auto pick = [](int v) { return v > 0 ? v : -v; };
}
"#,
    );
    let lambda = result
        .entities
        .iter()
        .find(|e| e.name.starts_with("<lambda@"))
        .expect("lambda entity");
    assert_eq!(lambda.metrics.cyclomatic, Some(2), "the conditional branches");
    assert_eq!(lambda.metrics.param_count, Some(1));
    assert!(lambda.metrics.working_set.is_some());
}

// --- E. How targets are spelled --------------------------------------------

/// §E1. A qualified call is spelled the way the resolver keys entities,
/// and template arguments are dropped so the target is a key rather than
/// a spelling.
#[test]
fn qualified_calls_use_the_resolvers_keys() {
    let result = parse(
        r#"
void run() {
    app::util::launch(1);
    Order::make(2);
    Cache<int>::get(3);
}
"#,
    );
    assert_eq!(
        calls(&result, "run"),
        vec!["Cache::get", "Order::make", "app::util::launch"]
    );
}

/// §E1, the bare case. A bare call inside a member function is a sibling
/// method only when this file says the class has one — otherwise it is a
/// free function, and qualifying it would invent a method nobody wrote.
#[test]
fn a_bare_call_is_qualified_only_when_the_class_has_such_a_member() {
    let result = parse(
        r#"
class Order {
public:
    void total() { applyDiscount(); freeHelper(); }
    void applyDiscount();
};
"#,
    );
    assert_eq!(
        calls(&result, "total"),
        vec!["Order::applyDiscount", "freeHelper"]
    );
}

/// §E2. A receiver resolves through its declared type, so a class that
/// reaches its collaborators through fields does not draw as depending on
/// none of them.
#[test]
fn a_receiver_resolves_through_its_declared_type() {
    let result = parse(
        r#"
class Order {
public:
    void save() {
        repo_->store(1);
        this->log();
        Session s;
        s.begin();
    }
    void log();
private:
    Repository* repo_;
};
"#,
    );
    assert_eq!(
        calls(&result, "save"),
        vec!["Order::log", "Repository::store", "Session::begin"]
    );
}

/// §E2 again, at the file edge. When no declaration in this file says
/// what a name holds, the receiver's text is left alone: a
/// receiver-shaped ghost is honest, and a guessed type is not.
#[test]
fn an_untyped_receiver_keeps_its_own_text() {
    let result = parse("double Order::total() { return repo_->fetch(); }");
    assert_eq!(calls(&result, "total"), vec!["repo_::fetch"]);
}

/// §E3. `(*ptr).method()` has no name that describes its receiver.
/// Dropping the edge is the correct answer; naming an entity after the
/// expression is not.
#[test]
fn a_call_through_an_unnameable_receiver_is_declined() {
    let result = parse("void run(int a, int b) { (a + b).method(); kept(); }");
    assert_eq!(calls(&result, "run"), vec!["kept"]);
}

/// §E3 has a limit, and it is worth naming: a parenthesised expression
/// whose receiver *is* a declared name still resolves. `(*ptr).method()`
/// says exactly as much about its receiver as `ptr->method()` does.
#[test]
fn a_dereferenced_pointer_still_resolves_through_its_type() {
    let result = parse("void run(Thing* ptr) { (*ptr).method(); }");
    assert_eq!(calls(&result, "run"), vec!["Thing::method"]);
}

/// §E4. An imported name in callee position is a `Calls` edge and nothing
/// else — a value pass that also claimed it would double-count one
/// dependency across two kinds.
#[test]
fn a_called_name_is_not_also_a_value_read() {
    let result = parse("void run() { MAX_RETRIES(); }");
    assert_eq!(
        targets(&result, "run", RelationshipKind::UsesValue),
        Vec::<String>::new()
    );
    assert_eq!(calls(&result, "run"), vec!["MAX_RETRIES"]);
}

// --- C, the filters --------------------------------------------------------

/// The standard library is filtered by qualifier where C++ gives one and
/// by member name where it does not — but binding the result to a local
/// lifts the filter, because that is the caller saying the value matters.
#[test]
fn the_standard_library_is_filtered_unless_the_result_is_bound() {
    let result = parse(
        r#"
void run() {
    std::sort(xs);
    xs.push_back(1);
    project();
}
void keep() {
    int n = xs.size();
}
"#,
    );
    assert_eq!(calls(&result, "run"), vec!["project"]);
    assert_eq!(calls(&result, "keep"), vec!["xs::size"]);
}

/// A header guard names the file it is already in, and is not a
/// declaration a reader navigates to. A valueless macro that is *not* a
/// guard still is one.
#[test]
fn a_header_guard_is_not_a_macro_entity() {
    let result = parse(
        r#"
#ifndef ORDER_H
#define ORDER_H
#define WITH_METRICS
#define MAX_ITEMS 100
#endif
"#,
    );
    assert_eq!(
        names_of_kind(&result, EntityKind::Macro),
        vec!["MAX_ITEMS", "WITH_METRICS"]
    );
}

/// Declarations inside a preprocessor conditional are declarations. The
/// walk goes through `#ifdef` rather than around it.
#[test]
fn a_gated_declaration_is_still_declared() {
    let result = parse(
        r#"
#ifdef WITH_METRICS
void report() { emit(); }
#endif
"#,
    );
    assert_eq!(calls(&result, "report"), vec!["emit"]);
}

/// A forward declaration says a name exists and nothing else. Twenty
/// headers saying `class Order;` must not produce twenty classes.
#[test]
fn a_forward_declaration_declares_no_entity() {
    let result = parse("class Order;\nclass Order { public: void save(); };");
    assert_eq!(names_of_kind(&result, EntityKind::Class), vec!["Order"]);
}

/// An anonymous namespace declares no name to qualify by, and its
/// contents are visible to no other translation unit.
#[test]
fn an_anonymous_namespace_qualifies_nothing_and_hides_its_contents() {
    let result = parse("namespace { void helper() {} }");
    assert!(result.entities.iter().all(|e| e.kind != EntityKind::Module));
    let helper = entity(&result, "helper");
    assert_eq!(helper.qualified_name, "helper");
    assert_eq!(helper.visibility, Visibility::Internal);
}

/// A constructor's member-initialiser list runs before its body and
/// builds what the body then uses, so its calls are the constructor's.
#[test]
fn a_member_initialiser_list_belongs_to_the_constructor() {
    let result = parse("Order::Order(int id) : id_(id), repo_(new Repository()) { setup(); }");
    assert_eq!(
        targets(&result, "Order", RelationshipKind::Instantiates),
        vec!["Repository"]
    );
    assert_eq!(calls(&result, "Order"), vec!["setup"]);
}

/// The same grammar parses C, and a C translation unit is a subset of
/// what the walk already handles.
#[test]
fn plain_c_parses_through_the_same_walk() {
    let result = CppParser::for_language(Language::C)
        .parse(
            Path::new("store.c"),
            r#"
struct Node { int value; struct Node* next; };

int sum(struct Node* head) {
    int total = 0;
    while (head) {
        total += head->value;
        head = head->next;
    }
    return total;
}
"#,
        )
        .expect("parse");
    assert_eq!(names_of_kind(&result, EntityKind::Struct), vec!["Node"]);
    let sum = entity(&result, "sum");
    assert_eq!(sum.metrics.cyclomatic, Some(2));
    assert_eq!(sum.metrics.param_count, Some(1));
}
