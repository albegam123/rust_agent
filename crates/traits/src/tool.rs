use anyhow::Result;
use async_trait::async_trait;
use ragent_types::tool::{ToolResult, ToolSpec};

/// Tool abstraction.
/// Borrowed from zeroclaw: each tool implements this trait
/// and is registered via factory functions.
#[async_trait]
pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;

    async fn call(&self, args: serde_json::Value) -> Result<ToolResult>;
}
