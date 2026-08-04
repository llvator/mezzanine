//! Mermaid diagram output renderer.

use super::{OutputFormat, Renderer};
use crate::config::Config;
use crate::graph::DependencyGraph;
use crate::models::{EntityKind, RelationshipKind};
use anyhow::Result;
use std::collections::HashMap;
use std::fmt::Write;

pub struct MermaidRenderer;

impl Renderer for MermaidRenderer {
    fn format(&self) -> OutputFormat {
        OutputFormat::Mermaid
    }

    fn render(&self, graph: &DependencyGraph, config: &Config) -> Result<String> {
        let mut output = String::new();
        let direction = layout_direction(config);

        writeln!(output, "```mermaid")?;
        writeln!(output, "flowchart {}", direction)?;
        writeln!(output)?;

        render_nodes(&mut output, graph, config)?;
        render_relationships(&mut output, graph)?;
        render_styles(&mut output, graph)?;

        writeln!(output, "```")?;
        Ok(output)
    }
}

fn layout_direction(config: &Config) -> &'static str {
    match config.display.layout_direction {
        crate::config::LayoutDirection::TopToBottom => "TB",
        crate::config::LayoutDirection::LeftToRight => "LR",
        crate::config::LayoutDirection::BottomToTop => "BT",
        crate::config::LayoutDirection::RightToLeft => "RL",
    }
}

fn render_nodes(out: &mut String, graph: &DependencyGraph, config: &Config) -> Result<()> {
    if config.display.group_by_file {
        render_nodes_grouped(out, graph, config)
    } else {
        render_nodes_flat(out, graph, config)
    }
}

fn render_nodes_grouped(out: &mut String, graph: &DependencyGraph, config: &Config) -> Result<()> {
    let mut file_groups: HashMap<String, Vec<String>> = HashMap::new();
    for entity in graph.entities() {
        let file_key = entity
            .file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        file_groups.entry(file_key).or_default().push(entity.id.clone());
    }
    for (file_name, entity_ids) in &file_groups {
        let subgraph_id = sanitize_id(file_name);
        writeln!(out, "    subgraph {}[\"📄 {}\"]", subgraph_id, file_name)?;
        for entity_id in entity_ids {
            if let Some(entity) = graph.get_entity(entity_id) {
                write_node(out, entity, config, "        ")?;
            }
        }
        writeln!(out, "    end")?;
        writeln!(out)?;
    }
    Ok(())
}

fn render_nodes_flat(out: &mut String, graph: &DependencyGraph, config: &Config) -> Result<()> {
    writeln!(out, "    %% Nodes")?;
    for entity in graph.entities() {
        write_node(out, entity, config, "    ")?;
    }
    Ok(())
}

fn write_node(out: &mut String, entity: &crate::models::CodeEntity, config: &Config, indent: &str) -> Result<()> {
    let node_id = sanitize_id(&entity.id);
    let label = format_entity_label(entity, config);
    let shape = entity_shape(entity.kind);
    writeln!(out, "{}{}{}", indent, node_id, shape(&label))?;
    Ok(())
}

fn render_relationships(out: &mut String, graph: &DependencyGraph) -> Result<()> {
    writeln!(out)?;
    writeln!(out, "    %% Relationships")?;
    for rel in graph.relationships() {
        let source_id = sanitize_id(&rel.source_id);
        let target_id = sanitize_id(&rel.target_id);
        let arrow = relationship_arrow(rel.kind);
        let label = rel.label.as_deref().unwrap_or(rel.kind.display_label());
        writeln!(out, "    {} {}|{}| {}", source_id, arrow, label, target_id)?;
    }
    Ok(())
}

/// Mermaid style class definitions — one per entity kind.
const KIND_STYLES: &[(&[EntityKind], &str, &str)] = &[
    (&[EntityKind::Class], "classStyle", "fill:#E3F2FD,stroke:#1976D2"),
    (&[EntityKind::AbstractClass], "abstractClassStyle", "fill:#BBDEFB,stroke:#1565C0,stroke-dasharray:5 5"),
    (&[EntityKind::Struct], "structStyle", "fill:#E8F5E9,stroke:#388E3C"),
    (&[EntityKind::Interface], "interfaceStyle", "fill:#FFF3E0,stroke:#F57C00"),
    (&[EntityKind::Function, EntityKind::Method], "functionStyle", "fill:#FFFDE7,stroke:#FBC02D"),
    (&[EntityKind::Module], "moduleStyle", "fill:#ECEFF1,stroke:#607D8B"),
];

fn render_styles(out: &mut String, graph: &DependencyGraph) -> Result<()> {
    writeln!(out)?;
    writeln!(out, "    %% Styling")?;
    for &(kinds, class_name, style) in KIND_STYLES {
        let nodes: Vec<String> = graph.entities()
            .filter(|e| kinds.contains(&e.kind))
            .map(|e| sanitize_id(&e.id))
            .collect();
        if !nodes.is_empty() {
            writeln!(out, "    classDef {} {}", class_name, style)?;
            writeln!(out, "    class {} {}", nodes.join(","), class_name)?;
        }
    }
    Ok(())
}

fn format_entity_label(entity: &crate::models::CodeEntity, config: &Config) -> String {
    let mut label = String::new();
    
    // Icon based on kind
    let icon = match entity.kind {
        EntityKind::Class => "📦",
        EntityKind::AbstractClass => "📦ₐ",
        EntityKind::Struct => "🔷",
        EntityKind::Interface => "🔶",
        EntityKind::Enum => "◇",
        EntityKind::Function => "ƒ",
        EntityKind::Method => "ƒ",
        EntityKind::Module => "📁",
        EntityKind::File => "📄",
        _ => "",
    };
    
    label.push_str(icon);
    label.push(' ');
    
    // Name
    if config.display.show_qualified_names {
        label.push_str(&entity.qualified_name);
    } else {
        label.push_str(&entity.name);
    }
    
    // Parameters for functions (simplified)
    if config.display.show_parameters && entity.kind.is_callable() && !entity.parameters.is_empty() {
        label.push_str("(...)");
    }
    
    label
}

fn sanitize_id(id: &str) -> String {
    let mut result = String::new();
    for c in id.chars() {
        if c.is_alphanumeric() {
            result.push(c);
        } else {
            result.push('_');
        }
    }
    // Ensure it starts with a letter
    if result.chars().next().map(|c| c.is_numeric()).unwrap_or(true) {
        result.insert(0, 'n');
    }
    result
}

fn entity_shape(kind: EntityKind) -> impl Fn(&str) -> String {
    move |label: &str| {
        let escaped = label.replace('"', "'");
        match kind {
            EntityKind::Class | EntityKind::AbstractClass | EntityKind::Struct => format!("[\"{}\"]", escaped),
            EntityKind::Interface => format!("([\"{}\"]", escaped),
            EntityKind::Enum => format!("{{{{\"{}\"}}}}"  , escaped),
            EntityKind::Function | EntityKind::Method => format!("[/\"{}\"/]", escaped),
            EntityKind::Module => format!("[[\"{}\"]]", escaped),
            _ => format!("[\"{}\"]", escaped),
        }
    }
}

fn relationship_arrow(kind: RelationshipKind) -> &'static str {
    match kind {
        RelationshipKind::Inherits => "-.->",
        RelationshipKind::Implements => "-.->",
        RelationshipKind::Contains => "-->",
        RelationshipKind::Calls => "-->",
        RelationshipKind::Imports => "-->",
        RelationshipKind::DependsOn => "==>",
        _ => "-->",
    }
}
