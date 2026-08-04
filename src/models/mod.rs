//! Core data models for representing code entities and relationships.

pub mod entity;
mod relationship;
pub mod file_info;
pub mod scope_metrics;
pub mod thresholds;

pub use entity::{CodeEntity, EntityKind, EntityMetrics, Parameter, SmellKind, Visibility};
pub use relationship::{Precision, Relationship, RelationshipKind};
pub use file_info::{FileInfo, Position, Span};
pub use scope_metrics::{FileMetrics, ModuleMetrics, ScopeMetrics};
pub use thresholds::Thresholds;
