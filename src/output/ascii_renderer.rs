//! ASCII/text tree output renderer.

use super::{OutputFormat, Renderer};
use crate::config::Config;
use crate::graph::DependencyGraph;
use crate::models::{CodeEntity, EntityKind, RelationshipKind};
use anyhow::Result;
use std::collections::HashMap;
use std::fmt::Write;

pub struct AsciiRenderer;

const MAX_INLINE_DEPS: usize = 5;

impl Renderer for AsciiRenderer {
    fn format(&self) -> OutputFormat {
        OutputFormat::Ascii
    }

    fn render(&self, graph: &DependencyGraph, config: &Config) -> Result<String> {
        let mut output = String::new();
        let metrics = graph.metrics();

        write_header(
            &mut output,
            config,
            metrics.node_count,
            metrics.edge_count,
            metrics.cycle_count,
        )?;

        if config.display.group_by_file {
            write_grouped_entities(&mut output, graph, config)?;
        } else {
            write_flat_entities(&mut output, graph, config)?;
        }

        write_dependency_summary(&mut output, graph, &metrics)?;
        Ok(output)
    }
}

fn write_header(
    output: &mut String,
    config: &Config,
    entities: usize,
    relationships: usize,
    cycles: usize,
) -> Result<()> {
    writeln!(
        output,
        "╔══════════════════════════════════════════════════════════════╗"
    )?;
    writeln!(
        output,
        "║              CODE DEPENDENCY VISUALIZATION                    ║"
    )?;
    writeln!(
        output,
        "║  Root: {:54} ║",
        truncate(&config.root_path.display().to_string(), 54)
    )?;
    writeln!(
        output,
        "╠══════════════════════════════════════════════════════════════╣"
    )?;
    writeln!(
        output,
        "║  Entities: {:5}  │  Relationships: {:5}  │  Cycles: {:5}  ║",
        entities, relationships, cycles
    )?;
    writeln!(
        output,
        "╚══════════════════════════════════════════════════════════════╝"
    )?;
    writeln!(output)?;
    Ok(())
}

fn write_grouped_entities(
    output: &mut String,
    graph: &DependencyGraph,
    config: &Config,
) -> Result<()> {
    let file_groups = group_entity_ids_by_file(graph);
    for (file_path, entity_ids) in &file_groups {
        writeln!(output, "📄 {}", file_path)?;
        writeln!(output, "│")?;

        let entities: Vec<&CodeEntity> = entity_ids
            .iter()
            .filter_map(|id| graph.get_entity(id))
            .collect();

        let classes: Vec<&CodeEntity> = entities
            .iter()
            .copied()
            .filter(|e| {
                e.kind == EntityKind::Class
                    || e.kind == EntityKind::AbstractClass
                    || e.kind == EntityKind::Struct
            })
            .collect();
        let interfaces: Vec<&CodeEntity> = entities
            .iter()
            .copied()
            .filter(|e| e.kind == EntityKind::Interface)
            .collect();
        let functions: Vec<&CodeEntity> = entities
            .iter()
            .copied()
            .filter(|e| e.kind.is_callable())
            .collect();

        write_class_group(output, &classes, graph, config)?;
        write_simple_group(output, "Interfaces/Traits", &interfaces, "│   ", config)?;
        write_simple_group(output, "Functions/Methods", &functions, "    ", config)?;

        writeln!(output)?;
    }
    Ok(())
}

fn group_entity_ids_by_file(graph: &DependencyGraph) -> HashMap<String, Vec<String>> {
    let mut groups: HashMap<String, Vec<String>> = HashMap::new();
    for entity in graph.entities() {
        groups
            .entry(entity.file_path.display().to_string())
            .or_default()
            .push(entity.id.clone());
    }
    groups
}

fn write_class_group(
    output: &mut String,
    classes: &[&CodeEntity],
    graph: &DependencyGraph,
    config: &Config,
) -> Result<()> {
    if classes.is_empty() {
        return Ok(());
    }
    writeln!(output, "├── Classes/Structs")?;
    for (i, entity) in classes.iter().enumerate() {
        let is_last = i == classes.len() - 1;
        write_entity_line(output, entity, tree_prefix("│   ", is_last), config)?;
        write_entity_deps(output, entity, graph, is_last)?;
    }
    Ok(())
}

fn write_simple_group(
    output: &mut String,
    label: &str,
    entities: &[&CodeEntity],
    indent: &str,
    config: &Config,
) -> Result<()> {
    if entities.is_empty() {
        return Ok(());
    }
    let header_prefix = if indent == "    " {
        "└──"
    } else {
        "├──"
    };
    writeln!(output, "{} {}", header_prefix, label)?;
    for (i, entity) in entities.iter().enumerate() {
        let is_last = i == entities.len() - 1;
        write_entity_line(output, entity, tree_prefix(indent, is_last), config)?;
    }
    Ok(())
}

fn write_entity_line(
    output: &mut String,
    entity: &CodeEntity,
    prefix: String,
    config: &Config,
) -> Result<()> {
    writeln!(
        output,
        "{} {} {}",
        prefix,
        entity_symbol(entity.kind),
        format_entity(entity, config)
    )?;
    Ok(())
}

fn write_entity_deps(
    output: &mut String,
    entity: &CodeEntity,
    graph: &DependencyGraph,
    is_last: bool,
) -> Result<()> {
    let deps = graph.dependencies(&entity.id);
    if deps.is_empty() {
        return Ok(());
    }
    let dep_prefix = if is_last {
        "│       "
    } else {
        "│   │   "
    };
    if deps.len() > MAX_INLINE_DEPS {
        writeln!(output, "{} └─→ [{} dependencies]", dep_prefix, deps.len())?;
    } else {
        for (dep_entity, rel) in &deps {
            writeln!(
                output,
                "{} └─→ {} ({})",
                dep_prefix,
                dep_entity.name,
                rel.kind.display_label()
            )?;
        }
    }
    Ok(())
}

fn tree_prefix(indent: &str, is_last: bool) -> String {
    let branch = if is_last { "└──" } else { "├──" };
    format!("{}{}", indent, branch)
}

fn write_flat_entities(
    output: &mut String,
    graph: &DependencyGraph,
    config: &Config,
) -> Result<()> {
    writeln!(output, "All Entities:")?;
    writeln!(output, "│")?;
    let entities: Vec<_> = graph.entities().collect();
    for (i, entity) in entities.iter().enumerate() {
        let is_last = i == entities.len() - 1;
        write_entity_line(output, entity, tree_prefix("", is_last), config)?;
    }
    Ok(())
}

fn write_dependency_summary(
    output: &mut String,
    graph: &DependencyGraph,
    metrics: &crate::graph::GraphMetrics,
) -> Result<()> {
    writeln!(output)?;
    writeln!(
        output,
        "═══════════════════════════════════════════════════════════════"
    )?;
    writeln!(output, "DEPENDENCY SUMMARY")?;
    writeln!(
        output,
        "═══════════════════════════════════════════════════════════════"
    )?;

    if !metrics.most_connected.is_empty() {
        writeln!(output, "\nMost Connected Entities:")?;
        for (i, (id, count)) in metrics.most_connected.iter().take(5).enumerate() {
            if let Some(entity) = graph.get_entity(id) {
                writeln!(
                    output,
                    "  {}. {} ({} connections)",
                    i + 1,
                    entity.name,
                    count
                )?;
            }
        }
    }

    writeln!(output, "\nRelationship Types:")?;
    let mut rel_counts: HashMap<RelationshipKind, usize> = HashMap::new();
    for rel in graph.relationships() {
        *rel_counts.entry(rel.kind).or_insert(0) += 1;
    }
    for (kind, count) in &rel_counts {
        writeln!(output, "  • {}: {}", kind.display_label(), count)?;
    }

    if metrics.cycle_count > 0 {
        writeln!(
            output,
            "\n⚠️  WARNING: {} circular dependencies detected!",
            metrics.cycle_count
        )?;
    }
    Ok(())
}

fn entity_symbol(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::File => "📄",
        EntityKind::Module => "📦",
        EntityKind::Class => "🔷",
        EntityKind::AbstractClass => "🔷ₐ",
        EntityKind::Struct => "🔶",
        EntityKind::Interface => "◯",
        EntityKind::Enum => "◇",
        EntityKind::Function => "ƒ",
        EntityKind::Method => "ƒ",
        EntityKind::Constant => "•",
        EntityKind::Variable => "•",
        EntityKind::TypeAlias => "T",
        EntityKind::Macro => "⚙",
        _ => "•",
    }
}

fn format_entity(entity: &crate::models::CodeEntity, config: &Config) -> String {
    let mut result = String::new();

    // Visibility indicator
    let vis = match entity.visibility {
        crate::models::Visibility::Public => "+",
        crate::models::Visibility::Private => "-",
        crate::models::Visibility::Protected => "#",
        crate::models::Visibility::Internal => "~",
        crate::models::Visibility::Crate => "~",
    };

    result.push_str(vis);
    result.push(' ');
    result.push_str(&entity.name);

    // Parameters for callables
    if config.display.show_parameters && entity.kind.is_callable() {
        result.push('(');
        let params: Vec<String> = entity
            .parameters
            .iter()
            .map(|p| {
                if let Some(t) = &p.type_name {
                    format!("{}: {}", p.name, t)
                } else {
                    p.name.clone()
                }
            })
            .collect();
        result.push_str(&params.join(", "));
        result.push(')');
    }

    // Return type
    if config.display.show_return_types {
        if let Some(ret) = &entity.return_type {
            result.push_str(" → ");
            result.push_str(ret);
        }
    }

    // Line number
    if config.display.show_line_numbers {
        result.push_str(&format!(" [L{}]", entity.span.start.line + 1));
    }

    result
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        format!("{:width$}", s, width = max_len)
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}
