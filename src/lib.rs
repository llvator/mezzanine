//! # Nao
//!
//! A tool for visualizing code relationships and dependencies at multiple levels:
//! - **File level**: Classes, functions, interfaces within a file
//! - **Class level**: External dependencies a class uses
//! - **Module level**: Inter-file dependencies
//! - **Service level**: Cross-service relationships
//!
//! ## Features
//! - Multi-language support (Rust, Python, JavaScript/TypeScript, Java, Go)
//! - Multiple output formats (DOT/Graphviz, Mermaid, JSON, ASCII)
//! - Configurable depth for dependency traversal
//! - Filter by entity type (classes, functions, interfaces)

pub mod models;
pub mod parser;
pub mod analyzer;
pub mod graph;
pub mod output;
pub mod config;
pub mod settings;
pub mod diff;
pub mod server;
pub mod educator;
pub mod mcp;

pub use models::*;
pub use analyzer::Analyzer;
pub use graph::DependencyGraph;
pub use config::Config;

/// Re-export commonly used types
pub mod prelude {
    pub use crate::models::{
        CodeEntity, EntityKind, Relationship, RelationshipKind,
        FileInfo, Position, Span,
    };
    pub use crate::analyzer::Analyzer;
    pub use crate::graph::DependencyGraph;
    pub use crate::output::{OutputFormat, Renderer};
    pub use crate::config::Config;
}
