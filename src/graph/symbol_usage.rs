//! AST-based symbol usage extraction.

use std::collections::BTreeSet;
use tree_sitter::{Language, Node, Parser};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum UsageKind {
    Call,
    TypeUse,
    Import,
    Inherit,
    Ref,
}

impl UsageKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Call => "call",
            Self::TypeUse => "type_use",
            Self::Import => "import",
            Self::Inherit => "inherit",
            Self::Ref => "ref",
        }
    }
}

pub fn extract_symbol_usages(content: &str, language: &str) -> Vec<(String, UsageKind)> {
    let ts_language: Language = match language {
        "python" => tree_sitter_python::LANGUAGE.into(),
        "rust" => tree_sitter_rust::LANGUAGE.into(),
        "javascript" => tree_sitter_javascript::LANGUAGE.into(),
        "typescript" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        "go" => tree_sitter_go::LANGUAGE.into(),
        _ => return Vec::new(),
    };

    let mut parser = Parser::new();
    if parser.set_language(&ts_language).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(content, None) else {
        return Vec::new();
    };

    let mut out: BTreeSet<(String, UsageKind)> = BTreeSet::new();
    visit(tree.root_node(), content, language, &mut out);
    out.into_iter().collect()
}

fn visit(node: Node<'_>, content: &str, language: &str, out: &mut BTreeSet<(String, UsageKind)>) {
    match language {
        "rust" => collect_rust(node, content, out),
        "python" => collect_python(node, content, out),
        "javascript" | "typescript" => collect_js_ts(node, content, out),
        "go" => collect_go(node, content, out),
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        visit(child, content, language, out);
    }
}

fn collect_rust(node: Node<'_>, content: &str, out: &mut BTreeSet<(String, UsageKind)>) {
    match node.kind() {
        "use_declaration" => {
            for sym in identifier_descendants(node, content) {
                out.insert((sym, UsageKind::Import));
            }
        }
        "call_expression" => {
            if let Some(callee) = node.child_by_field_name("function") {
                if matches!(callee.kind(), "identifier" | "scoped_identifier") {
                    if let Some(sym) = symbol_text(callee, content) {
                        out.insert((sym, UsageKind::Call));
                    }
                }
            }
        }
        "type_identifier" => {
            if !is_definition_name_node(node) {
                if let Some(sym) = symbol_text(node, content) {
                    out.insert((sym, UsageKind::TypeUse));
                }
            }
        }
        _ => {}
    }
}

fn collect_python(node: Node<'_>, content: &str, out: &mut BTreeSet<(String, UsageKind)>) {
    match node.kind() {
        "call" => {
            if let Some(func) = node.child_by_field_name("function") {
                if let Some(sym) = symbol_text(func, content) {
                    out.insert((sym, UsageKind::Call));
                }
            }
        }
        "import_statement" | "import_from_statement" => {
            for sym in identifier_descendants(node, content) {
                out.insert((sym, UsageKind::Import));
            }
        }
        "class_definition" => {
            if let Some(superclasses) = node.child_by_field_name("superclasses") {
                for sym in identifier_descendants(superclasses, content) {
                    out.insert((sym, UsageKind::Inherit));
                }
            }
        }
        _ => {}
    }
}

fn collect_js_ts(node: Node<'_>, content: &str, out: &mut BTreeSet<(String, UsageKind)>) {
    match node.kind() {
        "call_expression" => {
            if let Some(func) = node.child_by_field_name("function") {
                if let Some(sym) = symbol_text(func, content) {
                    out.insert((sym, UsageKind::Call));
                }
            }
        }
        "import_statement" | "import_declaration" => {
            for sym in identifier_descendants(node, content) {
                out.insert((sym, UsageKind::Import));
            }
        }
        "extends_clause" => {
            for sym in identifier_descendants(node, content) {
                out.insert((sym, UsageKind::Inherit));
            }
        }
        "type_identifier" | "type_reference" => {
            if let Some(sym) = symbol_text(node, content) {
                out.insert((sym, UsageKind::TypeUse));
            }
        }
        _ => {}
    }
}

fn collect_go(node: Node<'_>, content: &str, out: &mut BTreeSet<(String, UsageKind)>) {
    match node.kind() {
        "call_expression" => {
            if let Some(func) = node.child_by_field_name("function") {
                if let Some(sym) = symbol_text(func, content) {
                    out.insert((sym, UsageKind::Call));
                }
            }
        }
        "import_declaration" => {
            for sym in identifier_descendants(node, content) {
                out.insert((sym, UsageKind::Import));
            }
        }
        _ => {}
    }
}

fn identifier_descendants(node: Node<'_>, content: &str) -> Vec<String> {
    let mut out = Vec::new();
    collect_identifier_descendants(node, content, &mut out);
    out
}

fn collect_identifier_descendants(node: Node<'_>, content: &str, out: &mut Vec<String>) {
    if node.kind().contains("identifier") {
        if let Some(sym) = symbol_text(node, content) {
            out.push(sym);
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_identifier_descendants(child, content, out);
    }
}

fn symbol_text(node: Node<'_>, content: &str) -> Option<String> {
    let text = node.utf8_text(content.as_bytes()).ok()?;
    normalize_symbol(text)
}

fn normalize_symbol(text: &str) -> Option<String> {
    let raw = text.trim();
    if raw.is_empty() {
        return None;
    }

    let last = raw
        .split([':', '.', '/', '\\', '<', '>', '(', ')', ',', ';'])
        .filter(|s| !s.is_empty())
        .next_back()
        .unwrap_or(raw);
    let cleaned: String =
        last.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned.to_ascii_lowercase())
    }
}

fn is_definition_name_node(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    if !matches!(
        parent.kind(),
        "function_item"
            | "struct_item"
            | "enum_item"
            | "trait_item"
            | "impl_item"
            | "mod_item"
            | "type_item"
    ) {
        return false;
    }
    if let Some(name_node) = parent.child_by_field_name("name") {
        return name_node.start_byte() == node.start_byte()
            && name_node.end_byte() == node.end_byte();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::{extract_symbol_usages, UsageKind};

    #[test]
    fn extracts_rust_usage_edges() {
        let src = "use crate::Foo;\nfn x() { do_work(); let y: Foo = Foo::new(); }\n";
        let uses = extract_symbol_usages(src, "rust");
        assert!(uses.contains(&("foo".to_string(), UsageKind::Import)));
        assert!(uses.contains(&("do_work".to_string(), UsageKind::Call)));
        assert!(uses.contains(&("foo".to_string(), UsageKind::TypeUse)));
    }

    #[test]
    fn extracts_python_usage_edges() {
        let src = "from util import helper\nclass C(Base):\n  pass\n\ndef run():\n  helper()\n";
        let uses = extract_symbol_usages(src, "python");
        assert!(uses.contains(&("helper".to_string(), UsageKind::Import)));
        assert!(uses.contains(&("helper".to_string(), UsageKind::Call)));
        assert!(uses.contains(&("base".to_string(), UsageKind::Inherit)));
    }
}
