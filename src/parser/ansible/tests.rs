//! Tests for the ansible-deploy topology parser. Fixtures mirror the
//! real shapes in an ACME-style deployment repo.

use super::*;
use crate::parser::language_parser::LanguageParser;
use std::path::Path;

fn parse(path: &str, content: &str) -> ParseResult {
    AnsibleParser::new()
        .parse(Path::new(path), content)
        .expect("parse should not error")
}

fn ids_of(result: &ParseResult, kind: EntityKind) -> Vec<String> {
    result
        .entities
        .iter()
        .filter(|e| e.kind == kind)
        .map(|e| e.id.clone())
        .collect()
}

fn renders_edges(result: &ParseResult) -> Vec<(String, String)> {
    result
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::RendersFrom)
        .map(|r| (r.source_id.clone(), r.target_id.clone()))
        .collect()
}

// ---------------------------------------------------------------------
// Classification
// ---------------------------------------------------------------------

#[test]
fn classifies_group_var_file() {
    assert_eq!(
        classify(Path::new(
            "environments/production/group_vars/acme_portal/k8s.yml"
        )),
        FileRole::VarFile {
            scope: "acme_portal".into(),
            kind: VarKind::K8s,
        }
    );
}

#[test]
fn classifies_host_var_helm_file() {
    assert_eq!(
        classify(Path::new(
            "environments/production/host_vars/acme-be-prod.acmecluster/helm.yml"
        )),
        FileRole::VarFile {
            scope: "acme-be-prod.acmecluster".into(),
            kind: VarKind::Helm,
        }
    );
}

#[test]
fn classifies_template_relative_to_files_dir() {
    assert_eq!(
        classify(Path::new(
            "environments/production/files/acme-portal-k8s/acme-portal-ds-config.yml.j2"
        )),
        FileRole::Template {
            rel: "acme-portal-k8s/acme-portal-ds-config.yml.j2".into(),
        }
    );
}

#[test]
fn ignores_unrelated_yaml() {
    // A random YAML file that is neither a var file nor under `files/`.
    assert_eq!(classify(Path::new("ansible.cfg.yml")), FileRole::Other);
    assert_eq!(
        classify(Path::new("some/other/config.yaml")),
        FileRole::Other
    );
    // Any yaml under group_vars/host_vars is a var file — even one whose
    // stem we don't special-case (vault.yml contributes plain variable
    // definitions; if it's an encrypted vault, the YAML parse just fails
    // gracefully and yields nothing).
    assert_eq!(
        classify(Path::new(
            "environments/production/group_vars/acme_portal/vault.yml"
        )),
        FileRole::VarFile {
            scope: "acme_portal".into(),
            kind: VarKind::Other,
        }
    );
}

#[test]
fn template_classifier_requires_environments_ancestor() {
    // A vendored collection's own role `files/` dir must NOT be treated
    // as a deploy template.
    assert_eq!(
        classify(Path::new(
            "collections/ansible_collections/foo/roles/bar/files/thing.yml"
        )),
        FileRole::Other
    );
}

#[test]
fn is_deploy_file_predicate() {
    assert!(is_deploy_file(Path::new(
        "environments/production/group_vars/acme_portal/k8s.yml"
    )));
    assert!(is_deploy_file(Path::new(
        "environments/production/files/acme-portal-k8s/x.yml.j2"
    )));
    assert!(!is_deploy_file(Path::new("src/main.rs")));
    assert!(!is_deploy_file(Path::new("random/config.yaml")));
}

// ---------------------------------------------------------------------
// Var-file side
// ---------------------------------------------------------------------

const K8S_VARS: &str = r#"
k8s_pre_deployments:
  - name: acme-dockerconfigjson
    path: common
  - name: acme-portal-ds-config
    path: acme-portal-k8s
    template: acme-portal-ds-config.yml.j2
k8s_post_deployments:
  - name: acme-portal-admin
    path: acme-portal-k8s
"#;

#[test]
fn var_file_emits_group_set_and_entries() {
    let r = parse(
        "environments/production/group_vars/acme_portal/k8s.yml",
        K8S_VARS,
    );

    assert_eq!(ids_of(&r, EntityKind::HostGroup), vec!["ansible::group.acme_portal"]);

    let mut sets = ids_of(&r, EntityKind::DeploymentSet);
    sets.sort();
    assert_eq!(
        sets,
        vec![
            "ansible::set.acme_portal.k8s_post_deployments",
            "ansible::set.acme_portal.k8s_pre_deployments",
        ]
    );

    // Three entries total across both sets.
    assert_eq!(ids_of(&r, EntityKind::DeploymentEntry).len(), 3);
}

#[test]
fn entry_renders_from_default_template_name() {
    // No explicit `template:` → `{path}/{name}.yml.j2`.
    let r = parse(
        "environments/production/group_vars/acme_portal/k8s.yml",
        K8S_VARS,
    );
    let edges = renders_edges(&r);
    assert!(
        edges.contains(&(
            "ansible::entry.acme_portal.k8s_pre_deployments.0".into(),
            "ansible::tpl.common/acme-dockerconfigjson.yml.j2".into(),
        )),
        "default-name template resolution missing; got {:?}",
        edges
    );
}

#[test]
fn entry_renders_from_explicit_template_overrides_name() {
    let r = parse(
        "environments/production/group_vars/acme_portal/k8s.yml",
        K8S_VARS,
    );
    let edges = renders_edges(&r);
    assert!(edges.contains(&(
        "ansible::entry.acme_portal.k8s_pre_deployments.1".into(),
        "ansible::tpl.acme-portal-k8s/acme-portal-ds-config.yml.j2".into(),
    )));
}

#[test]
fn containment_tree_group_to_set_to_entry() {
    let r = parse(
        "environments/production/group_vars/acme_portal/k8s.yml",
        K8S_VARS,
    );
    let contains: Vec<(String, String)> = r
        .relationships
        .iter()
        .filter(|rel| rel.kind == RelationshipKind::Contains)
        .map(|rel| (rel.source_id.clone(), rel.target_id.clone()))
        .collect();

    assert!(contains.contains(&(
        "ansible::group.acme_portal".into(),
        "ansible::set.acme_portal.k8s_pre_deployments".into(),
    )));
    assert!(contains.contains(&(
        "ansible::set.acme_portal.k8s_pre_deployments".into(),
        "ansible::entry.acme_portal.k8s_pre_deployments.0".into(),
    )));
}

// ---------------------------------------------------------------------
// Template side
// ---------------------------------------------------------------------

const TEMPLATE: &str = r#"{% if enable_ds %}
apiVersion: v1
kind: ConfigMap
metadata:
  name: acme-portal-ds-config
  namespace: "{{ namespace }}"
data:
  ds.json: |
    {}
---
apiVersion: v1
kind: Secret
metadata:
  name: acme-portal-ds-secret
type: Opaque
{% endif %}
"#;

#[test]
fn template_extracts_each_k8s_resource() {
    let r = parse(
        "environments/production/files/acme-portal-k8s/acme-portal-ds-config.yml.j2",
        TEMPLATE,
    );

    // The TemplateFile itself.
    assert_eq!(
        ids_of(&r, EntityKind::TemplateFile),
        vec!["ansible::tpl.acme-portal-k8s/acme-portal-ds-config.yml.j2"]
    );

    // Two resources, in document order.
    let res = ids_of(&r, EntityKind::K8sResource);
    assert_eq!(res.len(), 2, "expected 2 resources, got {:?}", res);

    let kinds: Vec<String> = r
        .entities
        .iter()
        .filter(|e| e.kind == EntityKind::K8sResource)
        .flat_map(|e| e.attributes.iter())
        .filter_map(|a| a.strip_prefix("k8s_kind:").map(str::to_string))
        .collect();
    assert_eq!(kinds, vec!["ConfigMap", "Secret"]);

    let names: Vec<String> = r
        .entities
        .iter()
        .filter(|e| e.kind == EntityKind::K8sResource)
        .flat_map(|e| e.attributes.iter())
        .filter_map(|a| a.strip_prefix("k8s_name:").map(str::to_string))
        .collect();
    assert_eq!(names, vec!["acme-portal-ds-config", "acme-portal-ds-secret"]);
}

#[test]
fn template_resource_contained_by_template_file() {
    let r = parse(
        "environments/production/files/acme-portal-k8s/acme-portal-ds-config.yml.j2",
        TEMPLATE,
    );
    let has = r.relationships.iter().any(|rel| {
        rel.kind == RelationshipKind::Contains
            && rel.source_id == "ansible::tpl.acme-portal-k8s/acme-portal-ds-config.yml.j2"
            && rel.target_id == "ansible::res.ConfigMap.acme-portal-ds-config"
    });
    assert!(has, "template should Contain its first resource (name-based id)");
}

const DEPLOYMENT: &str = r#"apiVersion: apps/v1
kind: Deployment
metadata:
  name: acme-portal-maintenance
  namespace: acme-portal
spec:
  replicas: 2
  template:
    spec:
      imagePullSecrets:
        - name: a12-secrets-dockerconfigjson
      containers:
        - name: maintenance
          image: registry.example/maintenance-page:1.2.3
          envFrom:
            - secretRef:
                name: acme-portal-secrets
            - configMapRef:
                name: acme-portal-nginx-configmap
      volumes:
        - name: data
          persistentVolumeClaim:
            claimName: acme-portal-gclogs-pvc
"#;

#[test]
fn deployment_emits_reference_edges() {
    let r = parse(
        "environments/production/files/acme-portal-k8s/acme-portal-admin.yml.j2",
        DEPLOYMENT,
    );
    let refs: Vec<(String, String)> = r
        .relationships
        .iter()
        .filter(|rel| rel.kind == RelationshipKind::References)
        .map(|rel| (rel.source_id.clone(), rel.target_id.clone()))
        .collect();

    let src = "ansible::res.Deployment.acme-portal-maintenance".to_string();
    // imagePullSecrets + envFrom.secretRef -> two Secret refs
    assert!(refs.contains(&(src.clone(), "ansible::res.Secret.a12-secrets-dockerconfigjson".into())));
    assert!(refs.contains(&(src.clone(), "ansible::res.Secret.acme-portal-secrets".into())));
    // envFrom.configMapRef -> ConfigMap ref
    assert!(refs.contains(&(src.clone(), "ansible::res.ConfigMap.acme-portal-nginx-configmap".into())));
    // volumes.persistentVolumeClaim.claimName -> PVC ref
    assert!(refs.contains(&(src, "ansible::res.PersistentVolumeClaim.acme-portal-gclogs-pvc".into())));
}

#[test]
fn deployment_captures_attributes() {
    let r = parse(
        "environments/production/files/acme-portal-k8s/acme-portal-admin.yml.j2",
        DEPLOYMENT,
    );
    let dep = r
        .entities
        .iter()
        .find(|e| e.kind == EntityKind::K8sResource)
        .expect("a k8s resource");
    let attr = |k: &str| dep.attributes.iter().find_map(|a| a.strip_prefix(k));
    assert_eq!(attr("k8s_namespace:"), Some("acme-portal"));
    assert_eq!(attr("k8s_replicas:"), Some("2"));
    assert_eq!(attr("k8s_image:"), Some("registry.example/maintenance-page:1.2.3"));
}

#[test]
fn ingress_routes_to_service_across_files() {
    // An Ingress in one template references a Service by name; the Service
    // defined in another template gets the matching name-based id.
    const INGRESS: &str = r#"kind: Ingress
metadata:
  name: portal-ingress
spec:
  rules:
    - http:
        paths:
          - backend:
              service:
                name: acme-portal-service
                port:
                  name: http
"#;
    const SERVICE: &str = r#"kind: Service
metadata:
  name: acme-portal-service
spec:
  type: ClusterIP
"#;
    let ing = parse("environments/production/files/x/ingress.yml.j2", INGRESS);
    let svc = parse("environments/production/files/x/service.yml.j2", SERVICE);

    let routed = ing.relationships.iter().any(|rel| {
        rel.kind == RelationshipKind::References
            && rel.source_id == "ansible::res.Ingress.portal-ingress"
            && rel.target_id == "ansible::res.Service.acme-portal-service"
    });
    assert!(routed, "ingress should route to the named service");
    // The Service side emits exactly that id, so the edge resolves.
    assert!(svc
        .entities
        .iter()
        .any(|e| e.id == "ansible::res.Service.acme-portal-service"));
}

// ---------------------------------------------------------------------
// Playbook side  (orchestration: playbook → role → var)
// ---------------------------------------------------------------------

#[test]
fn classifies_playbook() {
    assert_eq!(
        classify(Path::new("playbooks/deploy.yml")),
        FileRole::Playbook { rel: "deploy".into() }
    );
    assert_eq!(
        classify(Path::new(
            "playbooks/clusterdeployments/clusterdeployments.yml"
        )),
        FileRole::Playbook {
            rel: "clusterdeployments/clusterdeployments".into()
        }
    );
}

const PLAYBOOK: &str = r#"
- name: Deploy k8s and helm charts
  hosts: "{{ hostlist | default([]) }}"
  tasks:
    - name: Deploy pre
      ansible.builtin.include_role:
        name: acme_platform.kubernetes.k8s_deployment
      vars:
        k8s_deployment_deployments: "{{ k8s_pre_deployments }}"
      when:
        - vars['k8s_pre_deployments'] is defined
    - name: Deploy helm
      ansible.builtin.include_role:
        name: acme_platform.kubernetes.helm_deployment
      when:
        - vars['helm_deployment_charts'] is defined
    - name: Deploy post
      ansible.builtin.include_role:
        name: acme_platform.kubernetes.k8s_deployment
      vars:
        k8s_deployment_deployments: "{{ k8s_post_deployments }}"
      when:
        - vars['k8s_post_deployments'] is defined
"#;

#[test]
fn playbook_emits_playbook_role_and_include_edges() {
    let r = parse("playbooks/deploy.yml", PLAYBOOK);
    assert_eq!(ids_of(&r, EntityKind::Playbook), vec!["ansible::playbook.deploy"]);

    // Two distinct roles (k8s_deployment used twice → deduped).
    let mut roles = ids_of(&r, EntityKind::Role);
    roles.sort();
    roles.dedup();
    assert_eq!(
        roles,
        vec![
            "ansible::role.acme_platform.kubernetes.helm_deployment",
            "ansible::role.acme_platform.kubernetes.k8s_deployment",
        ]
    );

    let includes: Vec<_> = r
        .relationships
        .iter()
        .filter(|rel| rel.kind == RelationshipKind::Includes)
        .collect();
    // deploy → k8s_deployment (once, deduped) + deploy → helm_deployment.
    assert_eq!(includes.len(), 2);
}

#[test]
fn playbook_requires_the_host_vars_from_when_and_vars() {
    let r = parse("playbooks/deploy.yml", PLAYBOOK);
    let requires: Vec<(String, String)> = r
        .relationships
        .iter()
        .filter(|rel| rel.kind == RelationshipKind::Requires)
        .map(|rel| (rel.source_id.clone(), rel.target_id.clone()))
        .collect();

    // `Requires` is playbook-scoped, and each var appears once (deduped)
    // even though it is referenced in both a `vars:` value and a `when:`.
    let pb = "ansible::playbook.deploy".to_string();
    assert!(requires.contains(&(pb.clone(), "ansible::var.k8s_pre_deployments".into())));
    assert!(requires.contains(&(pb.clone(), "ansible::var.k8s_post_deployments".into())));
    assert!(requires.contains(&(pb, "ansible::var.helm_deployment_charts".into())));
    assert_eq!(requires.len(), 3, "expected 3 unique requires, got {:?}", requires);
}

#[test]
fn playbook_role_links_to_host_group_set_through_var_hub() {
    // The whole point of the orchestration layer: deploy.yml's k8s role
    // must reach a real host group's set through the shared var hub.
    let pb = parse("playbooks/deploy.yml", PLAYBOOK);
    let vars = parse(
        "environments/production/group_vars/acme_portal/k8s.yml",
        K8S_VARS,
    );

    // Role → Requires → var hub id (from the playbook side)...
    let required: Vec<String> = pb
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Requires)
        .map(|r| r.target_id.clone())
        .collect();
    // ...and Set → Provides → the same var hub id (from the var-file side).
    let provided: Vec<String> = vars
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Provides)
        .map(|r| r.target_id.clone())
        .collect();

    assert!(
        required.contains(&"ansible::var.k8s_pre_deployments".to_string()),
        "role should require the pre-deployments var; got {:?}",
        required
    );
    assert!(
        provided.contains(&"ansible::var.k8s_pre_deployments".to_string()),
        "host group set should provide the pre-deployments var; got {:?}",
        provided
    );
}

// ---------------------------------------------------------------------
// Templating dimension: variable definitions + interpolations
// ---------------------------------------------------------------------

#[test]
fn generic_var_file_defines_scalar_variables() {
    const VARS: &str = r#"
docker_registry: registry.example.com
acme_portal_version: 1.2.3
replicas_default: 2
some_list:
  - a
  - b
some_map:
  k: v
"#;
    let r = parse("environments/production/group_vars/all/main.yml", VARS);

    let vars: Vec<String> = r
        .entities
        .iter()
        .filter(|e| e.kind == EntityKind::Variable)
        .map(|e| e.id.clone())
        .collect();
    assert!(vars.contains(&"ansible::var.docker_registry".to_string()));
    assert!(vars.contains(&"ansible::var.acme_portal_version".to_string()));
    assert!(vars.contains(&"ansible::var.replicas_default".to_string()));
    // Non-scalar keys are not substitution variables.
    assert!(!vars.contains(&"ansible::var.some_list".to_string()));
    assert!(!vars.contains(&"ansible::var.some_map".to_string()));

    // Scope Provides the variable, carrying its value.
    let prov = r
        .relationships
        .iter()
        .find(|rel| {
            rel.kind == RelationshipKind::Provides
                && rel.source_id == "ansible::group.all"
                && rel.target_id == "ansible::var.docker_registry"
        })
        .expect("a provides edge for docker_registry");
    assert_eq!(
        prov.metadata.get("value").map(String::as_str),
        Some("registry.example.com")
    );
}

#[test]
fn template_interpolates_variables() {
    const TPL: &str = r#"kind: Deployment
metadata:
  name: {{ service_name_admin }}
spec:
  template:
    spec:
      containers:
        - image: {{ docker_registry }}/app:{{ app_version }}
          env:
            - name: HOSTS
              value: "{{ hostlist | default([]) }}"
            - name: PORT
              value: "{{ lookup('env', 'PORT') }}"
"#;
    let r = parse("environments/production/files/x/dep.yml.j2", TPL);
    let interp: Vec<String> = r
        .relationships
        .iter()
        .filter(|rel| rel.kind == RelationshipKind::Interpolates)
        .map(|rel| rel.target_id.clone())
        .collect();

    assert!(interp.contains(&"ansible::var.service_name_admin".to_string()));
    assert!(interp.contains(&"ansible::var.docker_registry".to_string()));
    assert!(interp.contains(&"ansible::var.app_version".to_string()));
    // Filter is stripped, root var kept.
    assert!(interp.contains(&"ansible::var.hostlist".to_string()));
    // `lookup` (a function) and the quoted string literals must NOT be
    // treated as variables.
    assert!(!interp.iter().any(|t| t.ends_with(".env") || t.ends_with(".PORT") || t.ends_with(".lookup")));
}

#[test]
fn template_variables_are_tagged_but_deployment_hubs_are_not() {
    // A config variable (from a plain var file) is part of the templating
    // dimension → tagged so the UI can hide the layer by default.
    let vf = parse(
        "environments/production/group_vars/all/main.yml",
        "docker_registry: registry.example.com\n",
    );
    let cfg = vf
        .entities
        .iter()
        .find(|e| e.id == "ansible::var.docker_registry")
        .expect("config var node");
    assert!(cfg.tags.contains("template_var"));

    // A deployment-set hub is structural, not templating → untagged.
    let k = parse(
        "environments/production/group_vars/acme_portal/k8s.yml",
        K8S_VARS,
    );
    let hub = k
        .entities
        .iter()
        .find(|e| e.id == "ansible::var.k8s_pre_deployments")
        .expect("deployment hub node");
    assert!(!hub.tags.contains("template_var"));
}

#[test]
fn interpolation_links_template_to_its_definition() {
    // A template interpolating `docker_registry` and a var file defining
    // it compute the same `ansible::var.docker_registry` id, so the
    // Interpolates edge resolves to the Variable the scope Provides.
    let tpl = parse(
        "environments/production/files/x/dep.yml.j2",
        "kind: Deployment\nspec:\n  image: {{ docker_registry }}/x\n",
    );
    let vars = parse(
        "environments/production/group_vars/all/main.yml",
        "docker_registry: registry.example.com\n",
    );

    let target = tpl
        .relationships
        .iter()
        .find(|r| r.kind == RelationshipKind::Interpolates)
        .map(|r| r.target_id.clone())
        .expect("an interpolates edge");
    assert_eq!(target, "ansible::var.docker_registry");
    assert!(vars.entities.iter().any(|e| e.id == target));
}

// ---------------------------------------------------------------------
// The whole point: the two sides link by id across files
// ---------------------------------------------------------------------

#[test]
fn render_edge_target_matches_template_id_across_files() {
    let vars = parse(
        "environments/production/group_vars/acme_portal/k8s.yml",
        K8S_VARS,
    );
    let tpl = parse(
        "environments/production/files/acme-portal-k8s/acme-portal-ds-config.yml.j2",
        TEMPLATE,
    );

    // The id the var side predicts for the template...
    let predicted: Vec<String> = renders_edges(&vars)
        .into_iter()
        .map(|(_, tgt)| tgt)
        .collect();
    // ...must be exactly the id the template side actually emits.
    let emitted = &ids_of(&tpl, EntityKind::TemplateFile)[0];
    assert!(
        predicted.contains(emitted),
        "cross-file link broken: var side predicted {:?}, template emitted {}",
        predicted,
        emitted
    );
}
