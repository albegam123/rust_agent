use serde::{Deserialize, Serialize};

/// JSON shape for `/v1/chat/completions` function tools AND `/v1/responses` hosted `web_search`
/// (borrowed from codex-rs tooling model).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolSpec {
    #[serde(rename = "function")]
    Function {
        name: String,
        description: String,
        parameters: serde_json::Value,
    },
    /// OpenAI Responses API hosted search (runs on the provider; no local executor entry).
    #[serde(rename = "web_search")]
    WebSearch {
        external_web_access: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        search_context_size: Option<WebSearchContextSize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        filters: Option<WebSearchFilters>,
        #[serde(skip_serializing_if = "Option::is_none")]
        user_location: Option<WebSearchUserLocation>,
    },
}

impl ToolSpec {
    pub fn function(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Self {
        Self::Function {
            name: name.into(),
            description: description.into(),
            parameters,
        }
    }

    /// Executable tool name (`None` for provider-hosted tools like `WebSearch`).
    pub fn callable_name(&self) -> Option<&str> {
        match self {
            ToolSpec::Function { name, .. } => Some(name.as_str()),
            ToolSpec::WebSearch { .. } => None,
        }
    }

    pub fn hosted_web_search(
        external_web_access: bool,
        search_context_size: Option<WebSearchContextSize>,
        filters: Option<WebSearchFilters>,
        user_location: Option<WebSearchUserLocation>,
    ) -> Self {
        Self::WebSearch {
            external_web_access,
            search_context_size,
            filters,
            user_location,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WebSearchContextSize {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebSearchFilters {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_domains: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WebSearchUserLocationType {
    Approximate,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebSearchUserLocation {
    #[serde(rename = "type")]
    pub location_type: WebSearchUserLocationType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
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
    fn tool_spec_function_wire_shape() {
        let spec = ToolSpec::function(
            "read_file",
            "Read a file",
            serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }),
        );
        let v = serde_json::to_value(&spec).unwrap();
        assert_eq!(v["type"], "function");
        assert_eq!(v["name"], "read_file");
    }

    #[test]
    fn tool_spec_web_search_wire_shape_matches_codex() {
        let spec = ToolSpec::hosted_web_search(
            true,
            Some(WebSearchContextSize::High),
            Some(WebSearchFilters {
                allowed_domains: Some(vec!["example.com".into()]),
            }),
            Some(WebSearchUserLocation {
                location_type: WebSearchUserLocationType::Approximate,
                country: Some("US".into()),
                region: None,
                city: Some("New York".into()),
                timezone: Some("America/New_York".into()),
            }),
        );
        let v = serde_json::to_value(&spec).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "type": "web_search",
                "external_web_access": true,
                "search_context_size": "high",
                "filters": { "allowed_domains": ["example.com"] },
                "user_location": {
                    "type": "approximate",
                    "country": "US",
                    "city": "New York",
                    "timezone": "America/New_York",
                },
            })
        );
    }

    #[test]
    fn tool_spec_roundtrip() {
        let spec = ToolSpec::function(
            "bash",
            "Run bash",
            serde_json::json!({"type": "object"}),
        );
        let json = serde_json::to_string(&spec).unwrap();
        let deserialized: ToolSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, spec);
    }
}
