use anyhow::Result;
use async_trait::async_trait;
use ragent_traits::tool::Tool;
use ragent_types::tool::{ToolResult, ToolSpec};
use tracing::debug;

pub struct FileEditTool;

#[async_trait]
impl Tool for FileEditTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::function(
            "edit_file",
            "Edit a file by replacing an exact string match with new content. \
             The old_string must uniquely identify the text to replace.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The file path to edit"
                    },
                    "old_string": {
                        "type": "string",
                        "description": "The exact string to find and replace"
                    },
                    "new_string": {
                        "type": "string",
                        "description": "The replacement string"
                    }
                },
                "required": ["path", "old_string", "new_string"]
            }),
        )
    }

    async fn call(&self, args: serde_json::Value) -> Result<ToolResult> {
        let path = args["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: path"))?;
        let old_string = args["old_string"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: old_string"))?;
        let new_string = args["new_string"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: new_string"))?;

        debug!(
            path,
            old_len = old_string.len(),
            new_len = new_string.len(),
            "editing file"
        );

        let content = match tokio::fs::read_to_string(path).await {
            Ok(c) => c,
            Err(e) => return Ok(ToolResult::failure(format!("failed to read file: {e}"))),
        };

        let occurrences = content.matches(old_string).count();

        match occurrences {
            0 => Ok(ToolResult::failure(
                "old_string not found in file".to_string(),
            )),
            1 => {
                let new_content = content.replacen(old_string, new_string, 1);
                match tokio::fs::write(path, &new_content).await {
                    Ok(()) => Ok(ToolResult::success("edit applied successfully")),
                    Err(e) => Ok(ToolResult::failure(format!("failed to write file: {e}"))),
                }
            }
            n => Ok(ToolResult::failure(format!(
                "old_string found {n} times — must be unique. Add more surrounding context."
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn edit_file_single_match() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("edit_test.txt");
        tokio::fs::write(&file_path, b"fn main() {\n    println!(\"hello\");\n}\n")
            .await
            .unwrap();

        let tool = FileEditTool;
        let result = tool
            .call(serde_json::json!({
                "path": file_path.to_str().unwrap(),
                "old_string": "println!(\"hello\")",
                "new_string": "println!(\"goodbye\")"
            }))
            .await
            .unwrap();

        assert!(result.success);
        let content = String::from_utf8(tokio::fs::read(&file_path).await.unwrap()).unwrap();
        assert!(content.contains("goodbye"));
        assert!(!content.contains("hello"));
    }

    #[tokio::test]
    async fn edit_file_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("edit_test.txt");
        tokio::fs::write(&file_path, b"some content").await.unwrap();

        let tool = FileEditTool;
        let result = tool
            .call(serde_json::json!({
                "path": file_path.to_str().unwrap(),
                "old_string": "nonexistent text",
                "new_string": "replacement"
            }))
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.error.unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn edit_file_multiple_matches() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("edit_test.txt");
        tokio::fs::write(&file_path, b"aaa bbb aaa").await.unwrap();

        let tool = FileEditTool;
        let result = tool
            .call(serde_json::json!({
                "path": file_path.to_str().unwrap(),
                "old_string": "aaa",
                "new_string": "ccc"
            }))
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.error.unwrap().contains("2 times"));
    }
}
