//! The languages the index understands.

use std::path::Path;

use crate::symbols::SymbolKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Python,
    TypeScript,
    Tsx,
    JavaScript,
    Rust,
}

/// A syntax node that defines a symbol.
pub struct Definition {
    /// Tree-sitter node kind, for example `function_definition`.
    pub node_kind: &'static str,

    pub symbol_kind: SymbolKind,

    /// The field of the node that holds the symbol's name.
    pub name_field: &'static str,
}

const fn definition(
    node_kind: &'static str,
    symbol_kind: SymbolKind,
    name_field: &'static str,
) -> Definition {
    Definition {
        node_kind,
        symbol_kind,
        name_field,
    }
}

const PYTHON: &[Definition] = &[
    definition("function_definition", SymbolKind::Function, "name"),
    definition("class_definition", SymbolKind::Class, "name"),
];

const SCRIPT: &[Definition] = &[
    definition("function_declaration", SymbolKind::Function, "name"),
    definition(
        "generator_function_declaration",
        SymbolKind::Function,
        "name",
    ),
    definition("class_declaration", SymbolKind::Class, "name"),
    definition("abstract_class_declaration", SymbolKind::Class, "name"),
    definition("method_definition", SymbolKind::Method, "name"),
    definition("interface_declaration", SymbolKind::Interface, "name"),
    definition("type_alias_declaration", SymbolKind::Type, "name"),
    definition("enum_declaration", SymbolKind::Enum, "name"),
];

const RUST: &[Definition] = &[
    definition("function_item", SymbolKind::Function, "name"),
    definition("struct_item", SymbolKind::Struct, "name"),
    definition("enum_item", SymbolKind::Enum, "name"),
    definition("trait_item", SymbolKind::Trait, "name"),
    definition("impl_item", SymbolKind::Impl, "type"),
    definition("mod_item", SymbolKind::Module, "name"),
    definition("type_item", SymbolKind::Type, "name"),
    definition("const_item", SymbolKind::Constant, "name"),
    definition("static_item", SymbolKind::Constant, "name"),
];

impl Language {
    /// Chooses a language from a file's extension.
    pub fn for_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?;

        let language = match extension {
            "py" | "pyi" => Self::Python,
            "ts" | "mts" | "cts" => Self::TypeScript,
            "tsx" => Self::Tsx,
            "js" | "jsx" | "mjs" | "cjs" => Self::JavaScript,
            "rs" => Self::Rust,
            _ => return None,
        };

        Some(language)
    }

    pub fn grammar(self) -> tree_sitter::Language {
        match self {
            Self::Python => tree_sitter_python::LANGUAGE.into(),
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Self::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
        }
    }

    pub fn definitions(self) -> &'static [Definition] {
        match self {
            Self::Python => PYTHON,
            Self::TypeScript | Self::Tsx | Self::JavaScript => SCRIPT,
            Self::Rust => RUST,
        }
    }

    /// Whether `const name = ...` style declarations should be looked at.
    /// In JavaScript and TypeScript most functions are written that way.
    pub fn has_variable_declarations(self) -> bool {
        matches!(self, Self::TypeScript | Self::Tsx | Self::JavaScript)
    }
}
