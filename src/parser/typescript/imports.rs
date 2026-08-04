//! TypeScript `import` statement parsing — handles default imports,
//! named imports, and namespace (star) imports.

use super::super::language_parser::{node_text, node_to_span, ImportInfo};
use tree_sitter::Node;

pub(super) fn parse_import(node: &Node, source: &str) -> Option<ImportInfo> {
    let source_node = node.child_by_field_name("source")?;
    let raw_path = node_text(&source_node, source)
        .trim_matches('\'')
        .trim_matches('"')
        .to_string();
    let span = node_to_span(node);

    let is_relative = raw_path.starts_with('.') || raw_path.starts_with('/');
    let mut import = ImportInfo::new(&raw_path, span);
    if is_relative {
        import = import.relative();
    }

    let mut items = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "import_clause" {
            let mut ic = child.walk();
            for ic_child in child.children(&mut ic) {
                match ic_child.kind() {
                    "identifier" => {
                        // Default import
                        items.push(node_text(&ic_child, source).to_string());
                    }
                    "named_imports" => {
                        let mut ni = ic_child.walk();
                        for spec in ic_child.children(&mut ni) {
                            if spec.kind() == "import_specifier" {
                                if let Some(name_node) = spec.child_by_field_name("name") {
                                    items.push(node_text(&name_node, source).to_string());
                                }
                            }
                        }
                    }
                    "namespace_import" => {
                        // import * as Foo
                        let mut ns = ic_child.walk();
                        for ns_child in ic_child.children(&mut ns) {
                            if ns_child.kind() == "identifier" {
                                import = import.with_alias(
                                    node_text(&ns_child, source).to_string(),
                                );
                                break;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    if !items.is_empty() {
        import = import.with_items(items);
    }

    Some(import)
}
