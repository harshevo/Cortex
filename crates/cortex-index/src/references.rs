//! Extracting what one source file refers to: its imports and every place a
//! name is used.
//!
//! [`crate::symbols`] answers "what does this file define?". This module
//! answers the opposite question, "what does this file use?", which is what
//! the code graph is built from.

use std::collections::HashSet;

use tree_sitter::{Node, Parser};

use crate::language::Language;

/// One name brought into a file by an import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedName {
    /// The name in the module it comes from. `*` for a whole-module import
    /// and `default` for a JavaScript default import.
    pub name: String,

    /// The name it has in the importing file.
    pub local: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    /// The module as written, for example `./money`, `@/lib/money` or
    /// `app.models`.
    pub source: String,

    pub names: Vec<ImportedName>,

    /// Line, counted from 1.
    pub line: usize,

    /// `export ... from "x"`: the names pass through this file to its own
    /// importers.
    pub is_reexport: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UseKind {
    /// The name is called, constructed, or rendered as a component.
    Call,

    /// The name is mentioned without being called: passed as a value, used
    /// as a type, read as a property.
    Mention,

    /// The name appears in an import or re-export.
    Import,
}

/// One appearance of a name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameUse {
    pub name: String,
    pub kind: UseKind,

    /// Line, counted from 1.
    pub line: usize,

    /// For `receiver.name`, the receiver when it is a plain identifier.
    pub receiver: Option<String>,

    /// The name is reached through something (`a.name`, `a::name`), so it
    /// may be a method or field rather than a top-level symbol.
    pub is_member: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileReferences {
    pub imports: Vec<Import>,
    pub uses: Vec<NameUse>,
}

/// Parses `source` and returns its imports and name uses. A file that
/// cannot be parsed yields none.
pub fn extract_references(source: &str, language: Language) -> FileReferences {
    let mut parser = Parser::new();

    if parser.set_language(&language.grammar()).is_err() {
        return FileReferences::default();
    }

    let Some(tree) = parser.parse(source, None) else {
        return FileReferences::default();
    };

    let mut collector = Collector {
        source,
        language,
        references: FileReferences::default(),
        handled: HashSet::new(),
    };

    collector.visit(tree.root_node());

    collector.references
}

struct Collector<'a> {
    source: &'a str,
    language: Language,
    references: FileReferences,

    /// Name nodes already recorded by a parent (a callee, an import), so
    /// the generic identifier rule does not record them again.
    handled: HashSet<usize>,
}

impl Collector<'_> {
    fn visit(&mut self, node: Node) {
        match self.language {
            Language::Python => self.visit_python(node),

            Language::TypeScript | Language::Tsx | Language::JavaScript => self.visit_script(node),

            Language::Rust => self.visit_rust(node),
        }

        self.record_mention(node);

        let mut cursor = node.walk();

        for child in node.children(&mut cursor) {
            self.visit(child);
        }
    }

    // ---- JavaScript and TypeScript -------------------------------------

    fn visit_script(&mut self, node: Node) {
        match node.kind() {
            "import_statement" => self.script_import(node, false),

            "export_statement" if node.child_by_field_name("source").is_some() => {
                self.script_import(node, true)
            }

            "call_expression" => {
                if !self.script_dynamic_import(node) {
                    self.record_callee(node.child_by_field_name("function"));
                }
            }

            "new_expression" => self.record_callee(node.child_by_field_name("constructor")),

            "jsx_opening_element" | "jsx_self_closing_element" => {
                self.record_callee(node.child_by_field_name("name"))
            }

            _ => {}
        }
    }

    fn script_import(&mut self, node: Node, is_reexport: bool) {
        let Some(source) = node.child_by_field_name("source") else {
            return;
        };

        let mut names = Vec::new();

        self.script_import_names(node, &mut names);

        // `import "./side-effect"` and `export * from "./x"` name nothing.
        if names.is_empty() {
            names.push(ImportedName {
                name: "*".to_string(),
                local: "*".to_string(),
            });
        }

        self.references.imports.push(Import {
            source: unquote(&self.text(source)),
            names,
            line: line_of(node),
            is_reexport,
        });
    }

    fn script_import_names(&mut self, node: Node, names: &mut Vec<ImportedName>) {
        let mut cursor = node.walk();

        for child in node.children(&mut cursor) {
            match child.kind() {
                // `import React from "react"`
                "identifier" => {
                    self.record_import_name(child);

                    names.push(ImportedName {
                        name: "default".to_string(),
                        local: self.text(child),
                    });
                }

                // `import * as money from "./money"`
                "namespace_import" | "namespace_export" => {
                    let local = last_named_child(child)
                        .map(|name| {
                            self.record_import_name(name);

                            self.text(name)
                        })
                        .unwrap_or_else(|| "*".to_string());

                    names.push(ImportedName {
                        name: "*".to_string(),
                        local,
                    });
                }

                // `{ toCents, format as fmt }`
                "import_specifier" | "export_specifier" => {
                    let Some(name) = child.child_by_field_name("name") else {
                        continue;
                    };

                    let alias = child.child_by_field_name("alias");

                    self.record_import_name(name);

                    if let Some(alias) = alias {
                        self.record_import_name(alias);
                    }

                    names.push(ImportedName {
                        name: self.text(name),
                        local: self.text(alias.unwrap_or(name)),
                    });
                }

                "string" => {}

                _ => self.script_import_names(child, names),
            }
        }
    }

    /// `require("x")` and `import("x")`. Returns whether `node` was one.
    fn script_dynamic_import(&mut self, node: Node) -> bool {
        let Some(function) = node.child_by_field_name("function") else {
            return false;
        };

        let is_loader = matches!(function.kind(), "import")
            || (function.kind() == "identifier" && self.text(function) == "require");

        if !is_loader {
            return false;
        }

        let argument = node
            .child_by_field_name("arguments")
            .and_then(|arguments| arguments.named_child(0));

        let Some(argument) = argument.filter(|argument| argument.kind() == "string") else {
            return false;
        };

        self.references.imports.push(Import {
            source: unquote(&self.text(argument)),
            names: vec![ImportedName {
                name: "*".to_string(),
                local: "*".to_string(),
            }],
            line: line_of(node),
            is_reexport: false,
        });

        true
    }

    // ---- Python ----------------------------------------------------------

    fn visit_python(&mut self, node: Node) {
        match node.kind() {
            "import_statement" => self.python_import(node),

            "import_from_statement" => self.python_from_import(node),

            "call" => self.record_callee(node.child_by_field_name("function")),

            _ => {}
        }
    }

    /// `import a.b` or `import a.b as c`
    fn python_import(&mut self, node: Node) {
        let mut cursor = node.walk();

        for child in node.named_children(&mut cursor) {
            let (module, alias) = match child.kind() {
                "dotted_name" => (child, None),

                "aliased_import" => {
                    let Some(module) = child.child_by_field_name("name") else {
                        continue;
                    };

                    (module, child.child_by_field_name("alias"))
                }

                _ => continue,
            };

            let source = self.text(module);

            // `import a.b` binds `a`; calls are then written `a.b.f()`.
            let local = match alias {
                Some(alias) => self.text(alias),
                None => source.split('.').next().unwrap_or_default().to_string(),
            };

            self.mark_handled(child);

            self.references.imports.push(Import {
                source,
                names: vec![ImportedName {
                    name: "*".to_string(),
                    local,
                }],
                line: line_of(node),
                is_reexport: false,
            });
        }
    }

    /// `from a.b import c, d as e`
    fn python_from_import(&mut self, node: Node) {
        let Some(module) = node.child_by_field_name("module_name") else {
            return;
        };

        self.mark_handled(module);

        let mut names = Vec::new();
        let mut cursor = node.walk();

        for child in node.children_by_field_name("name", &mut cursor) {
            let (name, alias) = match child.kind() {
                "aliased_import" => (
                    child.child_by_field_name("name"),
                    child.child_by_field_name("alias"),
                ),

                _ => (Some(child), None),
            };

            let Some(name) = name else {
                continue;
            };

            self.record_import_name(name);

            if let Some(alias) = alias {
                self.record_import_name(alias);
            }

            names.push(ImportedName {
                name: self.text(name),
                local: self.text(alias.unwrap_or(name)),
            });
        }

        // `from a import *`
        if names.is_empty() {
            names.push(ImportedName {
                name: "*".to_string(),
                local: "*".to_string(),
            });
        }

        self.references.imports.push(Import {
            source: self.text(module),
            names,
            line: line_of(node),
            is_reexport: false,
        });
    }

    // ---- Rust ------------------------------------------------------------

    fn visit_rust(&mut self, node: Node) {
        match node.kind() {
            "use_declaration" => self.rust_use(node),

            "call_expression" => self.record_callee(node.child_by_field_name("function")),

            _ => {}
        }
    }

    /// `use crate::a::{b, c as d};`
    fn rust_use(&mut self, node: Node) {
        let Some(argument) = node.child_by_field_name("argument") else {
            return;
        };

        let mut leaves = Vec::new();

        self.rust_use_leaves(argument, String::new(), &mut leaves);

        for (path, name, local) in leaves {
            self.references.imports.push(Import {
                source: path,
                names: vec![ImportedName { name, local }],
                line: line_of(node),
                is_reexport: false,
            });
        }

        self.mark_handled(argument);
    }

    /// Flattens a use tree into `(module path, name, local name)` entries.
    fn rust_use_leaves(
        &self,
        node: Node,
        prefix: String,
        leaves: &mut Vec<(String, String, String)>,
    ) {
        let join = |prefix: &str, rest: &str| {
            if prefix.is_empty() {
                rest.to_string()
            } else {
                format!("{prefix}::{rest}")
            }
        };

        match node.kind() {
            "scoped_use_list" => {
                let path = node
                    .child_by_field_name("path")
                    .map(|path| self.text(path))
                    .unwrap_or_default();

                if let Some(list) = node.child_by_field_name("list") {
                    self.rust_use_leaves(list, join(&prefix, &path), leaves);
                }
            }

            "use_list" => {
                let mut cursor = node.walk();

                for child in node.named_children(&mut cursor) {
                    self.rust_use_leaves(child, prefix.clone(), leaves);
                }
            }

            "use_as_clause" => {
                let path = node
                    .child_by_field_name("path")
                    .map(|path| self.text(path))
                    .unwrap_or_default();

                let alias = node
                    .child_by_field_name("alias")
                    .map(|alias| self.text(alias))
                    .unwrap_or_default();

                let (module, name) = split_rust_path(&join(&prefix, &path));

                leaves.push((module, name, alias));
            }

            "use_wildcard" => {
                let path = node
                    .named_child(0)
                    .map(|path| self.text(path))
                    .unwrap_or_default();

                leaves.push((join(&prefix, &path), "*".to_string(), "*".to_string()));
            }

            // `self` inside a list refers to the module itself.
            "self" => {
                let (module, name) = split_rust_path(&prefix);

                leaves.push((module, name.clone(), name));
            }

            _ => {
                let (module, name) = split_rust_path(&join(&prefix, &self.text(node)));

                leaves.push((module, name.clone(), name));
            }
        }
    }

    // ---- Shared ----------------------------------------------------------

    /// Records the name being called in `callee`, which may be a plain
    /// name, a member access, or a path.
    fn record_callee(&mut self, callee: Option<Node>) {
        let Some(callee) = callee else {
            return;
        };

        let (name_node, receiver, is_member) = match callee.kind() {
            "identifier" => (callee, None, false),

            // `a.b()` in JavaScript, `a.b()` in Python, `a.b()` in Rust.
            "member_expression" | "attribute" | "field_expression" => {
                let field = match callee.kind() {
                    "member_expression" => "property",
                    "attribute" => "attribute",
                    _ => "field",
                };

                let Some(name) = callee.child_by_field_name(field) else {
                    return;
                };

                let receiver = callee
                    .child_by_field_name(if field == "field" { "value" } else { "object" })
                    .filter(|object| object.kind() == "identifier")
                    .map(|object| self.text(object));

                (name, receiver, true)
            }

            // `module::function()`
            "scoped_identifier" => {
                let Some(name) = callee.child_by_field_name("name") else {
                    return;
                };

                let receiver = callee
                    .child_by_field_name("path")
                    .filter(|path| path.kind() == "identifier")
                    .map(|path| self.text(path));

                (name, receiver, true)
            }

            // `function::<T>()`
            "generic_function" => {
                return self.record_callee(callee.child_by_field_name("function"));
            }

            _ => return,
        };

        self.handled.insert(name_node.id());

        self.references.uses.push(NameUse {
            name: self.text(name_node),
            kind: UseKind::Call,
            line: line_of(name_node),
            receiver,
            is_member,
        });
    }

    fn record_import_name(&mut self, node: Node) {
        self.mark_handled(node);

        // A dotted Python name is a module path, not a single name.
        if node.kind() == "dotted_name" && node.named_child_count() > 1 {
            return;
        }

        self.references.uses.push(NameUse {
            name: self.text(node),
            kind: UseKind::Import,
            line: line_of(node),
            receiver: None,
            is_member: false,
        });
    }

    /// Any other appearance of a name.
    fn record_mention(&mut self, node: Node) {
        let is_member = match node.kind() {
            "identifier" | "type_identifier" | "shorthand_property_identifier" => false,
            "property_identifier" | "field_identifier" => true,
            _ => return,
        };

        if self.handled.contains(&node.id()) || is_definition_name(node) {
            return;
        }

        self.references.uses.push(NameUse {
            name: self.text(node),
            kind: UseKind::Mention,
            line: line_of(node),
            receiver: None,
            is_member,
        });
    }

    /// Keeps the generic identifier rule away from `node` and everything
    /// under it.
    fn mark_handled(&mut self, node: Node) {
        self.handled.insert(node.id());

        let mut cursor = node.walk();

        for child in node.children(&mut cursor) {
            self.mark_handled(child);
        }
    }

    fn text(&self, node: Node) -> String {
        self.source[node.byte_range()].to_string()
    }
}

/// Whether `node` is the name being declared by its parent (`function
/// name`, `class Name`, `const name =`), which is a definition, not a use.
fn is_definition_name(node: Node) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };

    let declares = matches!(
        parent.kind(),
        "function_declaration"
            | "generator_function_declaration"
            | "class_declaration"
            | "abstract_class_declaration"
            | "method_definition"
            | "interface_declaration"
            | "type_alias_declaration"
            | "enum_declaration"
            | "variable_declarator"
            | "function_definition"
            | "class_definition"
            | "function_item"
            | "struct_item"
            | "enum_item"
            | "trait_item"
            | "mod_item"
            | "type_item"
            | "const_item"
            | "static_item"
    );

    declares
        && parent
            .child_by_field_name("name")
            .is_some_and(|name| name.id() == node.id())
}

fn last_named_child(node: Node) -> Option<Node> {
    let mut cursor = node.walk();

    node.named_children(&mut cursor).last()
}

fn line_of(node: Node) -> usize {
    node.start_position().row + 1
}

fn unquote(text: &str) -> String {
    text.trim_matches(['"', '\'', '`']).to_string()
}

/// Splits `crate::a::b` into the module `crate::a` and the name `b`.
fn split_rust_path(path: &str) -> (String, String) {
    match path.rsplit_once("::") {
        Some((module, name)) => (module.to_string(), name.to_string()),
        None => (String::new(), path.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calls(references: &FileReferences) -> Vec<&str> {
        references
            .uses
            .iter()
            .filter(|usage| usage.kind == UseKind::Call)
            .map(|usage| usage.name.as_str())
            .collect()
    }

    #[test]
    fn typescript_imports_with_aliases_and_namespaces() {
        let source = "\
import React from \"react\";
import { toCents, format as fmt } from \"@/lib/money\";
import * as dates from \"./dates\";
export { helper } from \"./helper\";
";

        let references = extract_references(source, Language::TypeScript);

        let sources: Vec<&str> = references
            .imports
            .iter()
            .map(|import| import.source.as_str())
            .collect();

        assert_eq!(sources, ["react", "@/lib/money", "./dates", "./helper"]);

        let money = &references.imports[1];

        assert_eq!(money.names[1].name, "format");
        assert_eq!(money.names[1].local, "fmt");

        assert_eq!(references.imports[2].names[0].local, "dates");
        assert!(references.imports[3].is_reexport);
    }

    #[test]
    fn typescript_calls_members_constructors_and_components() {
        let source = "\
export function total(rows: Row[]) {
  const cents = rows.map((row) => toCents(row.fee));
  const view = new Ledger(cents);
  return <Summary value={dates.today()} />;
}
";

        let references = extract_references(source, Language::Tsx);

        assert_eq!(
            calls(&references),
            ["map", "toCents", "Ledger", "Summary", "today"]
        );

        let today = references
            .uses
            .iter()
            .find(|usage| usage.name == "today")
            .unwrap();

        assert_eq!(today.receiver.as_deref(), Some("dates"));
        assert!(today.is_member);

        // The function's own name is a definition, not a use.
        assert!(!references.uses.iter().any(|usage| usage.name == "total"));
    }

    #[test]
    fn a_name_passed_as_a_value_is_a_mention() {
        let source = "const all = rows.map(toCents);\n";

        let references = extract_references(source, Language::TypeScript);

        let usage = references
            .uses
            .iter()
            .find(|usage| usage.name == "toCents")
            .unwrap();

        assert_eq!(usage.kind, UseKind::Mention);
    }

    #[test]
    fn python_imports_and_calls() {
        let source = "\
import os.path
from app.money import to_cents, fmt as format_money
from . import models

def total(rows):
    return sum(to_cents(row) for row in rows) + models.base()
";

        let references = extract_references(source, Language::Python);

        let sources: Vec<&str> = references
            .imports
            .iter()
            .map(|import| import.source.as_str())
            .collect();

        assert_eq!(sources, ["os.path", "app.money", "."]);

        assert_eq!(references.imports[0].names[0].local, "os");
        assert_eq!(references.imports[1].names[1].local, "format_money");

        assert_eq!(calls(&references), ["sum", "to_cents", "base"]);
    }

    #[test]
    fn rust_use_lists_and_calls() {
        let source = "\
use crate::money::{to_cents, Money as Cash};
use std::fs;

fn total() -> u64 {
    let text = fs::read_to_string(\"a\").unwrap();
    to_cents(&text)
}
";

        let references = extract_references(source, Language::Rust);

        let imported: Vec<(&str, &str, &str)> = references
            .imports
            .iter()
            .map(|import| {
                (
                    import.source.as_str(),
                    import.names[0].name.as_str(),
                    import.names[0].local.as_str(),
                )
            })
            .collect();

        assert_eq!(
            imported,
            [
                ("crate::money", "to_cents", "to_cents"),
                ("crate::money", "Money", "Cash"),
                ("std", "fs", "fs"),
            ]
        );

        assert_eq!(calls(&references), ["unwrap", "read_to_string", "to_cents"]);
    }
}
