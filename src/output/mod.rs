//! Output formatters for visualization.

mod ascii_renderer;
pub mod deps_report;
mod dot_renderer;
pub mod elevator_check;
pub mod elevator_code_map;
pub mod elevator_drift;
pub mod elevator_extract;
pub mod elevator_fix;
pub mod elevator_list;
pub(crate) mod elevator_text_renderer;
mod json_renderer;
mod mermaid_renderer;

pub use ascii_renderer::AsciiRenderer;
pub use dot_renderer::DotRenderer;
pub use elevator_text_renderer::ElevatorTextRenderer;
pub use json_renderer::JsonRenderer;
pub use mermaid_renderer::MermaidRenderer;

use crate::config::Config;
use crate::graph::DependencyGraph;
use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Output format for visualization.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    /// DOT format for Graphviz
    #[default]
    Dot,
    /// Mermaid diagram format
    Mermaid,
    /// JSON format
    Json,
    /// ASCII/text tree format
    Ascii,
    /// PlantUML format
    PlantUml,
    /// Compact text rendering of an Elevator (`.elv`) spec, intended
    /// for low-token LLM consumption. One node per line, hierarchy
    /// via indentation, edges in bracketed metadata. Honors
    /// `FilterConfig::root_entity` for subtree scoping.
    ElevatorText,
}

impl OutputFormat {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "dot" | "graphviz" => Some(OutputFormat::Dot),
            "mermaid" | "md" => Some(OutputFormat::Mermaid),
            "json" => Some(OutputFormat::Json),
            "ascii" | "text" | "tree" => Some(OutputFormat::Ascii),
            "plantuml" | "puml" => Some(OutputFormat::PlantUml),
            "elevator-text" | "elv-text" | "elv" => Some(OutputFormat::ElevatorText),
            _ => None,
        }
    }

    pub fn file_extension(&self) -> &'static str {
        match self {
            OutputFormat::Dot => "dot",
            OutputFormat::Mermaid => "md",
            OutputFormat::Json => "json",
            OutputFormat::Ascii => "txt",
            OutputFormat::PlantUml => "puml",
            OutputFormat::ElevatorText => "txt",
        }
    }
}

/// Trait for output renderers.
pub trait Renderer {
    /// Render the graph to a string
    fn render(&self, graph: &DependencyGraph, config: &Config) -> Result<String>;

    /// Get the output format this renderer produces
    fn format(&self) -> OutputFormat;
}

/// Get a renderer for the specified format.
pub fn get_renderer(format: OutputFormat) -> Box<dyn Renderer> {
    match format {
        OutputFormat::Dot => Box::new(DotRenderer),
        OutputFormat::Mermaid => Box::new(MermaidRenderer),
        OutputFormat::Json => Box::new(JsonRenderer),
        OutputFormat::Ascii => Box::new(AsciiRenderer),
        OutputFormat::PlantUml => Box::new(DotRenderer), // Fallback to DOT for now
        OutputFormat::ElevatorText => Box::new(ElevatorTextRenderer),
    }
}

/// Render a graph to a string using the specified format.
pub fn render(graph: &DependencyGraph, config: &Config) -> Result<String> {
    let renderer = get_renderer(config.output_format);
    renderer.render(graph, config)
}
