//! DOT/Graphviz output renderer.

use super::{OutputFormat, Renderer};
use crate::config::Config;
use crate::graph::DependencyGraph;
use crate::models::{EntityKind, RelationshipKind};
use anyhow::Result;
use std::collections::HashMap;
use std::fmt::Write;

pub struct DotRenderer;

impl Renderer for DotRenderer {
    fn format(&self) -> OutputFormat {
        OutputFormat::Dot
    }
    
    fn render(&self, graph: &DependencyGraph, config: &Config) -> Result<String> {
        let mut output = String::new();
        
        // Graph header
        writeln!(output, "digraph CodeDependencies {{")?;
        writeln!(output, "  rankdir={};", config.display.layout_direction.as_dot_rankdir())?;
        writeln!(output, "  node [shape=box, style=rounded, fontname=\"Helvetica\"];")?;
        writeln!(output, "  edge [fontname=\"Helvetica\", fontsize=10];")?;
        writeln!(output)?;
        
        // Group entities by file if configured
        if config.display.group_by_file {
            let mut file_groups: HashMap<String, Vec<String>> = HashMap::new();
            
            for entity in graph.entities() {
                let file_key = entity.file_path.display().to_string();
                file_groups
                    .entry(file_key)
                    .or_default()
                    .push(entity.id.clone());
            }
            
            for (file_path, entity_ids) in &file_groups {
                let cluster_name = sanitize_id(&format!("cluster_{}", file_path));
                writeln!(output, "  subgraph {} {{", cluster_name)?;
                writeln!(output, "    label=\"{}\";", escape_label(file_path))?;
                writeln!(output, "    style=filled;")?;
                writeln!(output, "    color=lightgrey;")?;
                
                for entity_id in entity_ids {
                    if let Some(entity) = graph.get_entity(entity_id) {
                        let node_id = sanitize_id(entity_id);
                        let label = format_entity_label(entity, config);
                        let color = entity_color(entity.kind);
                        let shape = entity_shape(entity.kind);
                        
                        writeln!(
                            output,
                            "    {} [label=\"{}\", fillcolor=\"{}\", style=filled, shape={}];",
                            node_id,
                            escape_label(&label),
                            color,
                            shape
                        )?;
                    }
                }
                
                writeln!(output, "  }}")?;
                writeln!(output)?;
            }
        } else {
            // Render all nodes without grouping
            for entity in graph.entities() {
                let node_id = sanitize_id(&entity.id);
                let label = format_entity_label(entity, config);
                let color = entity_color(entity.kind);
                let shape = entity_shape(entity.kind);
                
                writeln!(
                    output,
                    "  {} [label=\"{}\", fillcolor=\"{}\", style=filled, shape={}];",
                    node_id,
                    escape_label(&label),
                    color,
                    shape
                )?;
            }
        }
        
        writeln!(output)?;
        writeln!(output, "  // Relationships")?;
        
        // Render edges
        for rel in graph.relationships() {
            let source_id = sanitize_id(&rel.source_id);
            let target_id = sanitize_id(&rel.target_id);
            let style = rel.kind.dot_line_style();
            let arrow = rel.kind.dot_arrow_style();
            let color = relationship_color(rel.kind);
            
            let label = rel.label.as_deref().unwrap_or(rel.kind.display_label());
            
            writeln!(
                output,
                "  {} -> {} [label=\"{}\", style={}, arrowhead={}, color=\"{}\"];",
                source_id,
                target_id,
                escape_label(label),
                style,
                arrow,
                color
            )?;
        }
        
        writeln!(output, "}}")?;
        
        Ok(output)
    }
}

fn format_entity_label(entity: &crate::models::CodeEntity, config: &Config) -> String {
    let mut label = String::new();
    
    // Entity kind symbol
    let symbol = match entity.kind {
        EntityKind::Class | EntityKind::AbstractClass => "⬜",
        EntityKind::Struct => "⬛",
        EntityKind::Interface => "◯",
        EntityKind::Enum => "◇",
        EntityKind::Function | EntityKind::Method => "▷",
        EntityKind::Module => "📦",
        EntityKind::File => "📄",
        _ => "•",
    };
    
    label.push_str(symbol);
    label.push(' ');
    
    // Name
    if config.display.show_qualified_names {
        label.push_str(&entity.qualified_name);
    } else {
        label.push_str(&entity.name);
    }
    
    // Parameters for functions
    if config.display.show_parameters && entity.kind.is_callable() {
        label.push('(');
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
        label.push_str(&params.join(", "));
        label.push(')');
    }
    
    // Return type
    if config.display.show_return_types {
        if let Some(ret) = &entity.return_type {
            label.push_str(" → ");
            label.push_str(ret);
        }
    }
    
    // Line number
    if config.display.show_line_numbers {
        label.push_str(&format!("\\n[L{}]", entity.span.start.line + 1));
    }
    
    label
}

fn sanitize_id(id: &str) -> String {
    let mut result = String::with_capacity(id.len());
    for c in id.chars() {
        if c.is_alphanumeric() || c == '_' {
            result.push(c);
        } else {
            result.push('_');
        }
    }
    result
}

fn escape_label(label: &str) -> String {
    label
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn entity_color(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::Class => "#E3F2FD",     // Light blue
        EntityKind::AbstractClass => "#BBDEFB", // Darker light blue (abstract)
        EntityKind::Struct => "#E8F5E9",    // Light green
        EntityKind::Interface => "#FFF3E0", // Light orange
        EntityKind::Enum => "#F3E5F5",      // Light purple
        EntityKind::Function => "#FFFDE7",  // Light yellow
        EntityKind::Method => "#FFFDE7",    // Light yellow
        EntityKind::Module => "#ECEFF1",    // Light grey
        EntityKind::File => "#FAFAFA",      // Very light grey
        EntityKind::Constant => "#FFEBEE",  // Light red
        _ => "#FFFFFF",                      // White
    }
}

fn entity_shape(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::Class | EntityKind::AbstractClass | EntityKind::Struct => "box",
        EntityKind::Interface => "ellipse",
        EntityKind::Enum => "diamond",
        EntityKind::Function | EntityKind::Method => "box",
        EntityKind::Module => "folder",
        EntityKind::File => "note",
        _ => "box",
    }
}

fn relationship_color(kind: RelationshipKind) -> &'static str {
    match kind {
        RelationshipKind::Inherits => "#2196F3",       // Blue
        RelationshipKind::Implements => "#4CAF50",     // Green
        RelationshipKind::Imports => "#9E9E9E",        // Grey
        RelationshipKind::Calls => "#FF9800",          // Orange
        RelationshipKind::Contains => "#607D8B",       // Blue grey
        RelationshipKind::DependsOn => "#F44336",      // Red
        _ => "#000000",                                 // Black
    }
}
