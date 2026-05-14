use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolResult {
    pub success: bool,
    pub output: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ToolResult {
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            success: true,
            output: output.into(),
            error: None,
        }
    }

    pub fn failure(error: impl Into<String>) -> Self {
        let error = error.into();
        Self {
            success: false,
            output: String::new(),
            error: Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn tool_result_success() {
        let result = ToolResult::success("file content here");
        assert!(result.success);
        assert_eq!(result.output, "file content here");
        assert!(result.error.is_none());
    }

    #[test]
    fn tool_result_failure() {
        let result = ToolResult::failure("file not found");
        assert!(!result.success);
        assert!(result.output.is_empty());
        assert_eq!(result.error.as_deref(), Some("file not found"));
    }

    #[test]
    fn tool_spec_serialization() {
        let spec = ToolSpec {
            name: "read_file".into(),
            description: "Read a file".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path" }
                },
                "required": ["path"]
            }),
        };
        let json = serde_json::to_string(&spec).unwrap();
        let deserialized: ToolSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "read_file");
    }
}
