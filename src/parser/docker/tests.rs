//! Tests for the docker topology parser. Fixtures mirror the shapes a
//! real repo uses: a multi-stage build, a Compose file beside it, and the
//! `build.target` that ties one to the other.

use super::*;
use crate::models::RelationshipKind;
use crate::parser::language_parser::LanguageParser;
use std::path::Path;

fn parse(path: &str, content: &str) -> ParseResult {
    DockerParser::new()
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

fn names_of_kind(result: &ParseResult, kind: EntityKind) -> Vec<String> {
    result
        .entities
        .iter()
        .filter(|e| e.kind == kind)
        .map(|e| e.name.clone())
        .collect()
}

fn edges(result: &ParseResult, kind: RelationshipKind) -> Vec<(String, String)> {
    result
        .relationships
        .iter()
        .filter(|r| r.kind == kind)
        .map(|r| (r.source_id.clone(), r.target_id.clone()))
        .collect()
}

// ---------------------------------------------------------------------
// Classification
// ---------------------------------------------------------------------

#[test]
fn classifies_dockerfile_spellings() {
    for name in [
        "Dockerfile",
        "dockerfile",
        "Dockerfile.prod",
        "prod.Dockerfile",
        "Containerfile",
        "app/Dockerfile",
        "deploy/Dockerfile.ci",
    ] {
        assert_eq!(
            classify(Path::new(name)),
            FileRole::Dockerfile,
            "`{name}` should classify as a Dockerfile"
        );
    }
}

#[test]
fn classifies_compose_spellings() {
    for name in [
        "compose.yaml",
        "compose.yml",
        "docker-compose.yml",
        "docker-compose.yaml",
        "docker-compose.prod.yml",
        "compose.override.yaml",
        "deploy/docker-compose.yml",
    ] {
        assert_eq!(
            classify(Path::new(name)),
            FileRole::Compose,
            "`{name}` should classify as a Compose file"
        );
    }
}

/// The filename rule has to stay narrow: `.yml` belongs to no one, and a
/// repo is full of YAML that is not Compose.
#[test]
fn leaves_other_files_alone() {
    for name in [
        "README.md",
        "values.yml",
        "mycompose.yml",
        "docker-compose.txt",
        "compose.json",
        "src/main.rs",
        "docker/entrypoint.sh",
        // Code *about* Dockerfiles is not a Dockerfile. This module's own
        // `dockerfile.rs` was claimed by the first version of the rule.
        "src/parser/docker/dockerfile.rs",
        "dockerfile.ts",
        "Containerfile.go",
    ] {
        assert_eq!(
            classify(Path::new(name)),
            FileRole::Other,
            "`{name}` should not be claimed by the docker parser"
        );
    }
}

// ---------------------------------------------------------------------
// Dockerfile
// ---------------------------------------------------------------------

#[test]
fn a_single_stage_inherits_its_base_image() {
    let result = parse("Dockerfile", "FROM node:20\nRUN npm ci\n");

    assert_eq!(names_of_kind(&result, EntityKind::BaseImage), ["node:20"]);
    assert_eq!(
        edges(&result, RelationshipKind::Inherits),
        [(
            "docker::stage.Dockerfile#0".to_string(),
            "docker::image.node:20".to_string()
        )]
    );
}

#[test]
fn an_unnamed_stage_is_addressed_by_index() {
    let result = parse("Dockerfile", "FROM node:20\n");
    assert_eq!(
        ids_of(&result, EntityKind::Stage),
        ["docker::stage.Dockerfile#0"]
    );
    assert_eq!(names_of_kind(&result, EntityKind::Stage), ["stage 0"]);
}

#[test]
fn the_file_contains_its_stages() {
    let result = parse(
        "app/Dockerfile",
        "FROM node:20 AS builder\nFROM nginx AS runtime\n",
    );

    let contains = edges(&result, RelationshipKind::Contains);
    assert!(contains.contains(&(
        "docker::file.app/Dockerfile".to_string(),
        "docker::stage.app/Dockerfile#builder".to_string()
    )));
    assert!(contains.contains(&(
        "docker::file.app/Dockerfile".to_string(),
        "docker::stage.app/Dockerfile#runtime".to_string()
    )));
}

#[test]
fn copy_from_links_one_stage_to_another() {
    let result = parse(
        "Dockerfile",
        r#"
FROM node:20 AS builder
RUN npm run build

FROM nginx:alpine AS runtime
COPY --from=builder /app/dist /usr/share/nginx/html
"#,
    );

    assert_eq!(
        edges(&result, RelationshipKind::CopiesFrom),
        [(
            "docker::stage.Dockerfile#runtime".to_string(),
            "docker::stage.Dockerfile#builder".to_string()
        )]
    );
}

#[test]
fn copy_from_resolves_a_stage_index() {
    let result = parse(
        "Dockerfile",
        "FROM node:20\nFROM nginx\nCOPY --from=0 /app /app\n",
    );

    assert_eq!(
        edges(&result, RelationshipKind::CopiesFrom),
        [(
            "docker::stage.Dockerfile#1".to_string(),
            "docker::stage.Dockerfile#0".to_string()
        )]
    );
}

/// `COPY --from=` naming something the file does not declare is a copy
/// out of an external image, not a dangling edge.
#[test]
fn copy_from_an_unknown_name_is_an_external_image() {
    let result = parse(
        "Dockerfile",
        "FROM scratch\nCOPY --from=ghcr.io/acme/tools:1 /bin/tool /bin/tool\n",
    );

    assert!(
        names_of_kind(&result, EntityKind::BaseImage).contains(&"ghcr.io/acme/tools:1".to_string())
    );
    assert_eq!(
        edges(&result, RelationshipKind::CopiesFrom),
        [(
            "docker::stage.Dockerfile#0".to_string(),
            "docker::image.ghcr.io/acme/tools:1".to_string()
        )]
    );
}

#[test]
fn run_mount_from_is_a_copy_edge_too() {
    let result = parse(
        "Dockerfile",
        r#"
FROM golang AS deps
FROM golang AS build
RUN --mount=type=bind,from=deps,source=/go/pkg,target=/go/pkg go build ./...
"#,
    );

    assert_eq!(
        edges(&result, RelationshipKind::CopiesFrom),
        [(
            "docker::stage.Dockerfile#build".to_string(),
            "docker::stage.Dockerfile#deps".to_string()
        )]
    );
}

/// A `FROM` naming an earlier stage extends it. No `BaseImage` node is
/// invented for a name the file already declares.
#[test]
fn a_stage_can_inherit_a_sibling_stage() {
    let result = parse("Dockerfile", "FROM node:20 AS base\nFROM base AS test\n");

    assert_eq!(names_of_kind(&result, EntityKind::BaseImage), ["node:20"]);
    let inherits = edges(&result, RelationshipKind::Inherits);
    assert!(inherits.contains(&(
        "docker::stage.Dockerfile#test".to_string(),
        "docker::stage.Dockerfile#base".to_string()
    )));
}

#[test]
fn a_platform_flag_does_not_become_the_image() {
    let result = parse("Dockerfile", "FROM --platform=linux/amd64 alpine:3.20\n");
    assert_eq!(
        names_of_kind(&result, EntityKind::BaseImage),
        ["alpine:3.20"]
    );
}

#[test]
fn comments_and_continuations_fold_into_one_instruction() {
    let result = parse(
        "Dockerfile",
        r#"
# syntax=docker/dockerfile:1
FROM node:20 AS builder
FROM nginx
COPY \
    --from=builder \
    /app/dist \
    /usr/share/nginx/html
"#,
    );

    assert_eq!(
        edges(&result, RelationshipKind::CopiesFrom),
        [(
            "docker::stage.Dockerfile#1".to_string(),
            "docker::stage.Dockerfile#builder".to_string()
        )]
    );
}

/// One node per image ref, however many stages start from it — that is
/// what makes "which external images are we on" answerable.
#[test]
fn one_image_node_serves_every_reference_to_it() {
    let result = parse(
        "Dockerfile",
        "FROM node:20 AS a\nFROM node:20 AS b\nFROM node:20 AS c\n",
    );

    assert_eq!(ids_of(&result, EntityKind::BaseImage).len(), 1);
    assert_eq!(edges(&result, RelationshipKind::Inherits).len(), 3);
}

// ---------------------------------------------------------------------
// Compose
// ---------------------------------------------------------------------

const COMPOSE: &str = r#"
services:
  api:
    build:
      context: ./app
      dockerfile: Dockerfile
      target: runtime
    depends_on:
      - db
    volumes:
      - pgdata:/var/lib/data
      - ./src:/app/src
    networks: [backend]
  db:
    image: postgres:16
    volumes:
      - pgdata:/var/lib/postgresql/data
volumes:
  pgdata:
networks:
  backend:
"#;

#[test]
fn compose_emits_its_services() {
    let result = parse("compose.yaml", COMPOSE);
    assert_eq!(names_of_kind(&result, EntityKind::Service), ["api", "db"]);
}

#[test]
fn a_service_without_a_build_instantiates_the_image_it_pulls() {
    let result = parse("compose.yaml", COMPOSE);
    assert_eq!(
        edges(&result, RelationshipKind::Instantiates),
        [(
            "docker::service.compose.yaml#db".to_string(),
            "docker::image.postgres:16".to_string()
        )]
    );
}

#[test]
fn depends_on_links_two_services() {
    let result = parse("compose.yaml", COMPOSE);
    assert_eq!(
        edges(&result, RelationshipKind::DependsOn),
        [(
            "docker::service.compose.yaml#api".to_string(),
            "docker::service.compose.yaml#db".to_string()
        )]
    );
}

/// The long form carries a `condition:`; the edge it states is the same.
#[test]
fn depends_on_accepts_the_mapping_form() {
    let result = parse(
        "compose.yaml",
        r#"
services:
  api:
    image: api
    depends_on:
      db:
        condition: service_healthy
  db:
    image: postgres:16
"#,
    );

    assert_eq!(
        edges(&result, RelationshipKind::DependsOn),
        [(
            "docker::service.compose.yaml#api".to_string(),
            "docker::service.compose.yaml#db".to_string()
        )]
    );
}

/// A named volume is a shared resource worth an edge. A bind mount is a
/// host path, and nothing in the graph should point at it.
#[test]
fn named_volumes_link_and_bind_mounts_do_not() {
    let result = parse("compose.yaml", COMPOSE);
    let requires = edges(&result, RelationshipKind::Requires);

    assert!(requires.contains(&(
        "docker::service.compose.yaml#api".to_string(),
        "docker::volume.compose.yaml#pgdata".to_string()
    )));
    assert!(requires.contains(&(
        "docker::service.compose.yaml#db".to_string(),
        "docker::volume.compose.yaml#pgdata".to_string()
    )));
    assert!(
        !requires.iter().any(|(_, t)| t.contains("src")),
        "a bind mount must not become a volume edge: {requires:?}"
    );
}

#[test]
fn a_service_joins_the_networks_it_declares() {
    let result = parse("compose.yaml", COMPOSE);
    assert!(edges(&result, RelationshipKind::Requires).contains(&(
        "docker::service.compose.yaml#api".to_string(),
        "docker::network.compose.yaml#backend".to_string()
    )));
}

// ---------------------------------------------------------------------
// The cross-file edge
// ---------------------------------------------------------------------

/// The load-bearing test, and the reason the id scheme exists: the
/// Compose side predicts a stage id it cannot see, and the Dockerfile
/// side computes the same one. They only meet post-merge, so nothing but
/// a shared formula makes the edge resolve.
#[test]
fn a_build_target_lands_on_the_stage_the_dockerfile_declares() {
    let compose = parse("compose.yaml", COMPOSE);
    let dockerfile = parse(
        "app/Dockerfile",
        "FROM node:20 AS builder\nFROM nginx AS runtime\n",
    );

    let built = edges(&compose, RelationshipKind::BuildsFrom);
    assert_eq!(built.len(), 1);
    let (_, target) = &built[0];

    assert!(
        ids_of(&dockerfile, EntityKind::Stage).contains(target),
        "compose predicted `{target}`, which the Dockerfile never emitted: {:?}",
        ids_of(&dockerfile, EntityKind::Stage)
    );
}

/// Without a `target:`, the built stage is the file's last — which the
/// Compose side cannot know. The edge points at the file rather than
/// guessing a stage.
#[test]
fn a_build_without_a_target_lands_on_the_dockerfile_itself() {
    let compose = parse("compose.yaml", "services:\n  api:\n    build: ./app\n");
    let dockerfile = parse("app/Dockerfile", "FROM node:20\n");

    assert_eq!(
        edges(&compose, RelationshipKind::BuildsFrom),
        [(
            "docker::service.compose.yaml#api".to_string(),
            "docker::file.app/Dockerfile".to_string()
        )]
    );
    assert!(
        ids_of(&dockerfile, EntityKind::File).contains(&"docker::file.app/Dockerfile".to_string())
    );
}

#[test]
fn a_build_context_resolves_relative_to_the_compose_file() {
    let result = parse(
        "deploy/compose.yaml",
        "services:\n  api:\n    build:\n      context: ../app\n      target: runtime\n",
    );

    assert_eq!(
        edges(&result, RelationshipKind::BuildsFrom),
        [(
            "docker::service.deploy/compose.yaml#api".to_string(),
            "docker::stage.app/Dockerfile#runtime".to_string()
        )]
    );
}

/// A `build:` plus an `image:` names the tag the build produces. Writing
/// a dependency on it would point the arrow backwards.
#[test]
fn a_build_plus_an_image_tag_is_not_a_pull() {
    let result = parse(
        "compose.yaml",
        "services:\n  api:\n    build: .\n    image: acme/api:latest\n",
    );

    assert!(edges(&result, RelationshipKind::Instantiates).is_empty());
    assert_eq!(edges(&result, RelationshipKind::BuildsFrom).len(), 1);
}

// ---------------------------------------------------------------------
// Guards
// ---------------------------------------------------------------------

/// The filename is a candidate, not a verdict. A `compose.yml` with no
/// `services:` belongs to something else.
#[test]
fn a_compose_named_file_without_services_yields_nothing() {
    let result = parse("compose.yml", "instruments:\n  - piano\n  - cello\n");
    assert!(result.entities.is_empty(), "{:?}", result.entities);
    assert!(result.relationships.is_empty());
}

#[test]
fn broken_yaml_warns_instead_of_failing() {
    let result = parse("compose.yaml", "services: [api, db\n");
    assert!(result.entities.is_empty());
    assert_eq!(result.warnings.len(), 1);
}

#[test]
fn an_instruction_before_any_from_is_skipped() {
    let result = parse("Dockerfile", "ARG VERSION=1\nCOPY --from=builder /a /b\n");
    assert!(edges(&result, RelationshipKind::CopiesFrom).is_empty());
}

#[test]
fn normalize_folds_parent_components() {
    assert_eq!(
        normalize(Path::new("deploy/../app/Dockerfile")),
        "app/Dockerfile"
    );
    assert_eq!(normalize(Path::new("./app/./Dockerfile")), "app/Dockerfile");
}

#[test]
fn every_docker_entity_is_tagged() {
    let result = parse("compose.yaml", COMPOSE);
    assert!(result.entities.iter().all(|e| e.tags.contains("docker")));
}

// ---------------------------------------------------------------------
// Spans and source text (DK-001)
// ---------------------------------------------------------------------

fn entity<'a>(result: &'a ParseResult, name: &str) -> &'a crate::models::CodeEntity {
    result
        .entities
        .iter()
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("no entity named `{name}`"))
}

const MULTISTAGE: &str = r#"FROM node:20 AS builder
WORKDIR /app
RUN npm run build

FROM nginx:alpine AS runtime
COPY --from=builder /app/dist /usr/share/nginx/html
"#;

/// A stage runs from its `FROM` to the line before the next one — the
/// lines that define it, which is what the details pane shows.
#[test]
fn a_stage_spans_its_own_instructions() {
    let result = parse("Dockerfile", MULTISTAGE);
    let builder = entity(&result, "builder");

    assert_eq!(builder.span.start.line, 0);
    assert_eq!(builder.span.end.line, 2);
    assert_eq!(
        builder.source_code.as_deref(),
        Some("FROM node:20 AS builder\nWORKDIR /app\nRUN npm run build")
    );
}

/// The last stage runs to the end of the file.
#[test]
fn the_final_stage_spans_to_the_end() {
    let result = parse("Dockerfile", MULTISTAGE);
    let runtime = entity(&result, "runtime");

    assert_eq!(runtime.span.start.line, 4);
    assert_eq!(
        runtime.source_code.as_deref(),
        Some("FROM nginx:alpine AS runtime\nCOPY --from=builder /app/dist /usr/share/nginx/html")
    );
}

/// The span's offsets have to address the same bytes the text came from —
/// anything slicing the file by them must get the entity back.
#[test]
fn span_offsets_address_the_source_they_report() {
    let result = parse("Dockerfile", MULTISTAGE);
    let builder = entity(&result, "builder");
    let sliced = &MULTISTAGE[builder.span.start.offset..builder.span.end.offset];

    assert_eq!(sliced.trim_end(), builder.source_code.as_deref().unwrap());
}

#[test]
fn a_service_carries_its_yaml_block() {
    let result = parse("compose.yaml", COMPOSE);
    let db = entity(&result, "db");

    assert_eq!(
        db.source_code.as_deref(),
        Some(
            "  db:\n    image: postgres:16\n    volumes:\n      - pgdata:/var/lib/postgresql/data"
        )
    );
    assert!(db.span.end.line > db.span.start.line);
}

#[test]
fn a_volume_carries_the_line_that_declares_it() {
    let result = parse("compose.yaml", COMPOSE);
    assert_eq!(
        entity(&result, "pgdata").source_code.as_deref(),
        Some("  pgdata:")
    );
}

/// An external image is referenced, never defined. Its span points at the
/// line that pulled it in, so selecting it still shows something true.
#[test]
fn an_external_image_points_at_its_first_reference() {
    let result = parse("compose.yaml", COMPOSE);
    assert_eq!(
        entity(&result, "postgres:16").source_code.as_deref(),
        Some("    image: postgres:16")
    );
}

/// Every node the pane can select has to have something to show. This is
/// the regression the whole pass exists for.
#[test]
fn no_entity_is_left_without_source() {
    for (path, content) in [("Dockerfile", MULTISTAGE), ("compose.yaml", COMPOSE)] {
        let result = parse(path, content);
        for e in &result.entities {
            let source = e.source_code.as_deref().unwrap_or("");
            assert!(
                !source.trim().is_empty(),
                "`{}` ({:?}) in {path} has no source to display",
                e.name,
                e.kind
            );
        }
    }
}
