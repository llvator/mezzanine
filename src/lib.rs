//! # Mezzanine
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

pub mod analyzer;
pub mod check;
pub mod config;
pub mod diff;
pub mod educator;
pub mod graph;
pub mod init;
pub mod mcp;
pub mod models;
pub mod output;
pub mod parser;
pub mod server;
pub mod settings;

pub use analyzer::Analyzer;
pub use config::Config;
pub use graph::DependencyGraph;
pub use models::*;

/// Re-export commonly used types
pub mod prelude {
    pub use crate::analyzer::Analyzer;
    pub use crate::config::Config;
    pub use crate::graph::DependencyGraph;
    pub use crate::models::{
        CodeEntity, EntityKind, FileInfo, Position, Relationship, RelationshipKind, Span,
    };
    pub use crate::output::{OutputFormat, Renderer};
}
