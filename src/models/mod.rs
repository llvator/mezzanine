//! Core data models for representing code entities and relationships.

pub mod entity;
pub mod file_info;
pub mod folder_picture;
mod relationship;
pub mod scope_metrics;
pub mod thresholds;

pub use entity::{CodeEntity, EntityKind, EntityMetrics, Parameter, SmellKind, Visibility};
pub use file_info::{FileInfo, Position, Span};
pub use folder_picture::{
    ChildKind, EdgeVerdict, ErasedEdge, FolderPicture, OutsideEdge, OutsideVerdict, PictureChild,
    PictureEdge,
};
pub use relationship::{ImportSite, Precision, Relationship, RelationshipKind};
pub use scope_metrics::{
    FileMetrics, FolderMetrics, FolderShape, ScopeMetrics, ShapeBlocker, ShapePattern,
    ShapeTerms, WIRING_FILES,
};
pub use thresholds::Thresholds;
