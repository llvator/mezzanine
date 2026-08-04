//! Generic parser for languages without specialized implementations.
//! Uses a simple regex-based approach for basic entity detection.

use crate::models::{
    CodeEntity, EntityKind, Visibility, Span,
};
use crate::models::file_info::Language;
use super::language_parser::{LanguageParser, ParseResult, ImportInfo};
use anyhow::Result;
use std::path::Path;

/// Generic parser that provides basic functionality for unsupported languages.
pub struct GenericParser {
    language: Language,
}

impl GenericParser {
    pub fn new(language: Language) -> Self {
        Self { language }
    }
    
    /// Simple regex-based extraction (fallback when no tree-sitter parser is available)
    fn extract_with_patterns(&self, content: &str, path: &Path, result: &mut ParseResult) {
        // Track line numbers
        let mut line_number = 0;
        
        for line in content.lines() {
            let trimmed = line.trim();
            
            // Skip empty lines and comments
            if trimmed.is_empty() || self.is_comment(trimmed) {
                line_number += 1;
                continue;
            }
            
            // Try to extract entities based on language
            if let Some(mut entity) = self.try_extract_entity(trimmed, line_number, path) {
                entity.source_code = Some(trimmed.to_string());
                result.add_entity(entity);
            }
            
            // Try to extract imports
            if let Some(import) = self.try_extract_import(trimmed, line_number) {
                result.add_import(import);
            }
            
            line_number += 1;
        }
    }
    
    fn is_comment(&self, line: &str) -> bool {
        match self.language {
            Language::Python | Language::Ruby => line.starts_with('#'),
            Language::JavaScript | Language::TypeScript | Language::Java 
            | Language::Go | Language::CSharp | Language::Rust 
            | Language::Cpp | Language::C | Language::Swift | Language::Kotlin => {
                line.starts_with("//") || line.starts_with("/*") || line.starts_with('*')
            }
            Language::PHP => {
                line.starts_with("//") || line.starts_with('#') || 
                line.starts_with("/*") || line.starts_with('*')
            }
            _ => false,
        }
    }
    
    fn try_extract_entity(&self, line: &str, line_num: usize, path: &Path) -> Option<CodeEntity> {
        let span = Span::from_positions(line_num, 0, line_num, line.len());
        
        match self.language {
            Language::JavaScript | Language::TypeScript => {
                self.extract_js_entity(line, path, span)
            }
            Language::Java => self.extract_java_entity(line, path, span),
            Language::Go => self.extract_go_entity(line, path, span),
            _ => None,
        }
    }
    
    fn extract_js_entity(&self, line: &str, path: &Path, span: Span) -> Option<CodeEntity> {
        // Function patterns
        if line.contains("function ") {
            let name = self.extract_name_after(line, "function ")?;
            return Some(CodeEntity::new(name, EntityKind::Function, path, span));
        }
        
        // Arrow functions with const/let/var
        if let Some(rest) = line.strip_prefix("const ").or_else(|| line.strip_prefix("let ")).or_else(|| line.strip_prefix("var ")) {
            if rest.contains("=>") || rest.contains("= function") {
                let name = rest.split('=').next()?.trim();
                if !name.is_empty() {
                    return Some(CodeEntity::new(name, EntityKind::Function, path, span));
                }
            }
        }
        
        // Class definition
        if line.contains("class ") {
            let name = self.extract_name_after(line, "class ")?;
            return Some(CodeEntity::new(name, EntityKind::Class, path, span)
                .with_visibility(Visibility::Public));
        }
        
        // Interface (TypeScript)
        if line.contains("interface ") {
            let name = self.extract_name_after(line, "interface ")?;
            return Some(CodeEntity::new(name, EntityKind::Interface, path, span));
        }
        
        // Type alias (TypeScript)
        if line.contains("type ") && line.contains('=') {
            let name = self.extract_name_after(line, "type ")?;
            return Some(CodeEntity::new(name, EntityKind::TypeAlias, path, span));
        }
        
        None
    }
    
    fn extract_java_entity(&self, line: &str, path: &Path, span: Span) -> Option<CodeEntity> {
        let visibility = if line.contains("public ") {
            Visibility::Public
        } else if line.contains("private ") {
            Visibility::Private
        } else if line.contains("protected ") {
            Visibility::Protected
        } else {
            Visibility::Internal
        };
        
        // Class (check abstract before regular class)
        if line.contains(" class ") || line.starts_with("class ") {
            let name = self.extract_name_after(line, "class ")?;
            let kind = if line.contains("abstract") { EntityKind::AbstractClass } else { EntityKind::Class };
            return Some(CodeEntity::new(name, kind, path, span)
                .with_visibility(visibility));
        }
        
        // Interface
        if line.contains(" interface ") || line.starts_with("interface ") {
            let name = self.extract_name_after(line, "interface ")?;
            return Some(CodeEntity::new(name, EntityKind::Interface, path, span)
                .with_visibility(visibility));
        }
        
        // Enum
        if line.contains(" enum ") || line.starts_with("enum ") {
            let name = self.extract_name_after(line, "enum ")?;
            return Some(CodeEntity::new(name, EntityKind::Enum, path, span)
                .with_visibility(visibility));
        }
        
        None
    }
    
    fn extract_go_entity(&self, line: &str, path: &Path, span: Span) -> Option<CodeEntity> {
        // Function
        if line.starts_with("func ") {
            let rest = &line[5..];
            let name = if rest.starts_with('(') {
                // Method: func (r *Receiver) Name()
                rest.split(')').nth(1)?
                    .trim()
                    .split('(')
                    .next()?
                    .trim()
            } else {
                // Regular function: func Name()
                rest.split('(').next()?.trim()
            };
            if !name.is_empty() {
                let visibility = if name.chars().next()?.is_uppercase() {
                    Visibility::Public
                } else {
                    Visibility::Private
                };
                return Some(CodeEntity::new(name, EntityKind::Function, path, span)
                    .with_visibility(visibility));
            }
        }
        
        // Struct
        if line.starts_with("type ") && line.contains(" struct") {
            let name = self.extract_name_after(line, "type ")?;
            let visibility = if name.chars().next()?.is_uppercase() {
                Visibility::Public
            } else {
                Visibility::Private
            };
            return Some(CodeEntity::new(name, EntityKind::Struct, path, span)
                .with_visibility(visibility));
        }
        
        // Interface
        if line.starts_with("type ") && line.contains(" interface") {
            let name = self.extract_name_after(line, "type ")?;
            let visibility = if name.chars().next()?.is_uppercase() {
                Visibility::Public
            } else {
                Visibility::Private
            };
            return Some(CodeEntity::new(name, EntityKind::Interface, path, span)
                .with_visibility(visibility));
        }
        
        None
    }
    
    fn try_extract_import(&self, line: &str, line_num: usize) -> Option<ImportInfo> {
        let span = Span::from_positions(line_num, 0, line_num, line.len());
        
        match self.language {
            Language::JavaScript | Language::TypeScript => {
                if line.starts_with("import ") {
                    // Extract module path from import statement
                    let path = line.rsplit("from").next()?
                        .trim()
                        .trim_matches(|c| c == '\'' || c == '"' || c == ';');
                    return Some(ImportInfo::new(path, span));
                }
                if line.starts_with("require(") || line.contains("= require(") {
                    let path = line.split("require(").nth(1)?
                        .split(')').next()?
                        .trim_matches(|c| c == '\'' || c == '"');
                    return Some(ImportInfo::new(path, span));
                }
            }
            Language::Java => {
                if line.starts_with("import ") {
                    let path = line
                        .trim_start_matches("import ")
                        .trim_start_matches("static ")
                        .trim_end_matches(';')
                        .trim();
                    return Some(ImportInfo::new(path, span));
                }
            }
            Language::Go => {
                if line.starts_with("import ") {
                    let path = line
                        .trim_start_matches("import ")
                        .trim_matches('"');
                    return Some(ImportInfo::new(path, span));
                }
            }
            Language::CSharp => {
                if line.starts_with("using ") && !line.contains('(') {
                    let path = line
                        .trim_start_matches("using ")
                        .trim_end_matches(';')
                        .trim();
                    return Some(ImportInfo::new(path, span));
                }
            }
            _ => {}
        }
        
        None
    }
    
    fn extract_name_after<'a>(&self, line: &'a str, pattern: &str) -> Option<&'a str> {
        let rest = line.split(pattern).nth(1)?;
        let name = rest
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .next()?
            .trim();
        if name.is_empty() {
            None
        } else {
            Some(name)
        }
    }
}

impl LanguageParser for GenericParser {
    fn language(&self) -> Language {
        self.language
    }
    
    fn parse(&self, path: &Path, content: &str) -> Result<ParseResult> {
        let mut result = ParseResult::new();
        self.extract_with_patterns(content, path, &mut result);
        
        // Add a warning that this is using generic parsing
        if result.entities.is_empty() {
            result.add_warning(format!(
                "Using generic parser for {:?} - results may be incomplete",
                self.language
            ));
        }
        
        Ok(result)
    }
}
