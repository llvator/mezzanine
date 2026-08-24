//! ansible-deploy — topology parser for declarative Kubernetes
//! deployment repos (Ansible + Helm + Jinja2).
//!
//! This is not a code-quality parser. It recovers the *deploy topology*
//! of a repo whose logic lives in data and templates, not in functions:
//!
//! ```text
//! HostGroup (group_vars/<g>)
//!   └─contains→ DeploymentSet (k8s_pre_deployments / k8s_post_deployments)
//!                  └─contains→ DeploymentEntry {name, path, template?}
//!                                 └─RendersFrom→ TemplateFile (files/<path>/<tpl>)
//!                                                   └─contains→ K8sResource (kind: …)
//! ```
//!
//! See ADR 0003 (ansible/k8s topology) for the model.
//!
//! ## The per-file / cross-file split
//!
//! `LanguageParser::parse` sees one file at a time, so this parser can
//! only emit what a single file knows:
//!
//! - A **var file** (`group_vars/<g>/k8s.yml`) emits its HostGroup,
//!   DeploymentSets and DeploymentEntries, plus a `RendersFrom` edge to
//!   each entry's *predicted* TemplateFile id.
//! - A **template** (`files/<path>/<name>.yml.j2`) emits its TemplateFile
//!   and the K8sResources it defines.
//!
//! The two sides never meet at parse time. They link post-merge in the
//! analyzer, which keys entities by id: both sides compute the same
//! TemplateFile id from the `files/`-relative path, so the `RendersFrom`
//! edge resolves. This is the same id-prediction trick the Elevator
//! parser uses for cross-file `Contains`.
//!
//! ## Id scheme (must agree across both sides)
//!
//! - HostGroup:       `ansible::group.<scope>`
//! - DeploymentSet:   `ansible::set.<scope>.<var>`
//! - DeploymentEntry: `ansible::entry.<scope>.<var>.<idx>`
//! - TemplateFile:    `ansible::tpl.<files-relative-path>`
//! - K8sResource:     `ansible::res.<files-relative-path>#<doc-idx>`

#[cfg(test)]
mod tests;

use super::language_parser::{LanguageParser, ParseResult};
use crate::models::file_info::Language;
use crate::models::{
    CodeEntity, EntityKind, Position, Relationship, RelationshipKind, Span, Visibility,
};
use anyhow::Result;
use std::path::Path;

pub struct AnsibleParser;

impl AnsibleParser {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AnsibleParser {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageParser for AnsibleParser {
    fn language(&self) -> Language {
        Language::AnsibleDeploy
    }

    fn parse(&self, path: &Path, content: &str) -> Result<ParseResult> {
        let mut result = ParseResult::new();
        match classify(path) {
            FileRole::Playbook { rel } => parse_playbook(path, content, &rel, &mut result),
            FileRole::VarFile { scope, kind } => {
                parse_var_file(path, content, &scope, kind, &mut result)
            }
            FileRole::Template { rel } => parse_template(path, content, &rel, &mut result),
            FileRole::Other => {}
        }
        Ok(result)
    }

    /// ansible-deploy is path-classified rather than extension-owned:
    /// only var files and manifest templates are ours.
    fn can_parse(&self, path: &Path) -> bool {
        !matches!(classify(path), FileRole::Other)
    }
}

// =====================================================================
// Path classification
// =====================================================================

/// Which kind of ansible-deploy file this path is, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FileRole {
    /// A `playbooks/**/<name>.yml` play file; `rel` is the path relative
    /// to the `playbooks/` directory, extension stripped.
    Playbook { rel: String },
    /// A `group_vars/<g>/<kind>.yml` or `host_vars/<h>/<kind>.yml` file.
    VarFile { scope: String, kind: VarKind },
    /// A manifest template under `files/`; `rel` is the path relative to
    /// the `files/` directory (the id-linking key).
    Template { rel: String },
    /// Not an ansible-deploy file.
    Other,
}

/// The deployment-set variables a host group can define — the shared
/// vocabulary that links a Role's `Requires` to a Set's `Provides`.
/// Excludes the role's internal `k8s_deployment_deployments` param,
/// which is assigned *from* one of these.
const KNOWN_SET_VARS: &[&str] = &[
    "k8s_pre_deployments",
    "k8s_post_deployments",
    "helm_deployment_charts",
];

/// The deployment dimension a var file carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VarKind {
    /// `k8s.yml` → `k8s_pre_deployments` / `k8s_post_deployments`.
    K8s,
    /// `helm.yml` → `helm_deployment_charts`.
    Helm,
    /// Any other var file under `group_vars`/`host_vars` — carries no
    /// deployment sets, only plain variable definitions.
    Other,
}

impl VarKind {
    /// The deployment-set variable names this var kind may define, in
    /// declaration order.
    fn set_vars(self) -> &'static [&'static str] {
        match self {
            VarKind::K8s => &["k8s_pre_deployments", "k8s_post_deployments"],
            VarKind::Helm => &["helm_deployment_charts"],
            VarKind::Other => &[],
        }
    }

    /// The var kind implied by a file stem (`k8s`/`helm`), else `Other`.
    fn from_stem(stem: &str) -> Self {
        match stem {
            "k8s" => VarKind::K8s,
            "helm" => VarKind::Helm,
            _ => VarKind::Other,
        }
    }
}

fn classify(path: &Path) -> FileRole {
    let segs: Vec<String> = path
        .iter()
        .map(|s| s.to_string_lossy().to_string())
        .collect();

    // Playbook: `.../playbooks/**/<name>.yml`.
    if is_yaml(path) {
        if let Some(pos) = segs.iter().position(|s| s == "playbooks") {
            if pos + 1 < segs.len() {
                let rel = segs[pos + 1..].join("/");
                let rel = rel
                    .strip_suffix(".yml")
                    .or_else(|| rel.strip_suffix(".yaml"))
                    .unwrap_or(&rel)
                    .to_string();
                return FileRole::Playbook { rel };
            }
        }
    }

    // Var file: any yaml under `group_vars`/`host_vars`. `k8s.yml` /
    // `helm.yml` carry deployment sets; every other file contributes
    // plain variable definitions (the templating dimension). Supports
    // both the dir-per-scope layout (`group_vars/<scope>/<file>.yml`,
    // which the target repo uses) and the flat layout
    // (`group_vars/<scope>.yml`).
    if is_yaml(path) {
        let n = segs.len();
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        // dir-per-scope: parent = scope, grandparent = group_vars|host_vars
        if n >= 3 && is_var_root(&segs[n - 3]) {
            return FileRole::VarFile {
                scope: segs[n - 2].clone(),
                kind: VarKind::from_stem(stem),
            };
        }
        // flat: parent = group_vars|host_vars, file stem = scope
        if n >= 2 && is_var_root(&segs[n - 2]) {
            return FileRole::VarFile {
                scope: stem.to_string(),
                kind: VarKind::Other,
            };
        }
    }

    // Template: a manifest (`.j2`/`.yml`/`.yaml`) under an
    // `environments/<env>/files/` directory. The `environments`
    // requirement keeps vendored collections' own `roles/*/files/`
    // directories out — those aren't part of the deploy topology.
    if is_manifestish(path) {
        if let Some(pos) = segs.iter().position(|s| s == "files") {
            let under_environments = segs[..pos].iter().any(|s| s == "environments");
            if under_environments && pos + 1 < segs.len() {
                let rel = segs[pos + 1..].join("/");
                return FileRole::Template { rel };
            }
        }
    }

    FileRole::Other
}

/// True if `path` is an ansible-deploy file this parser handles. Used by
/// the language dispatcher for path-aware detection (ansible-deploy owns
/// no file extension).
pub fn is_deploy_file(path: &Path) -> bool {
    !matches!(classify(path), FileRole::Other)
}

/// True if a path segment is a variable root directory.
fn is_var_root(seg: &str) -> bool {
    seg == "group_vars" || seg == "host_vars"
}

/// True if the final extension is `yml`/`yaml`.
fn is_yaml(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("yml") | Some("yaml")
    )
}

/// True if the file could be a k8s manifest template: a `.j2`, `.yml` or
/// `.yaml`. (`.yml.j2` reports its final extension as `j2`.)
fn is_manifestish(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("j2") | Some("yml") | Some("yaml")
    )
}

// =====================================================================
// Var-file parsing  (the "what is declared as deployed" side)
// =====================================================================

fn parse_var_file(
    path: &Path,
    content: &str,
    scope: &str,
    kind: VarKind,
    result: &mut ParseResult,
) {
    let doc: serde_yaml::Value = match serde_yaml::from_str(content) {
        Ok(v) => v,
        Err(e) => {
            result.add_warning(format!("ansible: YAML parse error in {}: {}", scope, e));
            return;
        }
    };

    let group_id = ensure_host_group(scope, path, result);

    // Plain variable definitions (the templating dimension): every scalar
    // top-level key becomes a Variable the scope `Provides`. Templates
    // that interpolate the same name link to it post-merge. Non-scalar
    // keys (lists/maps — including the deployment-set vars handled below)
    // are skipped: they aren't `{{ substitution }}` values.
    extract_var_definitions(&doc, scope, &group_id, path, result);

    for var in kind.set_vars() {
        let seq = match doc.get(var).and_then(|v| v.as_sequence()) {
            Some(s) if !s.is_empty() => s,
            _ => continue,
        };

        let set_id = format!("ansible::set.{}.{}", scope, var);
        let mut set = new_entity(var, EntityKind::DeploymentSet, path, 0);
        set.id = set_id.clone();
        set.qualified_name = format!("{}.{}", scope, var);
        result.add_entity(set);
        result.add_relationship(Relationship::new(
            &group_id,
            &set_id,
            RelationshipKind::Contains,
        ));

        // Bind the set to the shared deployment-variable hub, so a Role
        // that `Requires` this variable connects to every host group's
        // set that provides it. The var node id is deterministic, so the
        // playbook side (which emits the same id) links post-merge.
        let var_id = ensure_var_hub(var, path, result);
        result.add_relationship(Relationship::new(
            &set_id,
            &var_id,
            RelationshipKind::Provides,
        ));

        for (idx, item) in seq.iter().enumerate() {
            emit_entry(path, scope, var, idx, item, kind, &set_id, result);
        }
    }
}

fn emit_entry(
    path: &Path,
    scope: &str,
    var: &str,
    idx: usize,
    item: &serde_yaml::Value,
    kind: VarKind,
    set_id: &str,
    result: &mut ParseResult,
) {
    let name = item
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("<unnamed>")
        .to_string();

    let entry_id = format!("ansible::entry.{}.{}.{}", scope, var, idx);
    let entry_kind = match kind {
        VarKind::K8s => EntityKind::DeploymentEntry,
        VarKind::Helm => EntityKind::HelmChart,
        // Unreachable: `Other` has no deployment-set vars, so this
        // per-entry path is never taken for it.
        VarKind::Other => EntityKind::DeploymentEntry,
    };
    let mut entry = new_entity(&name, entry_kind, path, 0);
    entry.id = entry_id.clone();
    entry.qualified_name = format!("{}.{}.{}", scope, var, name);
    result.add_relationship(Relationship::new(
        set_id,
        &entry_id,
        RelationshipKind::Contains,
    ));

    if kind == VarKind::K8s {
        // Resolve the template this entry renders:
        //   files/{path}/{template ?? name + ".yml.j2"}
        if let Some(dir) = item.get("path").and_then(|v| v.as_str()) {
            let file = item
                .get("template")
                .and_then(|v| v.as_str())
                .map(|t| t.to_string())
                .unwrap_or_else(|| format!("{}.yml.j2", name));
            let rel = format!("{}/{}", dir, file);
            entry.attributes.push(format!("renders:{}", rel));
            let tpl_id = format!("ansible::tpl.{}", rel);
            result.add_relationship(Relationship::new(
                &entry_id,
                &tpl_id,
                RelationshipKind::RendersFrom,
            ));
        } else {
            result.add_warning(format!(
                "ansible: k8s entry `{}` in {}.{} has no `path` — cannot resolve its template",
                name, scope, var
            ));
        }
    } else if let Some(chart) = item.get("chart").and_then(|v| v.as_str()) {
        entry.attributes.push(format!("chart:{}", chart));
    }

    result.add_entity(entry);
}

/// Emit a Variable node + `Provides` edge for each scalar top-level key.
/// The value rides the edge (label + `value` metadata) so a variable
/// defined in several scopes shows one node with a Provides edge per
/// scope — i.e. its override chain.
fn extract_var_definitions(
    doc: &serde_yaml::Value,
    scope: &str,
    group_id: &str,
    path: &Path,
    result: &mut ParseResult,
) {
    let map = match doc.as_mapping() {
        Some(m) => m,
        None => return,
    };
    for (k, v) in map {
        let key = match k.as_str() {
            Some(s) => s,
            None => continue,
        };
        let value = match scalar_to_string(v) {
            Some(s) => s,
            None => continue, // lists/maps aren't `{{ substitution }}` vars
        };
        let var_id = ensure_template_var(key, path, result);
        let label = truncate(&value, 60);
        add_unique_rel(
            Relationship::new(group_id, &var_id, RelationshipKind::Provides)
                .with_label(label)
                .with_metadata("value", value)
                .with_metadata("scope", scope.to_string()),
            result,
        );
    }
}

/// String form of a YAML scalar (string / number / bool). `None` for
/// nulls and composite values.
fn scalar_to_string(v: &serde_yaml::Value) -> Option<String> {
    match v {
        serde_yaml::Value::String(s) => Some(s.clone()),
        serde_yaml::Value::Number(n) => Some(n.to_string()),
        serde_yaml::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Truncate to `max` chars with an ellipsis (char-safe).
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        format!(
            "{}…",
            s.chars().take(max.saturating_sub(1)).collect::<String>()
        )
    } else {
        s.to_string()
    }
}

// =====================================================================
// Template parsing  (the "which file defines which primitive" side)
// =====================================================================

fn parse_template(path: &Path, content: &str, rel: &str, result: &mut ParseResult) {
    let tpl_id = format!("ansible::tpl.{}", rel);
    let display = rel.rsplit('/').next().unwrap_or(rel);

    // Templating dimension: the `{{ variables }}` this template depends
    // on. Each resolves to its group_vars/host_vars definition post-merge
    // through the shared `ansible::var.<name>` id.
    for var in scan_interpolations(content) {
        let var_id = ensure_template_var(&var, path, result);
        result.add_relationship(Relationship::new(
            &tpl_id,
            &var_id,
            RelationshipKind::Interpolates,
        ));
    }

    let mut tpl = new_entity(display, EntityKind::TemplateFile, path, 0);
    tpl.id = tpl_id.clone();
    tpl.qualified_name = rel.to_string();
    result.add_entity(tpl);

    for (doc_idx, res) in scan_k8s_resources(content).into_iter().enumerate() {
        let res_id = resource_id(&res, rel, doc_idx);
        let label = match &res.name {
            Some(n) => format!("{} {}", res.kind, n),
            None => res.kind.clone(),
        };
        let mut entity = new_entity(&label, EntityKind::K8sResource, path, res.line);
        entity.id = res_id.clone();
        entity.qualified_name = match &res.name {
            Some(n) => format!("{}/{}", res.kind, n),
            None => format!("{}#{}", rel, doc_idx),
        };
        entity.attributes.push(format!("k8s_kind:{}", res.kind));
        if let Some(n) = &res.name {
            entity.attributes.push(format!("k8s_name:{}", n));
        }
        if let Some(ns) = &res.namespace {
            entity.attributes.push(format!("k8s_namespace:{}", ns));
        }
        // Kind-specific detail (replicas, image, service type, PVC size…)
        // surfaced in the UI detail panel under a `k8s_` prefix.
        for (k, v) in &res.attrs {
            entity.attributes.push(format!("k8s_{}:{}", k, v));
        }
        result.add_entity(entity);
        result.add_relationship(Relationship::new(
            &tpl_id,
            &res_id,
            RelationshipKind::Contains,
        ));

        // Reference edges to other resources. Both ends compute the same
        // name-based id (`ansible::res.<kind>.<name>`), so the edge links
        // cross-file post-merge. Refs whose target isn't defined anywhere
        // (Jinja-templated names, externally-managed objects) simply stay
        // unresolved — the analyzer drops edges with no matching target.
        for r in &res.refs {
            let tgt = format!("ansible::res.{}.{}", r.target_kind, r.target_name);
            if tgt == res_id {
                continue; // guard against a resource referencing itself
            }
            result.add_relationship(
                Relationship::new(&res_id, &tgt, RelationshipKind::References)
                    .with_label(r.verb.clone())
                    .with_metadata("k8s_ref", r.verb.clone()),
            );
        }
    }
}

/// The cross-file-linkable id for a resource: name-based when the object
/// has a `metadata.name` (so a reference by name resolves to it), falling
/// back to a per-occurrence path id for the rare nameless document.
fn resource_id(res: &K8sResourceInfo, rel: &str, idx: usize) -> String {
    match &res.name {
        Some(n) => format!("ansible::res.{}.{}", res.kind, n),
        None => format!("ansible::res.{}#{}", rel, idx),
    }
}

/// A reference from one resource to another (by kind + name).
#[derive(Debug, Clone, PartialEq, Eq)]
struct K8sRef {
    target_kind: String,
    target_name: String,
    /// Human verb for the edge label (`mounts`, `routes to`, …).
    verb: String,
}

/// A Kubernetes object found in a template: its `kind:`, `metadata.name`
/// / `namespace`, a few kind-specific attributes, and outgoing references
/// to other resources.
#[derive(Debug, Clone, PartialEq, Eq)]
struct K8sResourceInfo {
    kind: String,
    name: Option<String>,
    namespace: Option<String>,
    /// 0-indexed source line of the `kind:` declaration.
    line: usize,
    attrs: Vec<(String, String)>,
    refs: Vec<K8sRef>,
}

/// Scan a (possibly Jinja-templated, possibly multi-document) manifest
/// for its Kubernetes objects. Deliberately line-oriented rather than a
/// YAML parse: templates contain `{{ }}`/`{% %}` and are not valid YAML.
///
/// A document is delimited by a line that is exactly `---`. Within a
/// document we read the first top-level `kind:`, the `metadata.name` /
/// `namespace`, a handful of kind-specific attributes, and references to
/// other resources expressed by the distinctive keys Kubernetes uses
/// (`secretName`, `claimName`, `imagePullSecrets`, `secretRef`,
/// `configMapRef`, `configMap`, and an Ingress backend `service`).
fn scan_k8s_resources(content: &str) -> Vec<K8sResourceInfo> {
    let mut out = Vec::new();
    let mut cur: Option<K8sResourceInfo> = None;
    let mut in_metadata = false;
    // (target_kind, verb) awaiting the next `name:` line to resolve into
    // a reference — e.g. `secretRef:` sets this, the following `name:`
    // consumes it.
    let mut pending: Option<(&'static str, &'static str)> = None;

    for (lineno, raw) in content.lines().enumerate() {
        let line = raw.trim_end();
        let trimmed = line.trim_start();

        if trimmed == "---" {
            if let Some(r) = cur.take() {
                out.push(r);
            }
            in_metadata = false;
            pending = None;
            continue;
        }

        let indent = line.len() - trimmed.len();

        // Top-level `kind:` opens a new resource document.
        if indent == 0 {
            if let Some(v) = trimmed.strip_prefix("kind:") {
                if cur.is_none() {
                    cur = Some(K8sResourceInfo {
                        kind: unquote(v.trim()).to_string(),
                        name: None,
                        namespace: None,
                        line: lineno,
                        attrs: Vec::new(),
                        refs: Vec::new(),
                    });
                }
                in_metadata = false;
                pending = None;
                continue;
            }
            in_metadata = trimmed.starts_with("metadata:");
            pending = None;
            continue;
        }

        let res = match cur.as_mut() {
            Some(r) => r,
            None => continue, // detail before the first `kind:` — ignore
        };

        // Normalise a leading list dash so `- name:` matches `name:`.
        let body = trimmed.strip_prefix("- ").unwrap_or(trimmed);

        // --- reference triggers with an inline value --------------------
        if let Some(v) = body.strip_prefix("secretName:") {
            push_ref(res, "Secret", v, "mounts");
            continue;
        }
        if let Some(v) = body.strip_prefix("claimName:") {
            push_ref(res, "PersistentVolumeClaim", v, "mounts");
            continue;
        }
        // --- reference triggers resolved by the following `name:` -------
        if body.starts_with("imagePullSecrets:") {
            pending = Some(("Secret", "pulls with"));
            continue;
        }
        if body.starts_with("secretRef:") {
            pending = Some(("Secret", "reads env from"));
            continue;
        }
        if body.starts_with("configMapRef:") {
            pending = Some(("ConfigMap", "reads env from"));
            continue;
        }
        if body.starts_with("configMap:") {
            pending = Some(("ConfigMap", "mounts"));
            continue;
        }
        if body.starts_with("service:") && res.kind == "Ingress" {
            pending = Some(("Service", "routes to"));
            continue;
        }

        // --- `name:` consumes a pending ref, else fills metadata.name ---
        if let Some(v) = body.strip_prefix("name:") {
            if let Some((tk, verb)) = pending.take() {
                push_ref(res, tk, v, verb);
            } else if in_metadata && res.name.is_none() {
                res.name = Some(unquote(v.trim()).to_string());
            }
            continue;
        }

        // --- attributes -------------------------------------------------
        if let Some(v) = body.strip_prefix("namespace:") {
            if res.namespace.is_none() {
                res.namespace = Some(unquote(v.trim()).to_string());
            }
            continue;
        }
        if let Some(v) = body.strip_prefix("replicas:") {
            set_attr(res, "replicas", v);
            continue;
        }
        if let Some(v) = body.strip_prefix("image:") {
            set_attr(res, "image", v);
            continue;
        }
        if let Some(v) = body.strip_prefix("storage:") {
            set_attr(res, "storage", v);
            continue;
        }
        if res.kind == "Service" {
            if let Some(v) = body.strip_prefix("type:") {
                set_attr(res, "type", v);
                continue;
            }
        }
    }
    if let Some(r) = cur.take() {
        out.push(r);
    }
    out
}

/// Record a reference to another resource, de-duplicated by (kind, name).
fn push_ref(res: &mut K8sResourceInfo, target_kind: &str, name: &str, verb: &str) {
    let target_name = unquote(name.trim()).to_string();
    if target_name.is_empty() {
        return;
    }
    let dup = res
        .refs
        .iter()
        .any(|r| r.target_kind == target_kind && r.target_name == target_name);
    if !dup {
        res.refs.push(K8sRef {
            target_kind: target_kind.to_string(),
            target_name,
            verb: verb.to_string(),
        });
    }
}

/// Record a kind-specific attribute, keeping the first value seen.
fn set_attr(res: &mut K8sResourceInfo, key: &str, value: &str) {
    if res.attrs.iter().any(|(k, _)| k == key) {
        return;
    }
    let v = unquote(value.trim()).to_string();
    if !v.is_empty() {
        res.attrs.push((key.to_string(), v));
    }
}

/// Strip a single pair of matching surrounding quotes.
fn unquote(s: &str) -> &str {
    let b = s.as_bytes();
    if b.len() >= 2
        && ((b[0] == b'"' && b[b.len() - 1] == b'"') || (b[0] == b'\'' && b[b.len() - 1] == b'\''))
    {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

// =====================================================================
// Jinja interpolation scanning  (the templating dimension)
// =====================================================================

/// Extract the distinct root variables interpolated by `{{ … }}` blocks.
/// Deliberately shallow: the first non-noise identifier in each block is
/// taken as the variable the template depends on (filters, string
/// literals, and Jinja keywords are skipped). Loop-local names and
/// Ansible magic vars that no var file defines simply resolve to nothing.
fn scan_interpolations(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut rest = content;
    while let Some(open) = rest.find("{{") {
        let after = &rest[open + 2..];
        let (expr, next) = match after.find("}}") {
            Some(end) => (&after[..end], &after[end + 2..]),
            None => break,
        };
        if let Some(root) = first_var_identifier(expr) {
            if seen.insert(root.clone()) {
                out.push(root);
            }
        }
        rest = next;
    }
    out
}

/// The first identifier in a Jinja expression that names a variable —
/// skipping quoted string literals and known Jinja keywords/filters.
fn first_var_identifier(expr: &str) -> Option<String> {
    let b = expr.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        // Skip string literals so `lookup('env', X)` doesn't yield `env`.
        if c == b'\'' || c == b'"' {
            i += 1;
            while i < b.len() && b[i] != c {
                i += 1;
            }
            i += 1;
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let ident = &expr[start..i];
            if !is_jinja_noise(ident) {
                return Some(ident.to_string());
            }
        } else {
            i += 1;
        }
    }
    None
}

/// Identifiers that name a Jinja keyword, common filter/test, or Ansible
/// magic var rather than a project variable.
fn is_jinja_noise(ident: &str) -> bool {
    if ident.starts_with("ansible_") {
        return true;
    }
    matches!(
        ident,
        "if" | "else"
            | "elif"
            | "endif"
            | "for"
            | "endfor"
            | "in"
            | "is"
            | "not"
            | "and"
            | "or"
            | "true"
            | "false"
            | "none"
            | "item"
            | "loop"
            | "default"
            | "defined"
            | "undefined"
            | "mandatory"
            | "length"
            | "count"
            | "join"
            | "split"
            | "upper"
            | "lower"
            | "capitalize"
            | "title"
            | "trim"
            | "replace"
            | "regex_replace"
            | "regex_search"
            | "int"
            | "float"
            | "string"
            | "bool"
            | "list"
            | "dict"
            | "map"
            | "select"
            | "reject"
            | "selectattr"
            | "rejectattr"
            | "first"
            | "last"
            | "unique"
            | "sort"
            | "min"
            | "max"
            | "range"
            | "lookup"
            | "b64encode"
            | "b64decode"
            | "to_json"
            | "to_yaml"
            | "to_nice_yaml"
            | "from_json"
            | "from_yaml"
            | "hostvars"
            | "groups"
            | "group_names"
            | "inventory_hostname"
            | "omit"
    )
}

// =====================================================================
// Playbook parsing  (the orchestration side: playbook → role → var)
// =====================================================================

fn parse_playbook(path: &Path, content: &str, rel: &str, result: &mut ParseResult) {
    let doc: serde_yaml::Value = match serde_yaml::from_str(content) {
        Ok(v) => v,
        Err(e) => {
            result.add_warning(format!(
                "ansible: YAML parse error in playbook {}: {}",
                rel, e
            ));
            return;
        }
    };
    let plays = match doc.as_sequence() {
        Some(s) => s,
        None => return, // not a list of plays
    };

    let playbook_id = format!("ansible::playbook.{}", rel);
    let display = rel.rsplit('/').next().unwrap_or(rel);
    let mut pb = new_entity(display, EntityKind::Playbook, path, 0);
    pb.id = playbook_id.clone();
    pb.qualified_name = rel.to_string();
    result.add_entity(pb);

    for play in plays {
        // Roles listed directly under `roles:`.
        if let Some(roles) = play.get("roles").and_then(|v| v.as_sequence()) {
            for r in roles {
                let name = role_name_from_entry(r);
                emit_role_usage(path, &playbook_id, name, r, result);
            }
        }
        // Roles pulled in via `include_role` / `import_role` tasks.
        if let Some(tasks) = play.get("tasks").and_then(|v| v.as_sequence()) {
            for t in tasks {
                if let Some(name) = include_role_name(t) {
                    emit_role_usage(path, &playbook_id, Some(name), t, result);
                }
            }
        }
    }
}

/// Extract a role name from a `roles:` list entry — either a bare string
/// or a map with a `role:` key.
fn role_name_from_entry(entry: &serde_yaml::Value) -> Option<&str> {
    entry
        .as_str()
        .or_else(|| entry.get("role").and_then(|v| v.as_str()))
}

/// If a task uses `include_role`/`import_role` (with or without the
/// `ansible.builtin.` FQCN prefix), return the referenced role name.
fn include_role_name(task: &serde_yaml::Value) -> Option<&str> {
    for key in [
        "include_role",
        "import_role",
        "ansible.builtin.include_role",
        "ansible.builtin.import_role",
    ] {
        if let Some(name) = task
            .get(key)
            .and_then(|v| v.get("name"))
            .and_then(|v| v.as_str())
        {
            return Some(name);
        }
    }
    None
}

/// Emit the Role node, the Playbook→Role `Includes` edge, and a
/// Role→Var `Requires` edge for each known deployment variable this
/// usage references (via its `vars:` values or `when:` conditions).
fn emit_role_usage(
    path: &Path,
    playbook_id: &str,
    name: Option<&str>,
    usage: &serde_yaml::Value,
    result: &mut ParseResult,
) {
    let name = match name {
        Some(n) => n,
        None => return,
    };
    let role_id = format!("ansible::role.{}", name);
    ensure_entity(&role_id, name, EntityKind::Role, path, result);
    add_unique_rel(
        Relationship::new(playbook_id, &role_id, RelationshipKind::Includes),
        result,
    );

    // `Requires` is sourced from the *playbook*, not the shared Role:
    // the var binding (`vars:` / `when:`) lives in the playbook's task,
    // and the same role is included by many playbooks with different
    // vars. Playbook-scoped edges are also naturally unique per file, so
    // no cross-file duplicates accumulate on the shared role node.
    for var in required_set_vars(usage) {
        let var_id = ensure_var_hub(var, path, result);
        add_unique_rel(
            Relationship::new(playbook_id, &var_id, RelationshipKind::Requires),
            result,
        );
    }
}

/// Collect the KNOWN_SET_VARS referenced by a role usage. Scans the
/// stringified `vars:` values (e.g. `{{ k8s_pre_deployments }}`) and
/// `when:` conditions (e.g. `vars['k8s_pre_deployments'] is defined`).
/// A whole-token match is unnecessary: no known var is a substring of
/// another identifier present in these files.
fn required_set_vars(usage: &serde_yaml::Value) -> Vec<&'static str> {
    let mut haystack = String::new();
    if let Some(vars) = usage.get("vars").and_then(|v| v.as_mapping()) {
        for (_, val) in vars {
            if let Some(s) = val.as_str() {
                haystack.push_str(s);
                haystack.push('\n');
            }
        }
    }
    match usage.get("when") {
        Some(serde_yaml::Value::String(s)) => haystack.push_str(s),
        Some(serde_yaml::Value::Sequence(seq)) => {
            for v in seq {
                if let Some(s) = v.as_str() {
                    haystack.push_str(s);
                    haystack.push('\n');
                }
            }
        }
        _ => {}
    }

    KNOWN_SET_VARS
        .iter()
        .copied()
        .filter(|v| haystack.contains(v))
        .collect()
}

// =====================================================================
// Shared helpers
// =====================================================================

/// Ensure a HostGroup node for `scope` exists and return its id.
fn ensure_host_group(scope: &str, path: &Path, result: &mut ParseResult) -> String {
    let group_id = format!("ansible::group.{}", scope);
    ensure_entity(&group_id, scope, EntityKind::HostGroup, path, result);
    group_id
}

/// Ensure the shared deployment-variable hub node exists and return its
/// id. Both the var-file side (Set `Provides` var) and the playbook side
/// (Role `Requires` var) call this with the same name, so the two link
/// post-merge through the identical id.
fn ensure_var_hub(var: &str, path: &Path, result: &mut ParseResult) -> String {
    let var_id = format!("ansible::var.{}", var);
    ensure_entity(&var_id, var, EntityKind::Variable, path, result);
    var_id
}

/// Like `ensure_var_hub`, but marks the variable as part of the
/// high-volume templating dimension (`{{ substitution }}` vars and their
/// definitions) so the UI can hide the layer by default. Kept distinct
/// from the handful of deployment-set hubs (`k8s_pre_deployments`, …),
/// which are structural and stay visible.
fn ensure_template_var(var: &str, path: &Path, result: &mut ParseResult) -> String {
    let var_id = ensure_var_hub(var, path, result);
    if let Some(e) = result.entities.iter_mut().find(|e| e.id == var_id) {
        e.tags.insert("template_var".to_string());
    }
    var_id
}

/// Add an entity with the given id if one isn't already present in this
/// parse result. (Cross-file duplicates are deduped by the analyzer,
/// which keys entities by id; this only avoids in-file repeats.)
fn ensure_entity(id: &str, name: &str, kind: EntityKind, path: &Path, result: &mut ParseResult) {
    if result.entities.iter().any(|e| e.id == id) {
        return;
    }
    let mut e = new_entity(name, kind, path, 0);
    e.id = id.to_string();
    e.qualified_name = name.to_string();
    result.add_entity(e);
}

/// Add a relationship unless an identical (source, target, kind) edge is
/// already present — role usages repeat across plays.
fn add_unique_rel(rel: Relationship, result: &mut ParseResult) {
    if result
        .relationships
        .iter()
        .any(|r| r.source_id == rel.source_id && r.target_id == rel.target_id && r.kind == rel.kind)
    {
        return;
    }
    result.add_relationship(rel);
}

/// Build a bare public, ansible-tagged entity on a single-line span.
fn new_entity(name: &str, kind: EntityKind, path: &Path, line: usize) -> CodeEntity {
    let span = Span::new(Position::new(line, 0, 0), Position::new(line, 0, 0));
    let mut e = CodeEntity::new(name, kind, path, span);
    e.visibility = Visibility::Public;
    e.tags.insert("ansible".to_string());
    e
}
