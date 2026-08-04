//! Integration tests for the educator module.
//!
//! Each test sets up an isolated temp directory holding a minimal `content/`
//! tree and a Java source file, then drives the full pipeline:
//! load rules → query position → assert on the partitioned response.

use super::*;
use crate::educator::catalog;
use std::fs;

struct TmpDir(std::path::PathBuf);

impl TmpDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "nao-edu-test-{}-{}-{}",
            name,
            std::process::id(),
            // Nanosecond timestamp keeps parallel test invocations from colliding.
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        fs::create_dir_all(&path).unwrap();
        TmpDir(path)
    }

    fn write(&self, rel: &str, body: &str) -> std::path::PathBuf {
        let full = self.0.join(rel);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&full, body).unwrap();
        full
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const SYNC_THIS_RULE: &str = r#"---
id: synchronized-on-this
language: java
applies-to: [synchronized_statement]
match:
  lock_expr_kind: this
severity: warning
kind: gotcha
---

# Don't synchronize on `this`

## Bad
```java
public void foo() {
    synchronized (this) { ... }
}
```

## Good
```java
private final Object lock = new Object();
synchronized (lock) { ... }
```

## Why
Locking on `this` exposes the lock to every caller of the public class —
external code can `synchronized (instance)` and deadlock you.
"#;

fn java_file(body: &str) -> String {
    format!(
        "package com.example;\n\npublic class Account {{\n    public void transfer() {{\n{}\n    }}\n}}\n",
        body
    )
}

/// Find the line/col of the `synchronized` keyword on the first line that
/// contains it.
fn locate(source: &str, needle: &str) -> (u32, u32) {
    for (line_idx, line) in source.lines().enumerate() {
        if let Some(col) = line.find(needle) {
            return (line_idx as u32, col as u32);
        }
    }
    panic!("needle {:?} not found in source", needle);
}

#[test]
fn loads_rule_from_content_tree() {
    let tmp = TmpDir::new("load");
    tmp.write("content/java/rules/synchronized-on-this.md", SYNC_THIS_RULE);

    let educator = Educator::load(&tmp.0.join("content")).expect("load");

    assert_eq!(educator.rules().len(), 1);
    let rule = &educator.rules()[0];
    assert_eq!(rule.id, "synchronized-on-this");
    assert_eq!(rule.language, "java");
    assert_eq!(rule.applies_to, vec!["synchronized_statement"]);
    assert!(rule.is_specific());
    assert_eq!(rule.severity, "warning");
    assert_eq!(rule.title(), "Don't synchronize on `this`");
}

#[test]
fn missing_content_root_is_empty_not_error() {
    let educator = Educator::load(std::path::Path::new("/nonexistent/path")).unwrap();
    assert_eq!(educator.rules().len(), 0);
}

#[test]
fn rejects_unknown_predicate_primitive() {
    let tmp = TmpDir::new("badpred");
    let body = r#"---
id: bad-rule
language: java
applies-to: [synchronized_statement]
match:
  lock_expr_kind:
    matches: "this.*"
---
# Bad

## Why
unused
"#;
    tmp.write("content/java/rules/bad.md", body);

    let educator = Educator::load(&tmp.0.join("content")).unwrap();
    assert_eq!(educator.rules().len(), 0, "unknown predicate must drop the rule");
    let errors: Vec<_> = educator
        .issues()
        .iter()
        .filter(|i| i.severity == LoadIssueSeverity::Error)
        .collect();
    assert!(!errors.is_empty(), "issue list must report the unknown predicate");
    let msg = &errors[0].message;
    assert!(
        msg.contains("matches"),
        "issue must name the offending primitive, got: {}",
        msg
    );
}

#[test]
fn rejects_unknown_construct_kind_with_suggestion() {
    let tmp = TmpDir::new("badkind");
    let body = r#"---
id: typo-rule
language: java
applies-to: [synchroized_statement]
severity: warning
kind: gotcha
---

# Typo

## Why
unused
"#;
    tmp.write("content/java/rules/typo.md", body);

    let educator = Educator::load(&tmp.0.join("content")).unwrap();
    assert_eq!(educator.rules().len(), 0, "unknown kind must drop the rule");
    let issue = &educator.issues()[0];
    assert_eq!(issue.field.as_deref(), Some("applies-to"));
    assert!(
        issue.suggestion.as_deref().unwrap_or("").contains("synchronized_statement"),
        "suggestion should point at the real kind, got: {:?}",
        issue.suggestion
    );
}

#[test]
fn rejects_unknown_match_attribute() {
    let tmp = TmpDir::new("badattr");
    let body = r#"---
id: bad-attr
language: java
applies-to: [synchronized_statement]
match:
  not_a_real_attr: this
severity: warning
kind: gotcha
---

# Bad attr

## Why
unused
"#;
    tmp.write("content/java/rules/badattr.md", body);

    let educator = Educator::load(&tmp.0.join("content")).unwrap();
    assert_eq!(educator.rules().len(), 0);
    assert!(
        educator
            .issues()
            .iter()
            .any(|i| i.field.as_deref() == Some("match.not_a_real_attr"))
    );
}

#[test]
fn duplicate_ids_drop_both_copies() {
    let tmp = TmpDir::new("dup");
    let body = r#"---
id: dup-rule
language: java
applies-to: [synchronized_statement]
severity: warning
kind: gotcha
---

# A

## Why
unused
"#;
    tmp.write("content/java/rules/a.md", body);
    tmp.write("content/java/rules/b.md", body);

    let educator = Educator::load(&tmp.0.join("content")).unwrap();
    assert_eq!(educator.rules().len(), 0, "both copies must be dropped");
    let id_issues: Vec<_> = educator
        .issues()
        .iter()
        .filter(|i| i.field.as_deref() == Some("id"))
        .collect();
    assert_eq!(id_issues.len(), 2, "one issue per duplicate copy");
}

#[test]
fn missing_why_is_warning_not_error() {
    let tmp = TmpDir::new("nowhy");
    let body = r#"---
id: no-why
language: java
applies-to: [synchronized_statement]
severity: warning
kind: gotcha
---

# Hello world
"#;
    tmp.write("content/java/rules/nowhy.md", body);

    let educator = Educator::load(&tmp.0.join("content")).unwrap();
    assert_eq!(educator.rules().len(), 1, "rule must be kept");
    assert!(
        educator
            .issues()
            .iter()
            .any(|i| i.severity == LoadIssueSeverity::Warning
                && i.field.as_deref() == Some("body"))
    );
}

/// Load the real `content/java/` corpus that ships in the repo. End-to-end
/// tests below use this to validate that the canonical rule set works.
fn load_repo_corpus() -> Educator {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("content");
    Educator::load(&path).expect("repo corpus loads")
}

/// Find the line/col of the *last* occurrence of `needle` in `source`. Lets
/// tests look at a specific token in a longer file (e.g. the `==` operator
/// inside a method body rather than the one in the `boolean ==(…)` signature).
fn locate_last(source: &str, needle: &str) -> (u32, u32) {
    let mut best: Option<(u32, u32)> = None;
    for (line_idx, line) in source.lines().enumerate() {
        if let Some(col) = line.rfind(needle) {
            best = Some((line_idx as u32, col as u32));
        }
    }
    best.unwrap_or_else(|| panic!("needle {:?} not found in source", needle))
}

#[test]
fn boxed_equality_fires_on_integer_compare() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("boxed-eq");
    let source = r#"
public class C {
    public boolean eq() {
        Integer a = 1;
        Integer b = 2;
        return a == b;
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate_last(source, "==");
    let resp = educator
        .query_position(&file, line, col + 1)
        .unwrap();
    assert!(
        resp.specific.iter().any(|r| r.rule_id == "boxed-equality"),
        "expected boxed-equality to fire, specific={:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
}

#[test]
fn boxed_equality_silent_on_primitive_compare() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("primitive-eq");
    let source = r#"
public class C {
    public boolean eq() {
        int a = 1;
        int b = 2;
        return a == b;
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate_last(source, "==");
    let resp = educator
        .query_position(&file, line, col + 1)
        .unwrap();
    assert!(
        !resp.specific.iter().any(|r| r.rule_id == "boxed-equality"),
        "primitive int compare must not fire boxed-equality"
    );
}

#[test]
fn finalize_deprecated_fires_on_finalize_method() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("finalize");
    let source = r#"
public class C {
    @Override
    protected void finalize() throws Throwable {
        super.finalize();
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "finalize() throws");
    let resp = educator
        .query_position(&file, line, col)
        .unwrap();
    assert!(
        resp.specific.iter().any(|r| r.rule_id == "finalize-deprecated"),
        "expected finalize-deprecated to fire, specific={:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
}

#[test]
fn finalize_deprecated_silent_on_other_methods() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("other-method");
    let source = r#"
public class C {
    public void foo() { }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "foo() {");
    let resp = educator
        .query_position(&file, line, col)
        .unwrap();
    assert!(
        !resp.specific.iter().any(|r| r.rule_id == "finalize-deprecated"),
        "must not fire on non-finalize methods"
    );
}

#[test]
fn arrays_aslist_fires() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("aslist");
    let source = r#"
import java.util.Arrays;
public class C {
    public void use() {
        var xs = Arrays.asList(1, 2, 3);
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "Arrays.asList");
    // Aim at the method-name part of the call so the innermost node is the
    // method_invocation.
    let resp = educator
        .query_position(&file, line, col + "Arrays.".len() as u32)
        .unwrap();
    assert!(
        resp.specific.iter().any(|r| r.rule_id == "arrays-aslist-mutability"),
        "expected arrays-aslist-mutability to fire, specific={:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
}

#[test]
fn arrays_aslist_silent_on_other_calls() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("not-aslist");
    let source = r#"
public class C {
    public void use() {
        String x = String.valueOf(42);
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "valueOf");
    let resp = educator
        .query_position(&file, line, col)
        .unwrap();
    assert!(
        !resp.specific.iter().any(|r| r.rule_id == "arrays-aslist-mutability"),
        "must not fire on String.valueOf"
    );
}

#[test]
fn raw_types_fires_on_bare_list() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("raw-list");
    let source = r#"
import java.util.List;
public class C {
    public void use() {
        List items = null;
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "List items");
    let resp = educator
        .query_position(&file, line, col)
        .unwrap();
    assert!(
        resp.specific.iter().any(|r| r.rule_id == "raw-types-warning"),
        "expected raw-types-warning to fire on bare List, specific={:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
}

#[test]
fn raw_types_silent_on_parameterised_list() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("parameterised-list");
    let source = r#"
import java.util.List;
public class C {
    public void use() {
        List<String> items = null;
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "List<String>");
    let resp = educator
        .query_position(&file, line, col)
        .unwrap();
    assert!(
        !resp.specific.iter().any(|r| r.rule_id == "raw-types-warning"),
        "must not fire on parameterised List<String>"
    );
}

#[test]
fn synchronized_method_fires_on_synchronized_modifier() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("sync-method");
    let source = r#"
public class C {
    public synchronized void foo() { }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "synchronized");
    let resp = educator
        .query_position(&file, line, col)
        .unwrap();
    assert!(
        resp.specific
            .iter()
            .any(|r| r.rule_id == "synchronized-method-on-this"),
        "expected synchronized-method-on-this to fire, specific={:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
}

#[test]
fn synchronized_method_silent_on_plain_method() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("plain-method");
    let source = r#"
public class C {
    public void foo() { }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "void");
    let resp = educator
        .query_position(&file, line, col)
        .unwrap();
    assert!(
        !resp.specific
            .iter()
            .any(|r| r.rule_id == "synchronized-method-on-this"),
        "must not fire on non-synchronized method"
    );
}

#[test]
fn equals_without_hashcode_fires() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("eq-no-hc");
    let source = r#"
public class Point {
    private final int x;

    @Override
    public boolean equals(Object o) { return false; }
}
"#;
    let file = tmp.write("Point.java", source);
    let (line, col) = locate(source, "class Point");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        resp.specific.iter().any(|r| r.rule_id == "equals-without-hashcode"),
        "expected equals-without-hashcode to fire, specific={:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
}

#[test]
fn equals_without_hashcode_silent_when_both_present() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("eq-and-hc");
    let source = r#"
public class Point {
    public boolean equals(Object o) { return false; }
    public int hashCode() { return 0; }
}
"#;
    let file = tmp.write("Point.java", source);
    let (line, col) = locate(source, "class Point");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        !resp.specific.iter().any(|r| r.rule_id == "equals-without-hashcode"),
        "must not fire when both are present"
    );
}

#[test]
fn public_mutable_static_field_fires() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("pms");
    let source = r#"
public class Config {
    public static int maxRetries = 3;
}
"#;
    let file = tmp.write("Config.java", source);
    let (line, col) = locate(source, "maxRetries");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        resp.specific.iter().any(|r| r.rule_id == "public-mutable-static-field"),
        "expected public-mutable-static-field, specific={:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
}

#[test]
fn public_mutable_static_field_silent_on_final() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("pms-final");
    let source = r#"
public class Config {
    public static final int DEFAULT = 3;
}
"#;
    let file = tmp.write("Config.java", source);
    let (line, col) = locate(source, "DEFAULT");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        !resp.specific.iter().any(|r| r.rule_id == "public-mutable-static-field"),
        "must not fire on public static final"
    );
}

#[test]
fn prefer_constructor_injection_fires_on_resource_field() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("inject-field");
    let source = r#"package com.example;
import javax.annotation.Resource;
public class OrderService {
    @Resource
    private OrderRepository repository;
}
"#;
    let file = tmp.write("OrderService.java", source);
    let (line, col) = locate(source, "@Resource");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        resp.specific
            .iter()
            .any(|r| r.rule_id == "prefer-constructor-injection"),
        "expected prefer-constructor-injection, specific={:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
}

#[test]
fn prefer_constructor_injection_silent_on_override() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("inject-override");
    let source = r#"package com.example;
public class C {
    @Override
    public String toString() { return "C"; }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "@Override");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        !resp.specific
            .iter()
            .any(|r| r.rule_id == "prefer-constructor-injection"),
        "must not fire on @Override"
    );
}

#[test]
fn checked_exception_fires_on_throws() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("checked");
    let source = r#"
public class C {
    public int parse(String s) throws IOException { return 0; }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "parse(String");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        resp.specific.iter().any(|r| r.rule_id == "checked-exception-over-use"),
        "expected checked-exception-over-use, specific={:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
}

#[test]
fn checked_exception_silent_on_method_without_throws() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("no-throws");
    let source = r#"
public class C {
    public int parse(String s) { return 0; }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "parse(String");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        !resp.specific.iter().any(|r| r.rule_id == "checked-exception-over-use"),
        "must not fire when no throws clause"
    );
}

#[test]
fn try_with_resources_fires_on_plain_try() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("plain-try");
    let source = r#"
public class C {
    public void use() {
        try {
            doSomething();
        } catch (Exception e) {
            // ignore
        }
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "try {");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        resp.specific.iter().any(|r| r.rule_id == "try-with-resources-opportunity"),
        "expected try-with-resources-opportunity, specific={:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
}

#[test]
fn try_with_resources_silent_on_resource_form() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("twr");
    let source = r#"
public class C {
    public String use() throws Exception {
        try (java.io.BufferedReader r = open()) {
            return r.readLine();
        }
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "try (");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        !resp.specific.iter().any(|r| r.rule_id == "try-with-resources-opportunity"),
        "must not fire on try-with-resources form"
    );
}

#[test]
fn legacy_date_time_fires() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("legacy-date");
    let source = "package com.example;\nimport java.util.Date;\npublic class C { Date created; }\n";
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "Date created");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        resp.specific.iter().any(|r| r.rule_id == "legacy-date-time"),
        "expected legacy-date-time, specific={:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
}

#[test]
fn legacy_date_time_silent_on_java_time() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("modern-date");
    let source = "package com.example;\nimport java.time.Instant;\npublic class C { Instant created; }\n";
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "Instant created");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        !resp.specific.iter().any(|r| r.rule_id == "legacy-date-time"),
        "java.time.Instant must not fire legacy-date-time"
    );
}

#[test]
fn legacy_thread_safe_collections_fires() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("legacy-vector");
    let source = "package com.example;\nimport java.util.Vector;\npublic class C { Vector recent; }\n";
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "Vector recent");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        resp.specific.iter().any(|r| r.rule_id == "legacy-thread-safe-collections"),
        "expected legacy-thread-safe-collections, specific={:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
}

#[test]
fn prefer_interface_fires_on_concrete_variable() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("prefer-iface");
    let source = "package com.example;\nimport java.util.ArrayList;\npublic class C { ArrayList<String> names; }\n";
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "ArrayList<String>");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        resp.specific.iter().any(|r| r.rule_id == "prefer-interface-as-variable-type"),
        "expected prefer-interface-as-variable-type, specific={:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
}

#[test]
fn prefer_interface_silent_on_interface_variable() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("interface-already");
    let source = "package com.example;\nimport java.util.List;\npublic class C { List<String> names; }\n";
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "List<String>");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        !resp.specific.iter().any(|r| r.rule_id == "prefer-interface-as-variable-type"),
        "interface declarations must not fire prefer-interface-as-variable-type"
    );
}

#[test]
fn prefer_interface_silent_on_raw_concrete() {
    // Raw `ArrayList` (no generic args) is already covered by raw-types-warning;
    // prefer-interface-as-variable-type must not double-fire on the same case.
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("raw-concrete");
    let source = "package com.example;\nimport java.util.ArrayList;\npublic class C { ArrayList names; }\n";
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "ArrayList names");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        !resp.specific.iter().any(|r| r.rule_id == "prefer-interface-as-variable-type"),
        "raw ArrayList (no <T>) must not fire prefer-interface (raw-types-warning handles it)"
    );
}

#[test]
fn print_stacktrace_fires() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("printstacktrace");
    let source = r#"package com.example;
public class C {
    public void use(Exception e) {
        e.printStackTrace();
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "printStackTrace");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        resp.specific.iter().any(|r| r.rule_id == "exception-printstacktrace"),
        "expected exception-printstacktrace, specific={:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
}

#[test]
fn wait_notify_fires_on_wait_call() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("wait-notify");
    let source = r#"package com.example;
public class C {
    private final Object lock = new Object();
    public void block() throws InterruptedException {
        synchronized (lock) {
            lock.wait();
        }
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "lock.wait");
    let resp = educator
        .query_position(&file, line, col + "lock.".len() as u32)
        .unwrap();
    assert!(
        resp.specific.iter().any(|r| r.rule_id == "wait-notify-low-level"),
        "expected wait-notify-low-level, specific={:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
}

#[test]
fn prefer_concurrent_collections_fires() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("synchronized-map");
    let source = r#"package com.example;
import java.util.Collections;
import java.util.HashMap;
public class C {
    public Object build() {
        return Collections.synchronizedMap(new HashMap<>());
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "synchronizedMap");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        resp.specific.iter().any(|r| r.rule_id == "prefer-concurrent-collections"),
        "expected prefer-concurrent-collections, specific={:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
}

#[test]
fn wildcard_import_fires() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("wildcard-import");
    let source = "package com.example;\nimport java.util.*;\npublic class C {}\n";
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "java.util.*");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        resp.specific.iter().any(|r| r.rule_id == "wildcard-import"),
        "expected wildcard-import to fire, specific={:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
}

#[test]
fn wildcard_import_silent_on_specific_import() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("specific-import");
    let source = "package com.example;\nimport java.util.List;\npublic class C {}\n";
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "java.util.List");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        !resp.specific.iter().any(|r| r.rule_id == "wildcard-import"),
        "specific import must not fire wildcard-import"
    );
}

#[test]
fn static_import_fires() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("static-import");
    let source = "package com.example;\nimport static java.lang.Math.PI;\npublic class C {}\n";
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "static java.lang.Math");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        resp.specific.iter().any(|r| r.rule_id == "static-import"),
        "expected static-import to fire, specific={:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
}

#[test]
fn default_package_fires_when_no_package() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("default-package");
    // No `package` statement → class in the default package.
    let source = "public class Service {}\n";
    let file = tmp.write("Service.java", source);
    let (line, col) = locate(source, "class Service");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        resp.specific.iter().any(|r| r.rule_id == "default-package"),
        "expected default-package to fire, specific={:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
}

#[test]
fn default_package_silent_when_package_present() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("named-package");
    let source = "package com.example;\npublic class Service {}\n";
    let file = tmp.write("Service.java", source);
    let (line, col) = locate(source, "class Service");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        !resp.specific.iter().any(|r| r.rule_id == "default-package"),
        "classes inside a named package must not fire default-package"
    );
}

#[test]
fn default_method_in_interface_fires() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("default-method");
    let source = r#"package com.example;
public interface R {
    default int foo() { return 0; }
}
"#;
    let file = tmp.write("R.java", source);
    let (line, col) = locate(source, "foo()");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        resp.specific.iter().any(|r| r.rule_id == "default-method-in-interface"),
        "expected default-method-in-interface to fire, specific={:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
}

#[test]
fn default_method_silent_on_regular_method() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("regular-method");
    let source = r#"package com.example;
public class C {
    public int foo() { return 0; }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "foo()");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        !resp.specific.iter().any(|r| r.rule_id == "default-method-in-interface"),
        "non-default method must not fire default-method-in-interface"
    );
}

#[test]
fn system_out_println_fires() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("println");
    let source = r#"
public class C {
    public void use() {
        System.out.println("hi");
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "println");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        resp.specific.iter().any(|r| r.rule_id == "system-out-println"),
        "expected system-out-println, specific={:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
}

#[test]
fn prefer_diamond_over_guava_lists_fires_on_lists_new_array_list() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("guava-lists");
    let source = r#"package com.example;
import com.google.common.collect.Lists;
public class C {
    public void use() {
        var xs = Lists.newArrayList();
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "Lists.newArrayList");
    let resp = educator
        .query_position(&file, line, col + "Lists.".len() as u32)
        .unwrap();
    assert!(
        resp.specific
            .iter()
            .any(|r| r.rule_id == "prefer-diamond-over-guava-lists"),
        "expected prefer-diamond-over-guava-lists on Lists.newArrayList, specific={:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
}

#[test]
fn prefer_diamond_over_guava_lists_fires_on_maps_new_hash_map() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("guava-maps");
    let source = r#"package com.example;
import com.google.common.collect.Maps;
public class C {
    public void use() {
        var m = Maps.newHashMap();
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "Maps.newHashMap");
    let resp = educator
        .query_position(&file, line, col + "Maps.".len() as u32)
        .unwrap();
    assert!(
        resp.specific
            .iter()
            .any(|r| r.rule_id == "prefer-diamond-over-guava-lists"),
        "expected prefer-diamond-over-guava-lists on Maps.newHashMap, specific={:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
}

#[test]
fn prefer_diamond_over_guava_lists_silent_on_diamond_constructor() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("diamond-new");
    let source = r#"package com.example;
import java.util.ArrayList;
public class C {
    public void use() {
        var xs = new ArrayList<String>();
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "ArrayList<String>");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        !resp.specific
            .iter()
            .any(|r| r.rule_id == "prefer-diamond-over-guava-lists"),
        "constructor form must not fire prefer-diamond-over-guava-lists"
    );
}

#[test]
fn prefer_diamond_over_guava_lists_silent_on_immutable_list() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("immutable-list");
    let source = r#"package com.example;
import com.google.common.collect.ImmutableList;
public class C {
    public void use() {
        var xs = ImmutableList.of("a", "b");
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "ImmutableList.of");
    let resp = educator
        .query_position(&file, line, col + "ImmutableList.".len() as u32)
        .unwrap();
    assert!(
        !resp.specific
            .iter()
            .any(|r| r.rule_id == "prefer-diamond-over-guava-lists"),
        "ImmutableList.of must not fire prefer-diamond-over-guava-lists \
         (it has no JDK one-liner equivalent)"
    );
}

// ─── Lesson tests ────────────────────────────────────────────────────────────

const TRY_LESSON: &str = r#"---
id: try-fundamentals
language: java
applies-to: [try_statement]
title: Exception handling fundamentals
level: intermediate
sources:
  - "JLS §14.20"
---

# Exception handling fundamentals

Wraps code that might throw so the caller can react instead of crashing.
"#;

#[test]
fn loads_lesson_from_content_tree() {
    let tmp = TmpDir::new("load-lesson");
    tmp.write("content/java/lessons/try-fundamentals.md", TRY_LESSON);
    let educator = Educator::load(&tmp.0.join("content")).expect("load");

    assert_eq!(educator.lessons().len(), 1);
    let lesson = &educator.lessons()[0];
    assert_eq!(lesson.id, "try-fundamentals");
    assert_eq!(lesson.language, "java");
    assert_eq!(lesson.applies_to, vec!["try_statement"]);
    assert_eq!(lesson.level, "intermediate");
    assert_eq!(lesson.title, "Exception handling fundamentals");
}

#[test]
fn lesson_with_unknown_kind_is_rejected() {
    let tmp = TmpDir::new("lesson-bad-kind");
    let body = r#"---
id: bad-lesson
language: java
applies-to: [synchroized_statement]
title: bad
level: beginner
---

# Body
"#;
    tmp.write("content/java/lessons/bad.md", body);
    let educator = Educator::load(&tmp.0.join("content")).unwrap();
    assert_eq!(educator.lessons().len(), 0, "unknown kind must drop the lesson");
    let issue = &educator
        .issues()
        .iter()
        .find(|i| i.field.as_deref() == Some("applies-to"))
        .expect("applies-to issue surfaced");
    assert!(
        issue.suggestion.as_deref().unwrap_or("").contains("synchronized_statement"),
        "suggestion should point at the real kind, got: {:?}",
        issue.suggestion
    );
}

#[test]
fn lesson_with_unknown_level_is_warning_not_error() {
    let tmp = TmpDir::new("lesson-bad-level");
    let body = r#"---
id: oddlevel
language: java
applies-to: [try_statement]
title: weirdness
level: expert
---

# Body
"#;
    tmp.write("content/java/lessons/oddlevel.md", body);
    let educator = Educator::load(&tmp.0.join("content")).unwrap();
    assert_eq!(educator.lessons().len(), 1, "lesson kept on warning-only issue");
    assert!(
        educator
            .issues()
            .iter()
            .any(|i| i.severity == LoadIssueSeverity::Warning
                && i.field.as_deref() == Some("level"))
    );
}

#[test]
fn duplicate_lesson_ids_drop_both_copies() {
    let tmp = TmpDir::new("dup-lessons");
    tmp.write("content/java/lessons/a.md", TRY_LESSON);
    tmp.write("content/java/lessons/b.md", TRY_LESSON);
    let educator = Educator::load(&tmp.0.join("content")).unwrap();
    assert_eq!(educator.lessons().len(), 0, "both duplicates must be dropped");
}

#[test]
fn position_query_returns_lessons() {
    let tmp = TmpDir::new("pos-lesson");
    tmp.write("content/java/lessons/try-fundamentals.md", TRY_LESSON);
    let educator = Educator::load(&tmp.0.join("content")).unwrap();

    let java_src = r#"package com.example;
public class C {
    public void use() {
        try {
            int x = 1;
        } catch (Exception e) {}
    }
}
"#;
    let file = tmp.write("C.java", java_src);
    let (line, col) = locate(java_src, "try {");
    let resp = educator.query_position(&file, line, col).unwrap();

    assert!(
        resp.lessons.iter().any(|l| l.lesson_id == "try-fundamentals"),
        "expected try-fundamentals lesson, lessons={:?}",
        resp.lessons.iter().map(|l| &l.lesson_id).collect::<Vec<_>>()
    );
}

#[test]
fn position_query_lessons_dedup_across_ancestors() {
    // A lesson attached to both `method_declaration` and `class_declaration`
    // should fire only once even when the cursor is inside both ancestors.
    let tmp = TmpDir::new("lesson-dedup");
    let body = r#"---
id: shared-lesson
language: java
applies-to: [method_declaration, class_declaration]
title: Shared
level: beginner
---

# Body
"#;
    tmp.write("content/java/lessons/shared.md", body);
    let educator = Educator::load(&tmp.0.join("content")).unwrap();

    let java_src = r#"package com.example;
public class C {
    public void use() {
        int x = 1;
    }
}
"#;
    let file = tmp.write("C.java", java_src);
    let (line, col) = locate(java_src, "int x");
    let resp = educator.query_position(&file, line, col).unwrap();

    let count = resp.lessons.iter().filter(|l| l.lesson_id == "shared-lesson").count();
    assert_eq!(count, 1, "lesson must dedup across ancestor matches");
}

#[test]
fn repo_corpus_lessons_load_without_validator_errors() {
    let educator = load_repo_corpus();
    let errors: Vec<_> = educator
        .issues()
        .iter()
        .filter(|i| i.severity == LoadIssueSeverity::Error)
        .filter(|i| i.path.to_string_lossy().contains("/lessons/"))
        .collect();
    assert!(
        errors.is_empty(),
        "repo lesson corpus must load without errors: {:#?}",
        errors
    );
}

#[test]
fn repo_corpus_position_query_surfaces_lessons() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("repo-lessons-position");
    let source = r#"package com.example;
public class Demo {
    public void use() {
        try { } catch (Exception e) {}
    }
}
"#;
    let file = tmp.write("Demo.java", source);
    let (line, col) = locate(source, "try {");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        resp.lessons.iter().any(|l| l.lesson_id == "try-statement-fundamentals"),
        "expected try-statement lesson from repo corpus, got: {:?}",
        resp.lessons.iter().map(|l| &l.lesson_id).collect::<Vec<_>>()
    );
}

#[test]
fn repo_corpus_field_declaration_lesson_fires() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("repo-lesson-field");
    let source = r#"package com.example;
public class C {
    private final int balance = 0;
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "balance");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        resp.lessons
            .iter()
            .any(|l| l.lesson_id == "field-declaration-fundamentals"),
        "expected field-declaration-fundamentals lesson, got: {:?}",
        resp.lessons.iter().map(|l| &l.lesson_id).collect::<Vec<_>>()
    );
}

#[test]
fn repo_corpus_declared_type_lesson_fires() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("repo-lesson-declared-type");
    let source = r#"package com.example;
import java.util.List;
public class C {
    public void use() {
        List<String> names = null;
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "List<String>");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        resp.lessons
            .iter()
            .any(|l| l.lesson_id == "declared-type-fundamentals"),
        "expected declared-type-fundamentals lesson, got: {:?}",
        resp.lessons.iter().map(|l| &l.lesson_id).collect::<Vec<_>>()
    );
}

#[test]
fn repo_corpus_method_invocation_lesson_fires() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("repo-lesson-method-invocation");
    let source = r#"package com.example;
public class C {
    public int parse(String s) {
        return Integer.parseInt(s);
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "Integer.parseInt");
    let resp = educator
        .query_position(&file, line, col + "Integer.".len() as u32)
        .unwrap();
    assert!(
        resp.lessons
            .iter()
            .any(|l| l.lesson_id == "method-invocation-fundamentals"),
        "expected method-invocation-fundamentals lesson, got: {:?}",
        resp.lessons.iter().map(|l| &l.lesson_id).collect::<Vec<_>>()
    );
}

#[test]
fn lesson_attachment_distinguishes_direct_from_ancestor() {
    // Cursor on a lambda body inside a method: the lambda lesson should be
    // marked `direct` (attached_to == lambda_expression, the innermost
    // construct under the cursor), while the method lesson should be marked
    // `ancestor` (attached_to == method_declaration, an enclosing construct).
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("attachment-classification");
    let source = r#"package com.example;
import java.util.List;
public class C {
    public void use(List<String> names) {
        names.forEach(name -> System.out.println(name));
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "name ->");
    let resp = educator.query_position(&file, line, col).unwrap();

    assert_eq!(
        resp.cursor_kind.as_deref(),
        Some("lambda_expression"),
        "cursor_kind should be the innermost construct under the cursor"
    );

    let lambda = resp
        .lessons
        .iter()
        .find(|l| l.lesson_id == "lambda-expression-fundamentals")
        .expect("lambda lesson must fire");
    assert_eq!(lambda.attached_to, "lambda_expression");
    assert_eq!(lambda.attachment, Attachment::Direct);

    let method = resp
        .lessons
        .iter()
        .find(|l| l.lesson_id == "method-declaration-fundamentals")
        .expect("method lesson must fire as ancestor context");
    assert_eq!(method.attached_to, "method_declaration");
    assert_eq!(method.attachment, Attachment::Ancestor);
}

#[test]
fn rule_attachment_marks_ancestor_when_match_is_on_enclosing_construct() {
    // Cursor on a `@Resource` annotation inside a class: the
    // prefer-constructor-injection rule attaches directly to the annotation;
    // the class-level default-package rule (if it fired) would be marked
    // ancestor. We assert the direct case here; ancestor coverage is in the
    // lesson_attachment_* test above.
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("rule-attachment-direct");
    let source = r#"package com.example;
import javax.annotation.Resource;
public class OrderService {
    @Resource
    private OrderRepository repository;
}
"#;
    let file = tmp.write("OrderService.java", source);
    let (line, col) = locate(source, "@Resource");
    let resp = educator.query_position(&file, line, col).unwrap();

    let hit = resp
        .specific
        .iter()
        .find(|r| r.rule_id == "prefer-constructor-injection")
        .expect("prefer-constructor-injection must fire");
    assert_eq!(hit.attached_to, "annotation");
    assert_eq!(hit.attachment, Attachment::Direct);
    assert_eq!(resp.cursor_kind.as_deref(), Some("annotation"));
}

#[test]
fn repo_corpus_lambda_lesson_fires() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("repo-lesson-lambda");
    let source = r#"package com.example;
import java.util.List;
public class C {
    public void use(List<String> names) {
        names.forEach(name -> System.out.println(name));
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "name ->");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        resp.lessons
            .iter()
            .any(|l| l.lesson_id == "lambda-expression-fundamentals"),
        "expected lambda-expression-fundamentals lesson, got: {:?}",
        resp.lessons.iter().map(|l| &l.lesson_id).collect::<Vec<_>>()
    );
}

#[test]
fn repo_corpus_annotation_lesson_fires() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("repo-lesson-annotation");
    let source = r#"package com.example;
public class C {
    @Override
    public String toString() { return "C"; }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "@Override");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        resp.lessons
            .iter()
            .any(|l| l.lesson_id == "annotation-fundamentals"),
        "expected annotation-fundamentals lesson, got: {:?}",
        resp.lessons.iter().map(|l| &l.lesson_id).collect::<Vec<_>>()
    );
}

// ─── End of lesson tests ─────────────────────────────────────────────────────

#[test]
fn repo_corpus_loads_without_validator_errors() {
    let educator = load_repo_corpus();
    let errors: Vec<_> = educator
        .issues()
        .iter()
        .filter(|i| i.severity == LoadIssueSeverity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "repo corpus must not produce validator errors: {:#?}",
        errors
    );
}

#[test]
fn unfamiliar_kind_is_warning_not_error() {
    let tmp = TmpDir::new("badkind-warn");
    let body = r#"---
id: oddkind
language: java
applies-to: [synchronized_statement]
severity: warning
kind: weird
---

# Hello

## Why
why
"#;
    tmp.write("content/java/rules/oddkind.md", body);

    let educator = Educator::load(&tmp.0.join("content")).unwrap();
    assert_eq!(educator.rules().len(), 1, "rule must be kept");
    assert!(
        educator
            .issues()
            .iter()
            .any(|i| i.severity == LoadIssueSeverity::Warning
                && i.field.as_deref() == Some("kind"))
    );
}

#[test]
fn fires_specific_on_synchronized_this() {
    let tmp = TmpDir::new("sync-this");
    tmp.write("content/java/rules/synchronized-on-this.md", SYNC_THIS_RULE);
    let educator = Educator::load(&tmp.0.join("content")).unwrap();

    let java_src = java_file("        synchronized (this) { x = 1; }");
    let file = tmp.write("Account.java", &java_src);
    let (line, col) = locate(&java_src, "synchronized");

    let resp = educator.query_position(&file, line, col).unwrap();

    assert_eq!(
        resp.specific.len(),
        1,
        "expected 1 specific hit, got {} (stack: {:?})",
        resp.specific.len(),
        resp.stack
    );
    assert_eq!(resp.specific[0].rule_id, "synchronized-on-this");
    assert!(resp.general.is_empty());
}

#[test]
fn does_not_fire_specific_on_synchronized_named_lock() {
    let tmp = TmpDir::new("sync-lock");
    tmp.write("content/java/rules/synchronized-on-this.md", SYNC_THIS_RULE);
    let educator = Educator::load(&tmp.0.join("content")).unwrap();

    let java_src = java_file("        synchronized (lock) { x = 1; }");
    let file = tmp.write("Account.java", &java_src);
    let (line, col) = locate(&java_src, "synchronized");

    let resp = educator.query_position(&file, line, col).unwrap();

    assert!(
        resp.specific.is_empty(),
        "rule must not fire when lock is not `this`: {:?}",
        resp.specific
    );
}

#[test]
fn empty_corpus_returns_no_contribution() {
    let tmp = TmpDir::new("empty");
    let educator = Educator::empty();

    let java_src = java_file("        synchronized (this) { x = 1; }");
    let file = tmp.write("Account.java", &java_src);
    let (line, col) = locate(&java_src, "synchronized");

    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(resp.specific.is_empty());
    assert!(resp.general.is_empty());
}

#[test]
fn ancestor_stack_includes_class_and_method() {
    let tmp = TmpDir::new("ancestors");
    tmp.write("content/java/rules/synchronized-on-this.md", SYNC_THIS_RULE);
    let educator = Educator::load(&tmp.0.join("content")).unwrap();

    let java_src = java_file("        synchronized (this) { x = 1; }");
    let file = tmp.write("Account.java", &java_src);
    let (line, col) = locate(&java_src, "synchronized");

    let resp = educator.query_position(&file, line, col).unwrap();
    let kinds: Vec<&str> = resp.stack.iter().map(|c| c.kind.as_str()).collect();
    assert!(kinds.contains(&"synchronized_statement"), "kinds: {:?}", kinds);
    assert!(kinds.contains(&"method_declaration"), "kinds: {:?}", kinds);
    assert!(kinds.contains(&"class_declaration"), "kinds: {:?}", kinds);
}

#[test]
fn non_java_file_returns_unknown_language() {
    let educator = Educator::empty();
    let resp = educator
        .query_position(std::path::Path::new("foo.py"), 0, 0)
        .unwrap();
    assert_eq!(resp.language, "unknown");
}

#[test]
fn scan_returns_hits_sorted_by_line() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("scan-sorted");
    let source = r#"package com.example;
import java.util.Vector;
import java.util.Date;

public class C {
    private Vector<String> v;
    private Date created;

    public synchronized void a() {
        System.out.println("hi");
        synchronized (this) {}
    }
}
"#;
    let file = tmp.write("C.java", source);
    let response = educator.scan_file(&file).unwrap();

    assert!(
        !response.hits.is_empty(),
        "expected at least one hit on a multi-anti-pattern file"
    );

    // Sorted by (line, col, rule_id).
    let mut prev = (0u32, 0u32, String::new());
    for hit in &response.hits {
        let key = (hit.line, hit.col, hit.rule_id.clone());
        assert!(key >= prev, "hits are not sorted: {:?} after {:?}", key, prev);
        prev = key;
    }

    // Expected rules to fire somewhere: legacy-thread-safe-collections (Vector),
    // legacy-date-time (Date), system-out-println (println), synchronized-method-on-this
    // (the `synchronized void`), synchronized-on-this (the `synchronized(this)` block).
    let ids: std::collections::HashSet<&str> =
        response.hits.iter().map(|h| h.rule_id.as_str()).collect();
    for expected in [
        "legacy-thread-safe-collections",
        "legacy-date-time",
        "system-out-println",
        "synchronized-method-on-this",
        "synchronized-on-this",
    ] {
        assert!(
            ids.contains(expected),
            "expected `{}` to fire, got: {:?}",
            expected,
            ids
        );
    }
}

#[test]
fn scan_non_java_returns_unknown() {
    let educator = Educator::empty();
    let resp = educator
        .scan_file(std::path::Path::new("foo.py"))
        .unwrap();
    assert_eq!(resp.language, "unknown");
    assert!(resp.hits.is_empty());
}

#[test]
fn scan_clean_file_has_zero_hits() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("scan-clean");
    let source = r#"package com.example;
public class C {
    public int parse(String s) {
        return Integer.parseInt(s);
    }
}
"#;
    let file = tmp.write("C.java", source);
    let response = educator.scan_file(&file).unwrap();
    assert_eq!(
        response.hits.len(),
        0,
        "clean file should produce no hits, got: {:?}",
        response.hits.iter().map(|h| &h.rule_id).collect::<Vec<_>>()
    );
}

#[test]
fn java_catalog_on_disk_matches_registry() {
    // Pins `content/java/construct-kinds.md` to the [`java::JAVA_CONSTRUCT_KINDS`]
    // registry. If you add a kind/attribute and forget to regenerate, this test
    // tells you which command to run.
    let specs = catalog::for_language("java").expect("java registered");
    let expected = catalog::render_catalog("java", specs);
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("content/java/construct-kinds.md");
    let actual = fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!("could not read {}: {} — run `nao construct-kinds java`", path.display(), e)
    });
    assert_eq!(
        actual, expected,
        "{} is out of date — run `nao construct-kinds java` to regenerate",
        path.display(),
    );
}

#[test]
fn java_index_on_disk_matches_renderer() {
    // Pins `content/java/INDEX.md` to whatever the renderer produces from the
    // currently-shipped rule and lesson corpus. Add or remove a rule/lesson
    // without regenerating → this test names the exact command to run.
    let educator = load_repo_corpus();
    let expected = super::index::render_index(&educator, "java");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("content/java/INDEX.md");
    let actual = fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!("could not read {}: {} — run `nao educator-index java`", path.display(), e)
    });
    assert_eq!(
        actual, expected,
        "{} is out of date — run `nao educator-index java` to regenerate",
        path.display(),
    );
}

#[test]
fn java_catalog_documents_every_extracted_kind() {
    // The runtime extractor and the documentation registry should not drift.
    // Anything `extract()` recognizes must appear in the registry so authors
    // can discover it (and EDU-004's validator can reject typos against it).
    let specs = catalog::for_language("java").expect("java registered");
    let registered: std::collections::HashSet<&str> = specs.iter().map(|s| s.kind).collect();

    for kind in EXTRACTOR_RECOGNIZED_KINDS {
        assert!(
            registered.contains(kind),
            "extractor recognizes `{}` but it is not in JAVA_CONSTRUCT_KINDS",
            kind
        );
    }
}

/// Mirror of the match arms inside `java::extract`. Kept in lockstep with that
/// function by the test above; bump both when the extractor learns a new kind.
const EXTRACTOR_RECOGNIZED_KINDS: &[&str] = &[
    "synchronized_statement",
    "method_declaration",
    "class_declaration",
    "equality_expression",
    "method_invocation",
    "declared_type",
    "field_declaration",
    "try_statement",
    "import_declaration",
    "annotation",
    "lambda_expression",
    "local_variable_declaration",
    "primitive_type",
    "for_statement",
    "if_statement",
    "switch_statement",
    "while_statement",
    "do_statement",
    "array_creation_expression",
    "constructor_declaration",
    "interface_declaration",
    "instanceof_expression",
    "throw_statement",
    "catch_clause",
    "ternary_expression",
    "type_parameters",
    "enum_declaration",
    "method_reference",
    "record_declaration",
    "text_block",
];

// ─── annotation construct + lesson ───────────────────────────────────────────

#[test]
fn annotation_lesson_fires_on_resource() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("annotation-lesson");
    let source = r#"package com.example;
import javax.annotation.Resource;
public class Service {
    @Resource
    private Object dep;
}
"#;
    let file = tmp.write("Service.java", source);
    let (line, col) = locate(source, "@Resource");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        resp.lessons.iter().any(|l| l.lesson_id == "annotation-fundamentals"),
        "expected annotation-fundamentals lesson to fire, lessons={:?}",
        resp.lessons.iter().map(|l| &l.lesson_id).collect::<Vec<_>>()
    );
}

#[test]
fn annotation_marker_form_extracts_correctly() {
    // tree-sitter-java distinguishes `@Override` (marker_annotation) from
    // `@SuppressWarnings("x")` (annotation). Both must surface as construct
    // kind `annotation` with `is_marker` reflecting the difference.
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("annotation-forms");
    let source = r#"package com.example;
public class C {
    @Override
    public String toString() { return "C"; }
    @SuppressWarnings("unchecked")
    public Object raw() { return null; }
}
"#;
    let file = tmp.write("C.java", source);

    let (line_m, col_m) = locate(source, "@Override");
    let stack_m = educator.query_position(&file, line_m, col_m).unwrap().stack;
    let marker = stack_m
        .iter()
        .find(|i| i.kind == "annotation")
        .expect("@Override must surface as annotation");
    assert_eq!(marker.attrs.get("name").map(String::as_str), Some("Override"));
    assert_eq!(marker.attrs.get("is_marker").map(String::as_str), Some("true"));

    let (line_a, col_a) = locate(source, "@SuppressWarnings");
    let stack_a = educator.query_position(&file, line_a, col_a).unwrap().stack;
    let annot = stack_a
        .iter()
        .find(|i| i.kind == "annotation")
        .expect("@SuppressWarnings must surface as annotation");
    assert_eq!(annot.attrs.get("name").map(String::as_str), Some("SuppressWarnings"));
    assert_eq!(annot.attrs.get("is_marker").map(String::as_str), Some("false"));
}

// ─── local_variable_declaration + primitive_type ─────────────────────────────

#[test]
fn cursor_on_boolean_keyword_surfaces_primitive_type_and_local_decl() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("primitive-cursor");
    let source = r#"package com.example;
public class C {
    public void use() {
        final boolean wawiActive = true;
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "boolean");
    let resp = educator.query_position(&file, line, col).unwrap();

    // The cursor's innermost recognized construct should now be the
    // primitive type itself, not the enclosing method declaration.
    assert_eq!(resp.cursor_kind.as_deref(), Some("primitive_type"));
    assert_eq!(resp.cursor_text.as_deref(), Some("boolean"));

    // local_variable_declaration must be an ancestor in the stack.
    let kinds: Vec<&str> = resp.stack.iter().map(|c| c.kind.as_str()).collect();
    assert!(
        kinds.contains(&"local_variable_declaration"),
        "expected local_variable_declaration on the stack, got: {:?}",
        kinds
    );
}

#[test]
fn local_variable_declaration_carries_type_and_final() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("local-var-attrs");
    let source = r#"package com.example;
public class C {
    public void use() {
        final boolean wawiActive = true;
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "wawiActive");
    let resp = educator.query_position(&file, line, col).unwrap();

    let decl = resp
        .stack
        .iter()
        .find(|c| c.kind == "local_variable_declaration")
        .expect("local_variable_declaration in stack");
    assert_eq!(decl.attrs.get("type_name").map(String::as_str), Some("boolean"));
    assert_eq!(decl.attrs.get("is_final").map(String::as_str), Some("true"));
}

#[test]
fn local_variable_declaration_picks_up_generic_head_only() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("generic-local-var");
    let source = r#"package com.example;
import java.util.HashMap;
import java.util.Map;
public class C {
    public void use() {
        Map<String, Integer> counts = new HashMap<>();
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "counts");
    let resp = educator.query_position(&file, line, col).unwrap();
    let decl = resp
        .stack
        .iter()
        .find(|c| c.kind == "local_variable_declaration")
        .expect("local_variable_declaration in stack");
    assert_eq!(
        decl.attrs.get("type_name").map(String::as_str),
        Some("Map"),
        "type_name should be the generic head, not the full parameterised type"
    );
}

// ─── for_statement construct ─────────────────────────────────────────────────

#[test]
fn classic_for_loop_recognised_with_classic_style() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("classic-for");
    let source = r#"package com.example;
public class C {
    public void use() {
        for (int i = 0; i < 10; i++) {
            System.out.println(i);
        }
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "for (int i");
    let resp = educator.query_position(&file, line, col).unwrap();
    let for_node = resp
        .stack
        .iter()
        .find(|c| c.kind == "for_statement")
        .expect("for_statement in stack");
    assert_eq!(
        for_node.attrs.get("style").map(String::as_str),
        Some("classic"),
    );
    assert!(
        !for_node.attrs.contains_key("element_type"),
        "classic loops should not carry element_type"
    );
}

#[test]
fn enhanced_for_loop_extracts_element_type_and_name() {
    // The exact case the user reported: `for (final ProductPromotionItemModel
    // promoItem : promoItems) { … }`. The construct must be detected with
    // `style: enhanced` and the type/name/iterable captured for matching.
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("enhanced-for");
    let source = r#"package com.example;
import java.util.List;
public class C {
    public void use(List<ProductPromotionItemModel> promoItems) {
        for (final ProductPromotionItemModel promoItem : promoItems) {
            process(promoItem);
        }
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "for (final");
    let resp = educator.query_position(&file, line, col).unwrap();
    let for_node = resp
        .stack
        .iter()
        .find(|c| c.kind == "for_statement")
        .expect("for_statement in stack");
    assert_eq!(
        for_node.attrs.get("style").map(String::as_str),
        Some("enhanced"),
    );
    assert_eq!(
        for_node.attrs.get("element_type").map(String::as_str),
        Some("ProductPromotionItemModel"),
    );
    assert_eq!(
        for_node.attrs.get("element_name").map(String::as_str),
        Some("promoItem"),
    );
    assert_eq!(
        for_node.attrs.get("iterable_text").map(String::as_str),
        Some("promoItems"),
    );
}

// ─── if_statement construct + rule + lesson ──────────────────────────────────

#[test]
fn if_statement_recognised_with_braced_body() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("if-braced");
    let source = r#"package com.example;
public class C {
    public void use(int x) {
        if (x > 0) {
            System.out.println("positive");
        }
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "if (x > 0)");
    let resp = educator.query_position(&file, line, col).unwrap();
    let if_node = resp
        .stack
        .iter()
        .find(|c| c.kind == "if_statement")
        .expect("if_statement in stack");
    assert_eq!(
        if_node.attrs.get("consequence_is_block").map(String::as_str),
        Some("true"),
    );
    assert_eq!(
        if_node.attrs.get("else_kind").map(String::as_str),
        Some("none"),
    );
    assert_eq!(
        if_node.attrs.get("has_else").map(String::as_str),
        Some("false"),
    );
}

#[test]
fn if_without_braces_fires_on_single_statement_then() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("if-no-braces");
    let source = r#"package com.example;
public class C {
    public int divide(int n, int d) {
        if (d == 0)
            throw new ArithmeticException("divide by zero");
        return n / d;
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "if (d == 0)");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        resp.specific.iter().any(|r| r.rule_id == "if-without-braces"),
        "expected if-without-braces to fire, specific={:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
}

#[test]
fn if_without_braces_silent_when_braced() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("if-braced-silent");
    let source = r#"package com.example;
public class C {
    public int divide(int n, int d) {
        if (d == 0) {
            throw new ArithmeticException("divide by zero");
        }
        return n / d;
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "if (d == 0)");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        !resp.specific.iter().any(|r| r.rule_id == "if-without-braces"),
        "braced if must not fire if-without-braces"
    );
}

#[test]
fn else_if_chain_detected_as_else_kind_if() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("else-if-chain");
    let source = r#"package com.example;
public class C {
    public String use(int x) {
        if (x > 0) {
            return "positive";
        } else if (x < 0) {
            return "negative";
        } else {
            return "zero";
        }
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "if (x > 0)");
    let resp = educator.query_position(&file, line, col).unwrap();
    let outer = resp
        .stack
        .iter()
        .find(|c| c.kind == "if_statement")
        .expect("outer if_statement in stack");
    assert_eq!(
        outer.attrs.get("else_kind").map(String::as_str),
        Some("if"),
        "outer if's else-branch is a chained else-if",
    );
    assert_eq!(
        outer.attrs.get("has_else").map(String::as_str),
        Some("true"),
    );
}

#[test]
fn else_without_braces_fires_on_single_statement_else() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("else-no-braces");
    let source = r#"package com.example;
public class C {
    public String classify(int x) {
        if (x > 0) {
            return "positive";
        } else
            return "non-positive";
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "if (x > 0)");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        resp.specific.iter().any(|r| r.rule_id == "else-without-braces"),
        "expected else-without-braces, specific={:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
}

#[test]
fn else_without_braces_silent_on_chained_else_if() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("else-if-silent");
    let source = r#"package com.example;
public class C {
    public String classify(int x) {
        if (x > 0) {
            return "positive";
        } else if (x < 0) {
            return "negative";
        } else {
            return "zero";
        }
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "if (x > 0)");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        !resp.specific.iter().any(|r| r.rule_id == "else-without-braces"),
        "else-if chain must not fire else-without-braces"
    );
}

#[test]
fn nested_if_could_be_and_fires() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("nested-if");
    let source = r#"package com.example;
public class C {
    public boolean canRetry(Order order) {
        if (order.isActive()) {
            if (order.hasRemainingAttempts()) {
                return true;
            }
        }
        return false;
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "if (order.isActive())");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        resp.specific.iter().any(|r| r.rule_id == "nested-if-could-be-and"),
        "expected nested-if-could-be-and, specific={:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
}

#[test]
fn nested_if_silent_when_outer_has_else() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("nested-if-with-else");
    let source = r#"package com.example;
public class C {
    public void use(Order order) {
        if (order.isActive()) {
            if (order.hasRemainingAttempts()) {
                retry();
            }
        } else {
            fallback();
        }
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "if (order.isActive())");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        !resp.specific.iter().any(|r| r.rule_id == "nested-if-could-be-and"),
        "merge changes semantics when outer has else — must not fire"
    );
}

#[test]
fn nested_if_silent_when_inner_has_else() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("nested-if-inner-else");
    let source = r#"package com.example;
public class C {
    public void use(Order order) {
        if (order.isActive()) {
            if (order.hasRemainingAttempts()) {
                retry();
            } else {
                stop();
            }
        }
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "if (order.isActive())");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        !resp.specific.iter().any(|r| r.rule_id == "nested-if-could-be-and"),
        "inner with else can't be merged via && — must not fire"
    );
}

#[test]
fn if_condition_complexity_bucketed_by_operator_count() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("if-condition-complexity");
    let source = r#"package com.example;
public class C {
    public void simple(int x) {
        if (x > 0) { foo(); }
    }
    public void compound(boolean a, boolean b) {
        if (a && b) { foo(); }
    }
    public void longChain(boolean a, boolean b, boolean c, boolean d) {
        if (a && b && c && d) { foo(); }
    }
}
"#;
    let file = tmp.write("C.java", source);

    let (line, col) = locate(source, "if (x > 0)");
    let resp = educator.query_position(&file, line, col).unwrap();
    let if_node = resp
        .stack
        .iter()
        .find(|c| c.kind == "if_statement")
        .expect("if_statement in stack");
    assert_eq!(
        if_node.attrs.get("condition_complexity").map(String::as_str),
        Some("simple"),
    );

    let (line, col) = locate(source, "if (a && b)");
    let resp = educator.query_position(&file, line, col).unwrap();
    let if_node = resp
        .stack
        .iter()
        .find(|c| c.kind == "if_statement")
        .expect("if_statement in stack");
    assert_eq!(
        if_node.attrs.get("condition_complexity").map(String::as_str),
        Some("compound"),
    );

    let (line, col) = locate(source, "if (a && b && c && d)");
    let resp = educator.query_position(&file, line, col).unwrap();
    let if_node = resp
        .stack
        .iter()
        .find(|c| c.kind == "if_statement")
        .expect("if_statement in stack");
    assert_eq!(
        if_node.attrs.get("condition_complexity").map(String::as_str),
        Some("long"),
    );
}

#[test]
fn lift_boolean_into_explaining_variable_fires_on_long_condition() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("lift-boolean-long");
    let source = r#"package com.example;
public class C {
    public void mark(Order order, Config config, Status status, boolean forced) {
        if (!forced
                && config.isFilterOldSequence()
                && status.getLastSequence() != null
                && status.getLastSequence().compareTo(order.getSequence()) >= 0) {
            markFiltered(order, status);
        }
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "if (!forced");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        resp.specific
            .iter()
            .any(|r| r.rule_id == "lift-boolean-into-explaining-variable"),
        "expected lift-boolean-into-explaining-variable, specific={:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
}

#[test]
fn lift_boolean_into_explaining_variable_silent_on_short_condition() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("lift-boolean-short");
    let source = r#"package com.example;
public class C {
    public void check(Order order, boolean forced) {
        if (!forced && order.isActive()) {
            apply(order);
        }
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "if (!forced");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        !resp.specific
            .iter()
            .any(|r| r.rule_id == "lift-boolean-into-explaining-variable"),
        "two-operand condition is below the long threshold — must not fire"
    );
}

#[test]
fn redundant_else_after_return_fires() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("redundant-else-return");
    let source = r#"package com.example;
public class C {
    public int abs(int x) {
        if (x < 0) {
            return -x;
        } else {
            return x;
        }
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "if (x < 0)");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        resp.specific.iter().any(|r| r.rule_id == "redundant-else-after-return"),
        "expected redundant-else-after-return, specific={:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
}

#[test]
fn redundant_else_after_throw_fires() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("redundant-else-throw");
    let source = r#"package com.example;
public class C {
    public Object place(Object order) {
        if (order == null) {
            throw new IllegalArgumentException("null");
        } else {
            return order;
        }
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "if (order == null)");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        resp.specific.iter().any(|r| r.rule_id == "redundant-else-after-return"),
        "expected redundant-else-after-return on throw, specific={:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
}

#[test]
fn redundant_else_silent_when_then_does_not_terminate() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("else-not-redundant");
    let source = r#"package com.example;
public class C {
    public int abs(int x) {
        int sign;
        if (x < 0) {
            sign = -1;
        } else {
            sign = 1;
        }
        return sign * x;
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "if (x < 0)");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        !resp.specific.iter().any(|r| r.rule_id == "redundant-else-after-return"),
        "non-terminating then-branch must not fire redundant-else-after-return"
    );
}

#[test]
fn if_statement_lesson_fires() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("if-lesson");
    let source = r#"package com.example;
public class C {
    public void use(int x) {
        if (x > 0) {
            return;
        }
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "if (x > 0)");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        resp.lessons.iter().any(|l| l.lesson_id == "if-statement-fundamentals"),
        "expected if-statement-fundamentals lesson, lessons={:?}",
        resp.lessons.iter().map(|l| &l.lesson_id).collect::<Vec<_>>()
    );
}

#[test]
fn for_statement_lesson_fires_on_either_form() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("for-lesson");
    let source = r#"package com.example;
import java.util.List;
public class C {
    public void use(List<String> xs) {
        for (String x : xs) {
            System.out.println(x);
        }
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "for (String");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        resp.lessons.iter().any(|l| l.lesson_id == "for-statement-fundamentals"),
        "expected for-statement-fundamentals lesson, lessons={:?}",
        resp.lessons.iter().map(|l| &l.lesson_id).collect::<Vec<_>>()
    );
}

// ─── switch_statement construct + lesson + rule ──────────────────────────────

#[test]
fn switch_classic_colon_style_extracted_and_rule_fires() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("switch-classic");
    let source = r#"package com.example;
public class C {
    public String dayType(String day) {
        switch (day) {
            case "MON":
            case "TUE":
                return "weekday";
            default:
                return "unknown";
        }
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "switch (day)");
    let resp = educator.query_position(&file, line, col).unwrap();

    let sw = resp
        .stack
        .iter()
        .find(|c| c.kind == "switch_statement")
        .expect("switch_statement in stack");
    assert_eq!(sw.attrs.get("case_style").map(String::as_str), Some("colon"));
    assert_eq!(sw.attrs.get("has_default").map(String::as_str), Some("true"));

    assert!(
        resp.specific.iter().any(|r| r.rule_id == "switch-classic-colon-style"),
        "expected switch-classic-colon-style rule, got: {:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
    assert!(
        resp.lessons.iter().any(|l| l.lesson_id == "switch-statement-fundamentals"),
        "expected switch-statement-fundamentals lesson"
    );
}

#[test]
fn switch_arrow_style_does_not_fire_classic_rule() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("switch-arrow");
    let source = r#"package com.example;
public class C {
    public String dayType(String day) {
        return switch (day) {
            case "MON", "TUE" -> "weekday";
            default -> "unknown";
        };
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "switch (day)");
    let resp = educator.query_position(&file, line, col).unwrap();
    let sw = resp
        .stack
        .iter()
        .find(|c| c.kind == "switch_statement")
        .expect("switch_statement in stack");
    assert_eq!(sw.attrs.get("case_style").map(String::as_str), Some("arrow"));
    assert!(
        !resp.specific.iter().any(|r| r.rule_id == "switch-classic-colon-style"),
        "arrow-form switch must not trigger the classic-colon rule"
    );
}

// ─── while_statement construct + lesson + rule ───────────────────────────────

#[test]
fn while_true_recognised_and_rule_fires() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("while-true");
    let source = r#"package com.example;
public class C {
    public void loop() {
        while (true) {
            if (done()) break;
        }
    }
    boolean done() { return false; }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "while (true)");
    let resp = educator.query_position(&file, line, col).unwrap();

    let wh = resp
        .stack
        .iter()
        .find(|c| c.kind == "while_statement")
        .expect("while_statement in stack");
    assert_eq!(
        wh.attrs.get("condition_is_literal_true").map(String::as_str),
        Some("true"),
    );

    assert!(
        resp.specific.iter().any(|r| r.rule_id == "while-true-needs-clear-exit"),
        "expected while-true-needs-clear-exit, got: {:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
    assert!(
        resp.lessons.iter().any(|l| l.lesson_id == "while-statement-fundamentals"),
        "expected while-statement-fundamentals lesson"
    );
}

#[test]
fn while_with_condition_does_not_fire_true_rule() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("while-cond");
    let source = r#"package com.example;
public class C {
    public void loop(int n) {
        while (n > 0) {
            n--;
        }
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "while (n > 0)");
    let resp = educator.query_position(&file, line, col).unwrap();
    let wh = resp
        .stack
        .iter()
        .find(|c| c.kind == "while_statement")
        .expect("while_statement in stack");
    assert_eq!(
        wh.attrs.get("condition_is_literal_true").map(String::as_str),
        Some("false"),
    );
    assert!(
        !resp.specific.iter().any(|r| r.rule_id == "while-true-needs-clear-exit"),
        "loops with real conditions must not trigger the while(true) rule"
    );
}

// ─── do_statement construct + lesson + rule ──────────────────────────────────

#[test]
fn do_statement_recognised_and_rule_fires() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("do-while");
    let source = r#"package com.example;
public class C {
    public void loop(int n) {
        do {
            n--;
        } while (n > 0);
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "do {");
    let resp = educator.query_position(&file, line, col).unwrap();

    assert!(
        resp.stack.iter().any(|c| c.kind == "do_statement"),
        "do_statement must surface in stack: {:?}",
        resp.stack.iter().map(|c| &c.kind).collect::<Vec<_>>(),
    );
    // `do-while-discouraged` has no `match:` clause — it teaches whenever a
    // do_statement is in scope, so the rule arrives via the General bucket.
    assert!(
        resp.general.iter().any(|r| r.rule_id == "do-while-discouraged"),
        "expected do-while-discouraged in general bucket, got: {:?}",
        resp.general.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
    assert!(
        resp.lessons.iter().any(|l| l.lesson_id == "do-statement-fundamentals"),
        "expected do-statement-fundamentals lesson"
    );
}

// ─── array_creation_expression construct + lesson + rule ─────────────────────

#[test]
fn array_literal_recognised_and_list_rule_fires() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("array-literal");
    let source = r#"package com.example;
public class C {
    public String[] days() {
        return new String[]{"SAT", "SUN"};
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "new String[]");
    let resp = educator.query_position(&file, line, col).unwrap();

    let arr = resp
        .stack
        .iter()
        .find(|c| c.kind == "array_creation_expression")
        .expect("array_creation_expression in stack");
    assert_eq!(arr.attrs.get("has_initializer").map(String::as_str), Some("true"));
    assert_eq!(arr.attrs.get("element_type").map(String::as_str), Some("String"));

    assert!(
        resp.specific.iter().any(|r| r.rule_id == "prefer-list-of-over-array-literal"),
        "expected prefer-list-of-over-array-literal rule"
    );
    assert!(
        resp.lessons.iter().any(|l| l.lesson_id == "array-creation-fundamentals"),
        "expected array-creation-fundamentals lesson"
    );
}

#[test]
fn sized_array_does_not_fire_list_rule() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("array-sized");
    let source = r#"package com.example;
public class C {
    public int[] zeros() {
        return new int[5];
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "new int[5]");
    let resp = educator.query_position(&file, line, col).unwrap();
    let arr = resp
        .stack
        .iter()
        .find(|c| c.kind == "array_creation_expression")
        .expect("array_creation_expression in stack");
    assert_eq!(arr.attrs.get("has_initializer").map(String::as_str), Some("false"));
    assert!(
        !resp.specific.iter().any(|r| r.rule_id == "prefer-list-of-over-array-literal"),
        "sized-only array must not trigger the List.of rule"
    );
}

// ─── constructor_declaration construct + lesson + rule ───────────────────────

#[test]
fn constructor_with_many_parameters_fires_rule() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("ctor-many");
    let source = r#"package com.example;
public class C {
    public C(int a, int b, int c, int d, int e, int f) {
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "public C(int a");
    let resp = educator.query_position(&file, line, col).unwrap();

    let ctor = resp
        .stack
        .iter()
        .find(|c| c.kind == "constructor_declaration")
        .expect("constructor_declaration in stack");
    assert_eq!(ctor.attrs.get("name").map(String::as_str), Some("C"));
    assert_eq!(ctor.attrs.get("parameter_count").map(String::as_str), Some("6"));
    assert_eq!(ctor.attrs.get("visibility").map(String::as_str), Some("public"));

    assert!(
        resp.specific.iter().any(|r| r.rule_id == "constructor-too-many-parameters"),
        "expected constructor-too-many-parameters rule"
    );
    assert!(
        resp.lessons.iter().any(|l| l.lesson_id == "constructor-declaration-fundamentals"),
        "expected constructor-declaration-fundamentals lesson"
    );
}

#[test]
fn constructor_with_delegation_detected() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("ctor-delegates");
    let source = r#"package com.example;
public class C {
    public C(String s) {
        this(s, 0);
    }
    public C(String s, int n) {
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "public C(String s)");
    let resp = educator.query_position(&file, line, col).unwrap();
    let ctor = resp
        .stack
        .iter()
        .find(|c| c.kind == "constructor_declaration")
        .expect("constructor_declaration in stack");
    assert_eq!(ctor.attrs.get("delegates_to").map(String::as_str), Some("this"));
    assert!(
        !resp.specific.iter().any(|r| r.rule_id == "constructor-too-many-parameters"),
        "1-arg constructor must not trigger the many-params rule"
    );
}

// ─── interface_declaration construct + lesson + rule ─────────────────────────

#[test]
fn interface_with_default_methods_fires_rule() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("iface-default");
    let source = r#"package com.example;
public interface I {
    void core();
    default void helper() { core(); }
}
"#;
    let file = tmp.write("I.java", source);
    let (line, col) = locate(source, "public interface I");
    let resp = educator.query_position(&file, line, col).unwrap();

    let iface = resp
        .stack
        .iter()
        .find(|c| c.kind == "interface_declaration")
        .expect("interface_declaration in stack");
    assert_eq!(iface.attrs.get("name").map(String::as_str), Some("I"));
    assert_eq!(
        iface.attrs.get("has_default_methods").map(String::as_str),
        Some("true"),
    );

    assert!(
        resp.specific.iter().any(|r| r.rule_id == "interface-default-method-heavy"),
        "expected interface-default-method-heavy rule"
    );
    assert!(
        resp.lessons.iter().any(|l| l.lesson_id == "interface-declaration-fundamentals"),
        "expected interface-declaration-fundamentals lesson"
    );
}

#[test]
fn pure_interface_does_not_fire_default_rule() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("iface-pure");
    let source = r#"package com.example;
public interface I {
    void core();
}
"#;
    let file = tmp.write("I.java", source);
    let (line, col) = locate(source, "public interface I");
    let resp = educator.query_position(&file, line, col).unwrap();
    let iface = resp
        .stack
        .iter()
        .find(|c| c.kind == "interface_declaration")
        .expect("interface_declaration in stack");
    assert_eq!(
        iface.attrs.get("has_default_methods").map(String::as_str),
        Some("false"),
    );
    assert!(
        !resp.specific.iter().any(|r| r.rule_id == "interface-default-method-heavy"),
        "pure interface must not trigger the default-method-heavy rule"
    );
}

// ─── instanceof_expression construct + lesson + rule ─────────────────────────

#[test]
fn classic_instanceof_fires_pattern_rule() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("instanceof-classic");
    let source = r#"package com.example;
public class C {
    public String describe(Object n) {
        if (n instanceof String) {
            String s = (String) n;
            return s;
        }
        return "";
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "n instanceof String");
    let resp = educator.query_position(&file, line, col).unwrap();

    let inst = resp
        .stack
        .iter()
        .find(|c| c.kind == "instanceof_expression")
        .expect("instanceof_expression in stack");
    assert_eq!(inst.attrs.get("is_pattern").map(String::as_str), Some("false"));
    assert_eq!(inst.attrs.get("target_type").map(String::as_str), Some("String"));

    assert!(
        resp.specific.iter().any(|r| r.rule_id == "prefer-pattern-instanceof"),
        "expected prefer-pattern-instanceof rule"
    );
    assert!(
        resp.lessons.iter().any(|l| l.lesson_id == "instanceof-expression-fundamentals"),
        "expected instanceof-expression-fundamentals lesson"
    );
}

#[test]
fn pattern_instanceof_does_not_fire_rule() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("instanceof-pattern");
    let source = r#"package com.example;
public class C {
    public String describe(Object n) {
        if (n instanceof String s) {
            return s;
        }
        return "";
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "n instanceof String");
    let resp = educator.query_position(&file, line, col).unwrap();
    let inst = resp
        .stack
        .iter()
        .find(|c| c.kind == "instanceof_expression")
        .expect("instanceof_expression in stack");
    assert_eq!(inst.attrs.get("is_pattern").map(String::as_str), Some("true"));
    assert!(
        !resp.specific.iter().any(|r| r.rule_id == "prefer-pattern-instanceof"),
        "pattern form must not trigger the prefer-pattern rule"
    );
}

// ─── throw_statement construct + lesson + rule ───────────────────────────────

#[test]
fn throw_generic_runtime_exception_fires_rule() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("throw-generic");
    let source = r#"package com.example;
public class C {
    public void check(Object o) {
        if (o == null) {
            throw new RuntimeException("nope");
        }
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "throw new RuntimeException");
    let resp = educator.query_position(&file, line, col).unwrap();

    let th = resp
        .stack
        .iter()
        .find(|c| c.kind == "throw_statement")
        .expect("throw_statement in stack");
    assert_eq!(
        th.attrs.get("exception_kind").map(String::as_str),
        Some("object_creation"),
    );
    assert_eq!(
        th.attrs.get("exception_type").map(String::as_str),
        Some("RuntimeException"),
    );

    assert!(
        resp.specific.iter().any(|r| r.rule_id == "throw-generic-exception"),
        "expected throw-generic-exception rule"
    );
    assert!(
        resp.lessons.iter().any(|l| l.lesson_id == "throw-statement-fundamentals"),
        "expected throw-statement-fundamentals lesson"
    );
}

#[test]
fn throw_specific_exception_does_not_fire_rule() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("throw-specific");
    let source = r#"package com.example;
public class C {
    public void check(Object o) {
        if (o == null) {
            throw new IllegalArgumentException("nope");
        }
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "throw new IllegalArgumentException");
    let resp = educator.query_position(&file, line, col).unwrap();
    let th = resp
        .stack
        .iter()
        .find(|c| c.kind == "throw_statement")
        .expect("throw_statement in stack");
    assert_eq!(
        th.attrs.get("exception_type").map(String::as_str),
        Some("IllegalArgumentException"),
    );
    assert!(
        !resp.specific.iter().any(|r| r.rule_id == "throw-generic-exception"),
        "specific exception must not trigger the generic-exception rule"
    );
}

// ─── catch_clause construct + lesson + rule ──────────────────────────────────

#[test]
fn empty_catch_block_fires_rule() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("catch-empty");
    let source = r#"package com.example;
public class C {
    public void use() {
        try {
            risky();
        } catch (RuntimeException e) {
        }
    }
    void risky() {}
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "catch (RuntimeException");
    let resp = educator.query_position(&file, line, col).unwrap();

    let c = resp
        .stack
        .iter()
        .find(|c| c.kind == "catch_clause")
        .expect("catch_clause in stack");
    assert_eq!(c.attrs.get("is_empty").map(String::as_str), Some("true"));
    assert_eq!(c.attrs.get("is_multi_catch").map(String::as_str), Some("false"));

    assert!(
        resp.specific.iter().any(|r| r.rule_id == "empty-catch-block"),
        "expected empty-catch-block rule"
    );
    assert!(
        resp.lessons.iter().any(|l| l.lesson_id == "catch-clause-fundamentals"),
        "expected catch-clause-fundamentals lesson"
    );
}

#[test]
fn non_empty_catch_block_does_not_fire_rule() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("catch-non-empty");
    let source = r#"package com.example;
public class C {
    public void use() {
        try {
            risky();
        } catch (RuntimeException e) {
            handle(e);
        }
    }
    void risky() {}
    void handle(Throwable t) {}
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "catch (RuntimeException");
    let resp = educator.query_position(&file, line, col).unwrap();
    let c = resp
        .stack
        .iter()
        .find(|c| c.kind == "catch_clause")
        .expect("catch_clause in stack");
    assert_eq!(c.attrs.get("is_empty").map(String::as_str), Some("false"));
    assert!(
        !resp.specific.iter().any(|r| r.rule_id == "empty-catch-block"),
        "non-empty catch must not trigger the empty-catch rule"
    );
}

// ─── Module 20: text_block construct + lesson ────────────────────────────────

#[test]
fn text_block_recognised_and_lesson_fires() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("text-block");
    let source = "package com.example;\npublic class C {\n    String json = \"\"\"\n            {\n              \"id\": 42\n            }\n            \"\"\";\n}\n";
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "\"\"\"");
    let resp = educator.query_position(&file, line, col).unwrap();
    let tb = resp
        .stack
        .iter()
        .find(|c| c.kind == "text_block")
        .expect("text_block in stack");
    let lc: u32 = tb.attrs.get("line_count").and_then(|s| s.parse().ok()).unwrap_or(0);
    assert!(lc >= 4, "expected >= 4 lines, got {}", lc);
    assert!(
        resp.lessons.iter().any(|l| l.lesson_id == "text-block-fundamentals"),
        "expected text-block-fundamentals lesson"
    );
}

#[test]
fn regular_string_literal_is_not_text_block() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("regular-string");
    let source = r#"package com.example;
public class C {
    String s = "hello";
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "\"hello\"");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        !resp.stack.iter().any(|c| c.kind == "text_block"),
        "regular string must not be classified as text_block"
    );
}

// ─── Module 19: record_declaration construct + lesson + instance-field rule ──

#[test]
fn record_with_only_components_does_not_fire_rule() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("record-pure");
    let source = r#"package com.example;
public record Point(int x, int y) {}
"#;
    let file = tmp.write("Point.java", source);
    let (line, col) = locate(source, "record Point");
    let resp = educator.query_position(&file, line, col).unwrap();
    let r = resp
        .stack
        .iter()
        .find(|c| c.kind == "record_declaration")
        .expect("record_declaration in stack");
    assert_eq!(r.attrs.get("name").map(String::as_str), Some("Point"));
    assert_eq!(r.attrs.get("component_count").map(String::as_str), Some("2"));
    assert_eq!(
        r.attrs.get("has_instance_field").map(String::as_str),
        Some("false"),
    );
    assert!(
        !resp.specific.iter().any(|r| r.rule_id == "record-with-instance-field"),
        "header-only record must not trigger the rule"
    );
    assert!(
        resp.lessons.iter().any(|l| l.lesson_id == "record-fundamentals"),
        "expected record-fundamentals lesson"
    );
}

#[test]
fn record_with_static_constant_is_ok() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("record-static");
    let source = r#"package com.example;
public record Point(int x, int y) {
    public static final Point ORIGIN = new Point(0, 0);
}
"#;
    let file = tmp.write("Point.java", source);
    let (line, col) = locate(source, "record Point");
    let resp = educator.query_position(&file, line, col).unwrap();
    let r = resp
        .stack
        .iter()
        .find(|c| c.kind == "record_declaration")
        .expect("record_declaration in stack");
    assert_eq!(
        r.attrs.get("has_instance_field").map(String::as_str),
        Some("false"),
        "static fields must not count as instance fields"
    );
}

// ─── Module 18: optional-as-field-type rule ──────────────────────────────────

#[test]
fn optional_as_field_type_fires() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("opt-field");
    let source = r#"package com.example;
import java.util.Optional;
public class C {
    private final Optional<String> middleName;
    public C(Optional<String> m) { this.middleName = m; }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "private final Optional");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        resp.specific.iter().any(|r| r.rule_id == "optional-as-field-type"),
        "expected optional-as-field-type, got: {:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
}

#[test]
fn optional_as_return_type_does_not_fire_field_rule() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("opt-return");
    let source = r#"package com.example;
import java.util.Optional;
public class C {
    private final String middleName;
    public C(String m) { this.middleName = m; }
    public Optional<String> getMiddleName() { return Optional.ofNullable(middleName); }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "private final String");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        !resp.specific.iter().any(|r| r.rule_id == "optional-as-field-type"),
        "non-Optional field must not trigger the rule"
    );
}

// ─── Module 17: streams lesson + prefer-toList rule ──────────────────────────

#[test]
fn streams_lesson_fires_on_lambda_in_pipeline() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("streams-lesson");
    let source = r#"package com.example;
import java.util.List;
public class C {
    public List<String> upper(List<String> xs) {
        return xs.stream().map(s -> s.toUpperCase()).toList();
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "s -> s.toUpperCase");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        resp.lessons.iter().any(|l| l.lesson_id == "streams-fundamentals"),
        "expected streams-fundamentals lesson"
    );
}

#[test]
fn prefer_toList_fires_on_collectors_toList() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("collectors-tolist");
    let source = r#"package com.example;
import java.util.List;
import java.util.stream.Collectors;
public class C {
    public List<String> all(List<String> xs) {
        return xs.stream().collect(Collectors.toList());
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "Collectors.toList()");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        resp.specific.iter().any(|r| r.rule_id == "prefer-toList-over-collectors-toList"),
        "expected prefer-toList-over-collectors-toList, got: {:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
}

// ─── Module 16: method_reference construct + lesson + lambda-block rule ──────

#[test]
fn method_reference_static_or_unbound_detected() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("method-ref-unbound");
    let source = r#"package com.example;
import java.util.List;
import java.util.function.Function;
public class C {
    public void use() {
        Function<String, Integer> f = String::length;
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "String::length");
    let resp = educator.query_position(&file, line, col).unwrap();
    let mr = resp
        .stack
        .iter()
        .find(|c| c.kind == "method_reference")
        .expect("method_reference in stack");
    assert_eq!(
        mr.attrs.get("reference_kind").map(String::as_str),
        Some("static_or_unbound"),
    );
    assert_eq!(mr.attrs.get("target_text").map(String::as_str), Some("length"));
    assert!(
        resp.lessons.iter().any(|l| l.lesson_id == "method-reference-fundamentals"),
        "expected method-reference-fundamentals lesson"
    );
}

#[test]
fn method_reference_constructor_detected() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("method-ref-ctor");
    let source = r#"package com.example;
import java.util.function.Function;
public class C {
    public void use() {
        Function<String, StringBuilder> f = StringBuilder::new;
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "StringBuilder::new");
    let resp = educator.query_position(&file, line, col).unwrap();
    let mr = resp
        .stack
        .iter()
        .find(|c| c.kind == "method_reference")
        .expect("method_reference in stack");
    assert_eq!(
        mr.attrs.get("reference_kind").map(String::as_str),
        Some("constructor"),
    );
    assert_eq!(mr.attrs.get("target_text").map(String::as_str), Some("new"));
}

#[test]
fn lambda_block_body_fires_extract_rule() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("lambda-block");
    let source = r#"package com.example;
import java.util.List;
public class C {
    public void use(List<String> xs) {
        xs.forEach(x -> {
            log(x);
            audit(x);
            notify(x);
        });
    }
    void log(String s) {}
    void audit(String s) {}
    void notify(String s) {}
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "x -> {");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        resp.specific.iter().any(|r| r.rule_id == "lambda-block-body-extract-method"),
        "expected lambda-block-body-extract-method, got: {:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
}

#[test]
fn expression_body_lambda_does_not_fire_extract_rule() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("lambda-expr");
    let source = r#"package com.example;
import java.util.List;
public class C {
    public void use(List<String> xs) {
        xs.forEach(x -> log(x));
    }
    void log(String s) {}
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "x -> log(x)");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        !resp.specific.iter().any(|r| r.rule_id == "lambda-block-body-extract-method"),
        "expression-body lambda must not trigger the extract-method rule"
    );
}

// ─── Module 15: enum_declaration construct + lesson + immutable-fields rule ──

#[test]
fn enum_with_mutable_field_fires_rule() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("enum-mutable");
    let source = r#"package com.example;
public enum Status {
    OK(200),
    ERR(500);

    private int code;

    Status(int code) { this.code = code; }
    public int code() { return code; }
}
"#;
    let file = tmp.write("Status.java", source);
    let (line, col) = locate(source, "public enum Status");
    let resp = educator.query_position(&file, line, col).unwrap();
    let e = resp
        .stack
        .iter()
        .find(|c| c.kind == "enum_declaration")
        .expect("enum_declaration in stack");
    assert_eq!(e.attrs.get("name").map(String::as_str), Some("Status"));
    assert_eq!(e.attrs.get("constant_count").map(String::as_str), Some("2"));
    assert_eq!(e.attrs.get("has_mutable_field").map(String::as_str), Some("true"));
    assert!(
        resp.specific.iter().any(|r| r.rule_id == "enum-prefer-immutable-fields"),
        "expected enum-prefer-immutable-fields, got: {:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
    assert!(
        resp.lessons.iter().any(|l| l.lesson_id == "enum-fundamentals"),
        "expected enum-fundamentals lesson"
    );
}

#[test]
fn enum_with_final_fields_does_not_fire_rule() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("enum-final");
    let source = r#"package com.example;
public enum Direction {
    NORTH("N"),
    SOUTH("S");

    private final String label;

    Direction(String label) { this.label = label; }
    public String label() { return label; }
}
"#;
    let file = tmp.write("Direction.java", source);
    let (line, col) = locate(source, "public enum Direction");
    let resp = educator.query_position(&file, line, col).unwrap();
    let e = resp
        .stack
        .iter()
        .find(|c| c.kind == "enum_declaration")
        .expect("enum_declaration in stack");
    assert_eq!(
        e.attrs.get("has_mutable_field").map(String::as_str),
        Some("false"),
    );
    assert!(
        !resp.specific.iter().any(|r| r.rule_id == "enum-prefer-immutable-fields"),
        "all-final-fields enum must not trigger the rule"
    );
}

// ─── Module 14: type_parameters construct + lesson + naming rule ─────────────

#[test]
fn type_parameters_conventional_names_extracted() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("type-params-ok");
    let source = r#"package com.example;
public class Box<T> {
    private final T value;
    public Box(T v) { this.value = v; }
}
"#;
    let file = tmp.write("Box.java", source);
    let (line, col) = locate(source, "<T>");
    let resp = educator.query_position(&file, line, col).unwrap();
    let tp = resp
        .stack
        .iter()
        .find(|c| c.kind == "type_parameters")
        .expect("type_parameters in stack");
    assert_eq!(tp.attrs.get("count").map(String::as_str), Some("1"));
    assert_eq!(tp.attrs.get("names").map(String::as_str), Some("T"));
    assert_eq!(
        tp.attrs.get("has_non_conventional_name").map(String::as_str),
        Some("false"),
    );
    assert_eq!(tp.attrs.get("has_bounds").map(String::as_str), Some("false"));
    assert!(
        !resp.specific.iter().any(|r| r.rule_id == "non-conventional-type-parameter-name"),
        "single-letter T must not trigger the naming rule"
    );
    assert!(
        resp.lessons.iter().any(|l| l.lesson_id == "generics-fundamentals"),
        "expected generics-fundamentals lesson"
    );
}

#[test]
fn type_parameters_long_name_fires_rule() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("type-params-long");
    let source = r#"package com.example;
public class Cache<KeyType, ValueType> {
    private KeyType k;
    private ValueType v;
}
"#;
    let file = tmp.write("Cache.java", source);
    let (line, col) = locate(source, "<KeyType");
    let resp = educator.query_position(&file, line, col).unwrap();
    let tp = resp
        .stack
        .iter()
        .find(|c| c.kind == "type_parameters")
        .expect("type_parameters in stack");
    assert_eq!(tp.attrs.get("count").map(String::as_str), Some("2"));
    assert_eq!(
        tp.attrs.get("has_non_conventional_name").map(String::as_str),
        Some("true"),
    );
    assert!(
        resp.specific.iter().any(|r| r.rule_id == "non-conventional-type-parameter-name"),
        "expected non-conventional-type-parameter-name rule"
    );
}

#[test]
fn type_parameters_bounds_detected() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("type-params-bounded");
    let source = r#"package com.example;
import java.util.List;
public class C {
    public static <T extends Number> double sum(List<T> xs) { return 0; }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "<T extends");
    let resp = educator.query_position(&file, line, col).unwrap();
    let tp = resp
        .stack
        .iter()
        .find(|c| c.kind == "type_parameters")
        .expect("type_parameters in stack");
    assert_eq!(tp.attrs.get("has_bounds").map(String::as_str), Some("true"));
}

// ─── Module 13: collections lesson + prefer-list-of-over-singleton-list ──────

#[test]
fn collections_lesson_fires_on_list_declared_type() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("collections-lesson");
    let source = r#"package com.example;
import java.util.List;
public class C {
    public void use(List<String> names) {
        process(names);
    }
    void process(List<String> xs) {}
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "List<String> names");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        resp.lessons.iter().any(|l| l.lesson_id == "collections-fundamentals"),
        "expected collections-fundamentals lesson, got: {:?}",
        resp.lessons.iter().map(|l| &l.lesson_id).collect::<Vec<_>>()
    );
}

#[test]
fn prefer_list_of_fires_on_singleton_list() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("singleton-list");
    let source = r#"package com.example;
import java.util.Collections;
import java.util.List;
public class C {
    private static final List<String> ONE = Collections.singletonList("only");
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "Collections.singletonList");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        resp.specific.iter().any(|r| r.rule_id == "prefer-list-of-over-singleton-list"),
        "expected rule, got: {:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
}

#[test]
fn prefer_list_of_silent_on_list_of() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("list-of");
    let source = r#"package com.example;
import java.util.List;
public class C {
    private static final List<String> WEEKEND = List.of("SAT", "SUN");
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "List.of");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        !resp.specific.iter().any(|r| r.rule_id == "prefer-list-of-over-singleton-list"),
        "List.of must not trigger the singleton-list rule"
    );
}

// ─── synchronized_statement lesson ───────────────────────────────────────────

#[test]
fn synchronized_statement_lesson_fires() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("sync-lesson");
    let source = r#"package com.example;
public class C {
    private final Object lock = new Object();
    public void use() {
        synchronized (lock) {
            work();
        }
    }
    void work() {}
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "synchronized (lock)");
    let resp = educator.query_position(&file, line, col).unwrap();
    assert!(
        resp.lessons.iter().any(|l| l.lesson_id == "synchronized-statement-fundamentals"),
        "expected synchronized-statement-fundamentals lesson, got: {:?}",
        resp.lessons.iter().map(|l| &l.lesson_id).collect::<Vec<_>>()
    );
}

// ─── ternary_expression construct + lesson + rule ────────────────────────────

#[test]
fn nested_ternary_fires_rule() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("ternary-nested");
    let source = r#"package com.example;
public class C {
    public String tier(int w) {
        return w < 1 ? "light" : w < 5 ? "standard" : "heavy";
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "w < 1 ?");
    let resp = educator.query_position(&file, line, col).unwrap();

    let t = resp
        .stack
        .iter()
        .find(|c| c.kind == "ternary_expression")
        .expect("ternary_expression in stack");
    assert_eq!(t.attrs.get("is_nested").map(String::as_str), Some("true"));

    assert!(
        resp.specific.iter().any(|r| r.rule_id == "nested-ternary-discouraged"),
        "expected nested-ternary-discouraged, got: {:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
    assert!(
        resp.lessons.iter().any(|l| l.lesson_id == "ternary-expression-fundamentals"),
        "expected ternary-expression-fundamentals lesson"
    );
}

#[test]
fn single_level_ternary_does_not_fire_nested_rule() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("ternary-flat");
    let source = r#"package com.example;
public class C {
    public String label(String t) {
        return t != null ? t : "untitled";
    }
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "t != null ?");
    let resp = educator.query_position(&file, line, col).unwrap();
    let t = resp
        .stack
        .iter()
        .find(|c| c.kind == "ternary_expression")
        .expect("ternary_expression in stack");
    assert_eq!(t.attrs.get("is_nested").map(String::as_str), Some("false"));
    assert!(
        !resp.specific.iter().any(|r| r.rule_id == "nested-ternary-discouraged"),
        "single-level ternary must not trigger the nested-ternary rule"
    );
}

// ─── class_declaration inheritance enrichment + lesson + rule ────────────────

#[test]
fn abstract_class_without_abstract_methods_fires_rule() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("abstract-no-abstract");
    let source = r#"package com.example;
public abstract class JsonHelpers {
    public String wrap(String v) { return "\"" + v + "\""; }
    public String unwrap(String v) { return v.substring(1, v.length() - 1); }
}
"#;
    let file = tmp.write("JsonHelpers.java", source);
    let (line, col) = locate(source, "public abstract class");
    let resp = educator.query_position(&file, line, col).unwrap();

    let cls = resp
        .stack
        .iter()
        .find(|c| c.kind == "class_declaration")
        .expect("class_declaration in stack");
    assert_eq!(cls.attrs.get("is_abstract").map(String::as_str), Some("true"));
    assert_eq!(
        cls.attrs.get("has_abstract_methods").map(String::as_str),
        Some("false"),
    );

    assert!(
        resp.specific.iter().any(|r| r.rule_id == "abstract-class-without-abstract-methods"),
        "expected abstract-class-without-abstract-methods, got: {:?}",
        resp.specific.iter().map(|r| &r.rule_id).collect::<Vec<_>>()
    );
    assert!(
        resp.lessons.iter().any(|l| l.lesson_id == "class-inheritance-fundamentals"),
        "expected class-inheritance-fundamentals lesson"
    );
}

#[test]
fn abstract_class_with_abstract_methods_does_not_fire_rule() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("abstract-with-abstract");
    let source = r#"package com.example;
public abstract class PaymentMethod {
    protected double balance;
    public abstract boolean processPayment(double amount);
    public double getBalance() { return balance; }
}
"#;
    let file = tmp.write("PaymentMethod.java", source);
    let (line, col) = locate(source, "public abstract class");
    let resp = educator.query_position(&file, line, col).unwrap();
    let cls = resp
        .stack
        .iter()
        .find(|c| c.kind == "class_declaration")
        .expect("class_declaration in stack");
    assert_eq!(cls.attrs.get("is_abstract").map(String::as_str), Some("true"));
    assert_eq!(
        cls.attrs.get("has_abstract_methods").map(String::as_str),
        Some("true"),
    );
    assert!(
        !resp.specific.iter().any(|r| r.rule_id == "abstract-class-without-abstract-methods"),
        "abstract class with abstract methods must not trigger the rule"
    );
}

#[test]
fn class_extends_captures_superclass_type() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("class-extends");
    let source = r#"package com.example;
public final class Manager extends Employee {
    public Manager() {}
}
"#;
    let file = tmp.write("Manager.java", source);
    let (line, col) = locate(source, "public final class");
    let resp = educator.query_position(&file, line, col).unwrap();
    let cls = resp
        .stack
        .iter()
        .find(|c| c.kind == "class_declaration")
        .expect("class_declaration in stack");
    assert_eq!(cls.attrs.get("is_final").map(String::as_str), Some("true"));
    assert_eq!(cls.attrs.get("is_abstract").map(String::as_str), Some("false"));
    assert_eq!(
        cls.attrs.get("extends_type").map(String::as_str),
        Some("Employee"),
    );
}

#[test]
fn class_without_extends_has_no_superclass_attr() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("class-no-extends");
    let source = r#"package com.example;
public class Plain {
}
"#;
    let file = tmp.write("Plain.java", source);
    let (line, col) = locate(source, "public class");
    let resp = educator.query_position(&file, line, col).unwrap();
    let cls = resp
        .stack
        .iter()
        .find(|c| c.kind == "class_declaration")
        .expect("class_declaration in stack");
    assert!(
        !cls.attrs.contains_key("extends_type"),
        "class without `extends` must not carry extends_type, got: {:?}",
        cls.attrs.get("extends_type"),
    );
}

#[test]
fn multi_catch_clause_recognised() {
    let educator = load_repo_corpus();
    let tmp = TmpDir::new("catch-multi");
    let source = r#"package com.example;
public class C {
    public void use() {
        try {
            risky();
        } catch (IllegalStateException | IllegalArgumentException e) {
            handle(e);
        }
    }
    void risky() {}
    void handle(Throwable t) {}
}
"#;
    let file = tmp.write("C.java", source);
    let (line, col) = locate(source, "catch (IllegalStateException");
    let resp = educator.query_position(&file, line, col).unwrap();
    let c = resp
        .stack
        .iter()
        .find(|c| c.kind == "catch_clause")
        .expect("catch_clause in stack");
    assert_eq!(c.attrs.get("is_multi_catch").map(String::as_str), Some("true"));
}
