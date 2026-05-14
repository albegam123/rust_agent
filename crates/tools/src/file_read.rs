use anyhow::Result;
use async_trait::async_trait;
use ragent_traits::tool::Tool;
use ragent_types::tool::{ToolResult, ToolSpec};
use tracing::debug;

pub struct FileReadTool;

#[async_trait]
impl Tool for FileReadTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read_file".into(),
            description: "Read the contents of a file at the given path. \
                          Use offset and limit to read specific portions of large files."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The file path to read"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Line offset to start reading from (0-based)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of lines to read"
                    }
                },
                "required": ["path"]
            }),
        }
    }

    async fn call(&self, args: serde_json::Value) -> Result<ToolResult> {
        let path = args["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: path"))?;
        let offset = args["offset"].as_u64().unwrap_or(0) as usize;
        let limit = args["limit"].as_u64().map(|v| v as usize);

        debug!(path, offset, ?limit, "reading file");

        match tokio::fs::read_to_string(path).await {
            Ok(content) => {
                let lines: Vec<&str> = content.lines().collect();
                let total_lines = lines.len();

                let start = offset.min(total_lines);
                let end = match limit {
                    Some(lim) => (start + lim).min(total_lines),
                    None => total_lines,
                };

                let selected: Vec<String> = lines[start..end]
                    .iter()
                    .enumerate()
                    .map(|(i, line)| format!("{:>6}|{line}", start + i + 1))
                    .collect();

                let mut output = selected.join("\n");
                if start > 0 || end < total_lines {
                    output = format!(
                        "[showing lines {}-{} of {total_lines}]\n{output}",
                        start + 1,
                        end
                    );
                }

                Ok(ToolResult::success(output))
            }
            Err(e) => Ok(ToolResult::failure(format!("failed to read file: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn read_file_success() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        tokio::fs::write(&file_path, b"line 1\nline 2\nline 3\n")
            .await
            .unwrap();

        let tool = FileReadTool;
        let result = tool
            .call(serde_json::json!({"path": file_path.to_str().unwrap()}))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("line 1"));
        assert!(result.output.contains("line 3"));
    }

    #[tokio::test]
    async fn read_file_with_offset_and_limit() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        tokio::fs::write(&file_path, b"a\nb\nc\nd\ne\n")
            .await
            .unwrap();

        let tool = FileReadTool;
        let result = tool
            .call(serde_json::json!({
                "path": file_path.to_str().unwrap(),
                "offset": 1,
                "limit": 2
            }))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("showing lines 2-3 of 5"));
    }

    #[tokio::test]
    async fn read_file_not_found() {
        let tool = FileReadTool;
        let result = tool
            .call(serde_json::json!({"path": "/nonexistent/file.txt"}))
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.error.unwrap().contains("failed to read file"));
    }
}
