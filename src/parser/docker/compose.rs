//! The Compose side: services, what builds or pulls them, what they wait
//! on, and the volumes and networks they share.
//!
//! Meaning comes from `serde_yaml`; line ranges come from
//! [`super::blocks`], which re-reads the text for indentation alone. The
//! two are kept apart because only one of them can be wrong in a way that
//! matters — a missed line range costs a blank details pane, a misread
//! mapping costs a wrong graph.

use super::blocks::{self, Block};
use super::{ensure_image, entity_at, file_id, link, new_entity, normalize, stage_id, Lines};
use crate::models::{EntityKind, RelationshipKind};
use crate::parser::language_parser::ParseResult;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Everything the per-service walk needs, so no handler below grows a
/// nine-parameter head.
struct Compose<'a> {
    path: &'a Path,
    content: &'a str,
    lines: &'a Lines<'a>,
    /// Id of the `File` entity every service hangs off.
    file: String,
    volumes: BTreeSet<String>,
    networks: BTreeSet<String>,
}

pub(super) fn parse(path: &Path, content: &str, result: &mut ParseResult) {
    let doc: serde_yaml::Value = match serde_yaml::from_str(content) {
        Ok(v) => v,
        Err(e) => {
            result.add_warning(format!(
                "docker: YAML parse error in {}: {e}",
                path.display()
            ));
            return;
        }
    };

    // The filename said Compose; this is what makes it Compose. A repo
    // whose `compose.yml` means something else yields nothing rather than
    // a graph of imaginary services.
    let Some(services) = doc.get("services").and_then(|v| v.as_mapping()) else {
        return;
    };

    let lines = Lines::index(content);
    let file = super::emit_file(path, &lines, result);
    let mut compose = Compose {
        path,
        content,
        lines: &lines,
        file,
        volumes: BTreeSet::new(),
        networks: BTreeSet::new(),
    };

    compose.volumes = compose.emit_resources(&doc, "volumes", EntityKind::Volume, result);
    compose.networks = compose.emit_resources(&doc, "networks", EntityKind::Network, result);

    let blocks = blocks::index(content, "services");
    for (name, spec) in services {
        let Some(name) = name.as_str() else { continue };
        compose.emit_service(name, spec, blocks.get(name).copied(), result);
    }
}

impl Compose<'_> {
    /// Emit the entities for a top-level `volumes:` or `networks:` block
    /// and return the names it declared.
    ///
    /// The names matter as much as the nodes: a service's `volumes:` list
    /// mixes named volumes with bind mounts, and the only thing that tells
    /// them apart is whether the source was declared up here.
    fn emit_resources(
        &self,
        doc: &serde_yaml::Value,
        key: &str,
        kind: EntityKind,
        result: &mut ParseResult,
    ) -> BTreeSet<String> {
        let Some(block) = doc.get(key).and_then(|v| v.as_mapping()) else {
            return BTreeSet::new();
        };
        let located: BTreeMap<String, Block> = blocks::index(self.content, key);
        let declared: BTreeSet<String> = block
            .keys()
            .filter_map(|k| k.as_str())
            .map(str::to_string)
            .collect();
        for name in &declared {
            self.emit_resource(name, kind, located.get(name).copied(), result);
        }
        declared
    }

    /// One named volume or network: the node, and the `Contains` edge
    /// hanging it off the file that declares it.
    fn emit_resource(
        &self,
        name: &str,
        kind: EntityKind,
        block: Option<Block>,
        result: &mut ParseResult,
    ) {
        let id = resource_id(kind, self.path, name);
        let mut entity = self.entity(name, kind, block);
        entity.id = id.clone();
        entity.qualified_name = format!("{}#{name}", normalize(self.path));
        result.add_entity(entity);
        link(&self.file, &id, RelationshipKind::Contains, result);
    }

    fn emit_service(
        &self,
        name: &str,
        spec: &serde_yaml::Value,
        block: Option<Block>,
        result: &mut ParseResult,
    ) {
        let id = service_id(self.path, name);
        let mut entity = self.entity(name, EntityKind::Service, block);
        entity.id = id.clone();
        entity.qualified_name = format!("{}#{name}", normalize(self.path));
        result.add_entity(entity);
        link(&self.file, &id, RelationshipKind::Contains, result);

        self.link_origin(&id, spec, block, result);

        // `depends_on` accepts both a list of names and a map keyed by name
        // with a `condition:`; the edge is the same either way.
        for target in names_of(spec.get("depends_on")) {
            link(
                &id,
                &service_id(self.path, &target),
                RelationshipKind::DependsOn,
                result,
            );
        }
        self.link_attachments(&id, spec, result);
    }

    /// Build an entity spanning `block`, or — when the locating pass could
    /// not find one — a zero-width node at the top of the file. The graph
    /// is still correct without a range; only the details pane is poorer.
    fn entity(
        &self,
        name: &str,
        kind: EntityKind,
        block: Option<Block>,
    ) -> crate::models::CodeEntity {
        match block {
            Some(b) => entity_at(name, kind, self.path, self.lines, b.first, b.last),
            None => new_entity(name, kind, self.path, self.lines.span(0, 0), None),
        }
    }

    /// Where a service's image comes from: a `build:` block pointing into
    /// a Dockerfile, or an `image:` it pulls.
    ///
    /// Not both. A service with a `build:` may also carry an `image:`, but
    /// there it names the tag the build *produces* — writing a dependency
    /// on it would invert the arrow.
    fn link_origin(
        &self,
        service: &str,
        spec: &serde_yaml::Value,
        block: Option<Block>,
        result: &mut ParseResult,
    ) {
        if let Some(build) = spec.get("build") {
            if let Some(target) = build_target(self.path, build) {
                link(service, &target, RelationshipKind::BuildsFrom, result);
            }
            return;
        }
        if let Some(image) = spec.get("image").and_then(|v| v.as_str()) {
            let line = self.key_line(block, "image").unwrap_or(0);
            let target = ensure_image(image, self.path, self.lines, line, result);
            link(service, &target, RelationshipKind::Instantiates, result);
        }
    }

    /// The line inside `block` that declares `key`, for pointing an
    /// external image at the `image:` that pulled it rather than at the
    /// top of the service.
    fn key_line(&self, block: Option<Block>, key: &str) -> Option<usize> {
        let block = block?;
        let prefix = format!("{key}:");
        self.content
            .lines()
            .enumerate()
            .skip(block.first)
            .take(block.last + 1 - block.first)
            .find(|(_, raw)| raw.trim().starts_with(&prefix))
            .map(|(index, _)| index)
    }

    /// Link a service to the named volumes it mounts and the networks it
    /// joins — the shared resources that couple two services together
    /// without either one naming the other.
    fn link_attachments(&self, service: &str, spec: &serde_yaml::Value, result: &mut ParseResult) {
        for source in mount_sources(spec.get("volumes")) {
            if self.volumes.contains(&source) {
                let id = resource_id(EntityKind::Volume, self.path, &source);
                link(service, &id, RelationshipKind::Requires, result);
            }
        }
        for name in names_of(spec.get("networks")) {
            if self.networks.contains(&name) {
                let id = resource_id(EntityKind::Network, self.path, &name);
                link(service, &id, RelationshipKind::Requires, result);
            }
        }
    }
}

fn resource_id(kind: EntityKind, path: &Path, name: &str) -> String {
    let prefix = match kind {
        EntityKind::Volume => "volume",
        _ => "network",
    };
    format!("docker::{prefix}.{}#{name}", normalize(path))
}

fn service_id(path: &Path, name: &str) -> String {
    format!("docker::service.{}#{name}", normalize(path))
}

/// Resolve a `build:` block to the id it depends on.
///
/// With a `target:`, that is the exact stage. Without one, Docker builds
/// the file's last stage — which this side cannot know without reading
/// the Dockerfile — so the edge lands on the Dockerfile's `File` node
/// instead. See the module header for why a guess would be worse.
fn build_target(compose: &Path, build: &serde_yaml::Value) -> Option<String> {
    let (context, dockerfile, target) = match build.as_str() {
        Some(context) => (context, None, None),
        None => (
            build.get("context").and_then(|v| v.as_str()).unwrap_or("."),
            build.get("dockerfile").and_then(|v| v.as_str()),
            build.get("target").and_then(|v| v.as_str()),
        ),
    };

    let dir = compose.parent().unwrap_or_else(|| Path::new("."));
    let resolved: PathBuf = dir.join(context).join(dockerfile.unwrap_or("Dockerfile"));

    Some(match target {
        Some(stage) => stage_id(&resolved, stage),
        None => file_id(&resolved),
    })
}

/// The names in a value that is either a sequence of strings or a mapping
/// keyed by name — the two shapes `depends_on` and `networks` each accept.
fn names_of(value: Option<&serde_yaml::Value>) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };
    if let Some(seq) = value.as_sequence() {
        return seq
            .iter()
            .filter_map(|v| v.as_str())
            .map(str::to_string)
            .collect();
    }
    value
        .as_mapping()
        .map(|m| {
            m.keys()
                .filter_map(|k| k.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The source side of each entry in a service's `volumes:` list, for both
/// spellings: the `source:target` short form and the `{type, source,
/// target}` long one.
///
/// Bind mounts are dropped here rather than filtered later: a source
/// beginning with `.`, `/` or `~` is a host path, not a volume name, and
/// nothing in the graph should point at it.
fn mount_sources(value: Option<&serde_yaml::Value>) -> Vec<String> {
    let Some(seq) = value.and_then(|v| v.as_sequence()) else {
        return Vec::new();
    };
    seq.iter()
        .filter_map(|entry| match entry.as_str() {
            Some(short) => short.split(':').next().map(str::to_string),
            None => entry
                .get("source")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        })
        .filter(|source| !source.starts_with(['.', '/', '~']))
        .collect()
}
