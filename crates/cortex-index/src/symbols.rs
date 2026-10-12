//! Extracting the symbols of one source file.

use tree_sitter::{Node, Parser};

use crate::language::Language;

const MAX_SIGNATURE_CHARS: usize = 140;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Interface,
    Struct,
    Enum,
    Trait,
    Impl,
    Module,
    Type,
    Constant,
}

impl SymbolKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Method => "method",
            Self::Class => "class",
            Self::Interface => "interface",
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Trait => "trait",
            Self::Impl => "impl",
            Self::Module => "module",
            Self::Type => "type",
            Self::Constant => "constant",
        }
    }

    /// Whether symbols defined inside this one belong to it, the way
    /// methods belong to a class.
    fn is_container(self) -> bool {
        matches!(
            self,
            Self::Class | Self::Interface | Self::Trait | Self::Impl | Self::Module
        )
    }

    fn is_callable(self) -> bool {
        matches!(self, Self::Function | Self::Method)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,

    /// The class, trait, impl or module this symbol is defined in.
    pub container: Option<String>,

    /// First and last line, counted from 1.
    pub start_line: usize,
    pub end_line: usize,

    /// The first line of the definition, for display.
    pub signature: String,

    /// How many containers enclose this symbol.
    pub depth: usize,
}

/// Parses `source` and returns its symbols in the order they appear.
/// A file that cannot be parsed yields no symbols.
pub fn extract_symbols(source: &str, language: Language) -> Vec<Symbol> {
    let mut parser = Parser::new();

    if parser.set_language(&language.grammar()).is_err() {
        return Vec::new();
    }

    let Some(tree) = parser.parse(source, None) else {
        return Vec::new();
    };

    let mut collector = Collector {
        source,
        language,
        symbols: Vec::new(),
    };

    collector.visit(tree.root_node(), &Scope::default());

    collector.symbols
}

/// Where the walk currently is.
#[derive(Default, Clone)]
struct Scope {
    container: Option<String>,
    depth: usize,

    /// Inside a function body, where local variables are not symbols.
    inside_callable: bool,
}

struct Collector<'a> {
    source: &'a str,
    language: Language,
    symbols: Vec<Symbol>,
}

impl Collector<'_> {
    fn visit(&mut self, node: Node, scope: &Scope) {
        let child_scope = match self.symbol_at(node, scope) {
            Some(symbol) => {
                let child_scope = scope_inside(&symbol, scope);

                self.symbols.push(symbol);

                child_scope
            }

            None => scope.clone(),
        };

        let mut cursor = node.walk();

        for child in node.children(&mut cursor) {
            self.visit(child, &child_scope);
        }
    }

    fn symbol_at(&self, node: Node, scope: &Scope) -> Option<Symbol> {
        if let Some(symbol) = self.declared_symbol(node, scope) {
            return Some(symbol);
        }

        if self.language.has_variable_declarations() && !scope.inside_callable {
            return self.variable_symbol(node, scope);
        }

        None
    }

    /// A node whose kind is one of the language's definition kinds.
    fn declared_symbol(&self, node: Node, scope: &Scope) -> Option<Symbol> {
        let definition = self
            .language
            .definitions()
            .iter()
            .find(|definition| definition.node_kind == node.kind())?;

        let name = self.text(node.child_by_field_name(definition.name_field)?);

        // A Python function defined inside a class is a method.
        let kind = match (definition.symbol_kind, &scope.container) {
            (SymbolKind::Function, Some(_)) if !scope.inside_callable => SymbolKind::Method,
            (kind, _) => kind,
        };

        // Include a leading `export` in the range and the signature.
        let statement = match node.parent() {
            Some(parent) if parent.kind() == "export_statement" => parent,
            _ => node,
        };

        Some(self.symbol(statement, name, kind, scope))
    }

    /// `const name = ...` in JavaScript and TypeScript.
    fn variable_symbol(&self, node: Node, scope: &Scope) -> Option<Symbol> {
        if node.kind() != "variable_declarator" {
            return None;
        }

        let name_node = node.child_by_field_name("name")?;

        // Destructuring patterns have no single name.
        if name_node.kind() != "identifier" {
            return None;
        }

        let is_function = node.child_by_field_name("value").is_some_and(|value| {
            matches!(
                value.kind(),
                "arrow_function" | "function_expression" | "function"
            )
        });

        let kind = if is_function {
            SymbolKind::Function
        } else {
            SymbolKind::Constant
        };

        // Report the whole statement (`export const x = ...`), not just the
        // declarator, so its line range covers the full definition.
        let statement = enclosing_statement(node);

        Some(self.symbol(statement, self.text(name_node), kind, scope))
    }

    fn symbol(&self, node: Node, name: String, kind: SymbolKind, scope: &Scope) -> Symbol {
        Symbol {
            name,
            kind,
            container: scope.container.clone(),
            start_line: node.start_position().row + 1,
            end_line: node.end_position().row + 1,
            signature: self.signature(node),
            depth: scope.depth,
        }
    }

    fn text(&self, node: Node) -> String {
        self.source[node.byte_range()].to_string()
    }

    fn signature(&self, node: Node) -> String {
        let text = &self.source[node.byte_range()];

        let first_line = text.lines().next().unwrap_or_default().trim();

        let cleaned = first_line.trim_end_matches(['{', ':', ' ']);

        if cleaned.chars().count() <= MAX_SIGNATURE_CHARS {
            return cleaned.to_string();
        }

        let clipped: String = cleaned.chars().take(MAX_SIGNATURE_CHARS).collect();

        format!("{clipped}...")
    }
}

fn scope_inside(symbol: &Symbol, scope: &Scope) -> Scope {
    if symbol.kind.is_container() {
        return Scope {
            container: Some(symbol.name.clone()),
            depth: scope.depth + 1,
            inside_callable: false,
        };
    }

    Scope {
        container: scope.container.clone(),
        depth: scope.depth,
        inside_callable: scope.inside_callable || symbol.kind.is_callable(),
    }
}

/// Climbs from a declarator to the statement that contains it, including a
/// surrounding `export`.
fn enclosing_statement(node: Node) -> Node {
    let mut current = node;

    while let Some(parent) = current.parent() {
        let is_wrapper = matches!(
            parent.kind(),
            "lexical_declaration" | "variable_declaration" | "export_statement"
        );

        if !is_wrapper {
            break;
        }

        current = parent;
    }

    current
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(symbols: &[Symbol]) -> Vec<(&str, SymbolKind)> {
        symbols
            .iter()
            .map(|symbol| (symbol.name.as_str(), symbol.kind))
            .collect()
    }

    #[test]
    fn python_functions_classes_and_methods() {
        let source = "\
import os

def top(a, b):
    def helper():
        pass
    return a

class Cart:
    def total(self):
        return 0
";

        let symbols = extract_symbols(source, Language::Python);

        assert_eq!(
            names(&symbols),
            vec![
                ("top", SymbolKind::Function),
                ("helper", SymbolKind::Function),
                ("Cart", SymbolKind::Class),
                ("total", SymbolKind::Method),
            ]
        );

        let total = &symbols[3];

        assert_eq!(total.container.as_deref(), Some("Cart"));
        assert_eq!((total.start_line, total.end_line), (9, 10));
        assert_eq!(total.signature, "def total(self)");
    }

    #[test]
    fn typescript_declarations_and_arrow_functions() {
        let source = "\
export type Fill = { id: string };

const LIMIT = 100;

export function matchFills(fills: Fill[]) {
  const local = 1;
  return fills;
}

export const toCents = (value: string): bigint => {
  return BigInt(value);
};

export class Database {
  async save(entry: Fill): Promise<void> {}
}
";

        let symbols = extract_symbols(source, Language::TypeScript);

        assert_eq!(
            names(&symbols),
            vec![
                ("Fill", SymbolKind::Type),
                ("LIMIT", SymbolKind::Constant),
                ("matchFills", SymbolKind::Function),
                ("toCents", SymbolKind::Function),
                ("Database", SymbolKind::Class),
                ("save", SymbolKind::Method),
            ]
        );

        // The range of an exported arrow function covers the whole statement.
        let to_cents = &symbols[3];

        assert_eq!((to_cents.start_line, to_cents.end_line), (10, 12));
    }

    #[test]
    fn rust_items_and_impl_methods() {
        let source = "\
pub struct Agent;

impl Agent {
    pub fn run(&self) {}
}

pub trait Tool {
    fn spec(&self);
}
";

        let symbols = extract_symbols(source, Language::Rust);

        assert_eq!(
            names(&symbols),
            vec![
                ("Agent", SymbolKind::Struct),
                ("Agent", SymbolKind::Impl),
                ("run", SymbolKind::Method),
                ("Tool", SymbolKind::Trait),
            ]
        );
    }
}
