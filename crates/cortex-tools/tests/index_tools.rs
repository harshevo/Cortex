//! The code index tools, run the way the agent runs them.

use std::fs;
use std::sync::Arc;

use cortex_core::{AutoApprove, ToolCall, ToolContext, ToolRegistry, Workspace};
use serde_json::{Value, json};

const MONEY: &str = "\
export type Currency = \"USD\" | \"USDT\";

export function toCents(value: string): bigint {
  return BigInt(value);
}

export function formatMoney(cents: bigint): string {
  return cents.toString();
}
";

const TAX: &str = "\
def compute_tax(amount, rate):
    return round(amount * rate, 2)
";

fn project() -> (tempfile::TempDir, ToolRegistry, ToolContext) {
    let dir = tempfile::tempdir().unwrap();

    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/money.ts"), MONEY).unwrap();
    fs::write(dir.path().join("tax.py"), TAX).unwrap();

    let mut registry = cortex_tools::default_registry();

    cortex_tools::add_code_index_tools(&mut registry);

    let context = ToolContext {
        workspace: Workspace::new(dir.path()).unwrap(),
        approver: Arc::new(AutoApprove),
    };

    (dir, registry, context)
}

async fn call(registry: &ToolRegistry, context: &ToolContext, name: &str, input: Value) -> String {
    let call = ToolCall {
        id: "call".to_string(),
        name: name.to_string(),
        input,
    };

    let result = registry.dispatch(&call, context).await;

    assert!(!result.is_error, "{name} failed: {}", result.output);

    result.output
}

#[tokio::test]
async fn outline_lists_symbols_with_line_ranges() {
    let (_dir, registry, context) = project();

    let output = call(
        &registry,
        &context,
        "outline",
        json!({ "path": "src/money.ts" }),
    )
    .await;

    assert!(output.starts_with("src/money.ts (9 lines)"));
    assert!(output.contains("1-1  type  export type Currency = "));
    assert!(output.contains("3-5  function  export function toCents(value: string): bigint"));
    assert!(output.contains("7-9  function  export function formatMoney"));
}

#[tokio::test]
async fn read_symbol_returns_numbered_source_from_any_file() {
    let (_dir, registry, context) = project();

    let typescript = call(
        &registry,
        &context,
        "read_symbol",
        json!({ "name": "formatMoney" }),
    )
    .await;

    assert!(typescript.starts_with("src/money.ts:7-9  function formatMoney"));
    assert!(typescript.contains("8\t  return cents.toString();"));

    let python = call(
        &registry,
        &context,
        "read_symbol",
        json!({ "name": "compute_tax" }),
    )
    .await;

    assert!(python.starts_with("tax.py:1-2  function compute_tax"));
}

#[tokio::test]
async fn read_symbol_says_when_nothing_matches() {
    let (_dir, registry, context) = project();

    let output = call(
        &registry,
        &context,
        "read_symbol",
        json!({ "name": "doesNotExist" }),
    )
    .await;

    assert!(output.contains("No definition named `doesNotExist`"));
}
