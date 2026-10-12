//! Helpers shared by the tools.

use cortex_core::ToolError;
use globset::{Glob, GlobMatcher};
use serde::de::DeserializeOwned;
use serde_json::Value;

/// Decodes the model's JSON arguments into a tool's input struct.
pub fn parse<T: DeserializeOwned>(input: Value) -> Result<T, ToolError> {
    serde_json::from_value(input).map_err(|error| ToolError::InvalidInput(error.to_string()))
}

/// Wraps an I/O failure with the path it happened on.
pub fn io_error(action: &str, path: &str, error: std::io::Error) -> ToolError {
    ToolError::Failed(format!("could not {action} `{path}`: {error}"))
}

pub fn compile_glob(pattern: &str) -> Result<GlobMatcher, ToolError> {
    let glob = Glob::new(pattern)
        .map_err(|error| ToolError::InvalidInput(format!("bad glob `{pattern}`: {error}")))?;

    Ok(glob.compile_matcher())
}
