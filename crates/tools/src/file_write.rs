use anyhow::Result;
use async_trait::async_trait;
use ragent_traits::tool::Tool;
use ragent_types::tool::{ToolResult, ToolSpec};
use tracing::debug;

pub struct FileWriteTool;

#[async_trait]
impl Tool for FileWriteTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::function(
            "write_file",
            "Write content to a file, creating it if it doesn't exist \
             or overwriting if it does.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The file path to write to"
                    },
                    "content": {
                        "type": "string",
                        "description": "The content to write"
                    }
                },
                "required": ["path", "content"]
            }),
        )
    }

    async fn call(&self, args: serde_json::Value) -> Result<ToolResult> {
        let path = args["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: path"))?;
        let content = args["content"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: content"))?;

        debug!(path, bytes = content.len(), "writing file");

        if let Some(parent) = std::path::Path::new(path).parent() {
            if !parent.exists() {
                if let Err(e) = tokio::fs::create_dir_all(parent).await {
                    return Ok(ToolResult::failure(format!(
                        "failed to create parent directory: {e}"
                    )));
                }
            }
        }

        match tokio::fs::write(path, content).await {
            Ok(()) => Ok(ToolResult::success(format!(
                "wrote {} bytes to {path}",
                content.len()
            ))),
            Err(e) => Ok(ToolResult::failure(format!("failed to write file: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn write_file_creates_new() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("new_file.txt");

        let tool = FileWriteTool;
        let result = tool
            .call(serde_json::json!({
                "path": file_path.to_str().unwrap(),
                "content": "hello world"
            }))
            .await
            .unwrap();

        assert!(result.success);
        let content = String::from_utf8(tokio::fs::read(&file_path).await.unwrap()).unwrap();
        assert_eq!(content, "hello world");
    }

    #[tokio::test]
    async fn write_file_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("a/b/c/deep_file.txt");

        let tool = FileWriteTool;
        let result = tool
            .call(serde_json::json!({
                "path": file_path.to_str().unwrap(),
                "content": "deep content"
            }))
            .await
            .unwrap();

        assert!(result.success);
        let content = String::from_utf8(tokio::fs::read(&file_path).await.unwrap()).unwrap();
        assert_eq!(content, "deep content");
    }
}
