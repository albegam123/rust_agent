use anyhow::Result;
use async_trait::async_trait;
use ragent_traits::llm::LLMProvider;
use ragent_types::config::RetryConfig;
use ragent_types::message::{FunctionCall, LLMResponse, Message, Role, ToolCall, Usage};
use ragent_types::tool::ToolSpec;
use tracing::debug;

pub struct AnthropicProvider {
    api_key: String,
    api_base: String,
    model: String,
    retry_config: RetryConfig,
    client: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(
        api_key: String,
        api_base: Option<String>,
        model: String,
        retry_config: RetryConfig,
    ) -> Self {
        Self {
            api_key,
            api_base: api_base.unwrap_or_else(|| "https://api.anthropic.com".to_string()),
            model,
            retry_config,
            client: reqwest::Client::new(),
        }
    }

    /// Resolves the API root before `/v1/messages`.
    ///
    /// - Official Anthropic: `https://api.anthropic.com` → unchanged.
    /// - MiniMax (intl / 国内): docs use `https://api.minimax.io/anthropic` or
    ///   `https://api.minimaxi.com/anthropic`. Users often set only the origin
    ///   (`https://api.minimaxi.com`); the gateway still serves the protocol under
    ///   `/anthropic`, so we append it when the host is MiniMax and the path is empty.
    fn resolved_anthropic_root(&self) -> String {
        let base = self.api_base.trim_end_matches('/').to_string();
        if base.ends_with("/anthropic") {
            return base;
        }
        const MINIMAX_ORIGIN_ONLY: &[&str] = &[
            "https://api.minimaxi.com",
            "http://api.minimaxi.com",
            "https://api.minimax.io",
            "http://api.minimax.io",
        ];
        if MINIMAX_ORIGIN_ONLY.iter().any(|&o| base == o) {
            return format!("{base}/anthropic");
        }
        base
    }

    /// Full URL for Messages API, matching the Python Anthropic SDK:
    /// `AsyncAnthropic(base_url=...)` posts to `{base_url}/v1/messages`.
    fn messages_url(&self) -> String {
        let root = self.resolved_anthropic_root();
        let root = root.trim_end_matches('/');
        format!("{root}/v1/messages")
    }

    fn build_request_body(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
    ) -> (Option<String>, serde_json::Value) {
        let mut system_prompt = None;
        let mut api_messages = Vec::new();

        for msg in messages {
            match msg.role {
                Role::System => {
                    system_prompt = msg.content.clone();
                }
                Role::User => {
                    api_messages.push(serde_json::json!({
                        "role": "user",
                        "content": msg.content.as_deref().unwrap_or(""),
                    }));
                }
                Role::Assistant => {
                    let mut content_blocks = Vec::new();

                    if let Some(thinking) = &msg.thinking {
                        content_blocks.push(serde_json::json!({
                            "type": "thinking",
                            "thinking": thinking,
                        }));
                    }

                    if let Some(text) = &msg.content {
                        content_blocks.push(serde_json::json!({
                            "type": "text",
                            "text": text,
                        }));
                    }

                    if let Some(tool_calls) = &msg.tool_calls {
                        for tc in tool_calls {
                            let args: serde_json::Value =
                                serde_json::from_str(&tc.function.arguments)
                                    .unwrap_or(serde_json::Value::Object(Default::default()));
                            content_blocks.push(serde_json::json!({
                                "type": "tool_use",
                                "id": tc.id,
                                "name": tc.function.name,
                                "input": args,
                            }));
                        }
                    }

                    if content_blocks.is_empty() {
                        content_blocks.push(serde_json::json!({"type": "text", "text": ""}));
                    }

                    api_messages.push(serde_json::json!({
                        "role": "assistant",
                        "content": content_blocks,
                    }));
                }
                Role::Tool => {
                    api_messages.push(serde_json::json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": msg.tool_call_id.as_deref().unwrap_or(""),
                            "content": msg.content.as_deref().unwrap_or(""),
                        }],
                    }));
                }
            }
        }

        let mut body = serde_json::json!({
            "model": self.model,
            "messages": api_messages,
            "max_tokens": 8192,
        });

        if let Some(system) = &system_prompt {
            body["system"] = serde_json::Value::String(system.clone());
        }

        if !tools.is_empty() {
            let tool_schemas: Vec<serde_json::Value> = tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.parameters,
                    })
                })
                .collect();
            body["tools"] = serde_json::Value::Array(tool_schemas);
        }

        (system_prompt, body)
    }

    fn parse_response(&self, json: &serde_json::Value) -> Result<LLMResponse> {
        let content_blocks = json["content"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("no content in Anthropic response"))?;

        let mut text_content = String::new();
        let mut thinking = None;
        let mut tool_calls = Vec::new();

        for block in content_blocks {
            match block["type"].as_str() {
                Some("text") => {
                    if let Some(text) = block["text"].as_str() {
                        text_content.push_str(text);
                    }
                }
                Some("thinking") => {
                    thinking = block["thinking"].as_str().map(String::from);
                }
                Some("tool_use") => {
                    if let (Some(id), Some(name)) = (block["id"].as_str(), block["name"].as_str()) {
                        let input = &block["input"];
                        tool_calls.push(ToolCall {
                            id: id.to_string(),
                            call_type: "function".to_string(),
                            function: FunctionCall {
                                name: name.to_string(),
                                arguments: serde_json::to_string(input).unwrap_or_default(),
                            },
                        });
                    }
                }
                _ => {}
            }
        }

        let content = if text_content.is_empty() {
            None
        } else {
            Some(text_content)
        };

        let finish_reason = json["stop_reason"].as_str().map(String::from);

        let usage = json.get("usage").map(|u| Usage {
            prompt_tokens: u["input_tokens"].as_u64().unwrap_or(0) as u32,
            completion_tokens: u["output_tokens"].as_u64().unwrap_or(0) as u32,
            total_tokens: (u["input_tokens"].as_u64().unwrap_or(0)
                + u["output_tokens"].as_u64().unwrap_or(0)) as u32,
        });

        Ok(LLMResponse {
            content,
            thinking,
            tool_calls,
            finish_reason,
            usage,
        })
    }
}

#[async_trait]
impl LLMProvider for AnthropicProvider {
    async fn chat(&self, messages: &[Message], tools: &[ToolSpec]) -> Result<LLMResponse> {
        let (_system, body) = self.build_request_body(messages, tools);
        let url = self.messages_url();

        debug!(provider = "anthropic", model = %self.model, url = %url, "sending chat request");

        let make_request = || {
            let client = self.client.clone();
            let url = url.clone();
            let api_key = self.api_key.clone();
            let body = body.clone();
            async move {
                let resp = client
                    .post(&url)
                    .header("x-api-key", &api_key)
                    .header("Authorization", format!("Bearer {api_key}"))
                    .header("anthropic-version", "2023-06-01")
                    .header("Content-Type", "application/json")
                    .json(&body)
                    .send()
                    .await?;

                let status = resp.status();
                let text = resp.text().await?;

                if !status.is_success() {
                    anyhow::bail!("Anthropic API error ({status}): {text}");
                }

                let json: serde_json::Value = serde_json::from_str(&text)?;
                Ok(json)
            }
        };

        let response_json = crate::retry::with_retry(&self.retry_config, make_request)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        self.parse_response(&response_json)
    }

    fn name(&self) -> &str {
        "anthropic"
    }

    fn model(&self) -> &str {
        &self.model
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_url_matches_anthropic_sdk() {
        let p = AnthropicProvider::new(
            "k".into(),
            Some("https://api.minimaxi.com/anthropic".into()),
            "m".into(),
            RetryConfig::default(),
        );
        assert_eq!(
            p.messages_url(),
            "https://api.minimaxi.com/anthropic/v1/messages"
        );

        // Common misconfig: origin only (no /anthropic) — same effective URL as Python default.
        let origin_only = AnthropicProvider::new(
            "k".into(),
            Some("https://api.minimaxi.com".into()),
            "m".into(),
            RetryConfig::default(),
        );
        assert_eq!(
            origin_only.messages_url(),
            "https://api.minimaxi.com/anthropic/v1/messages"
        );

        let official = AnthropicProvider::new("k".into(), None, "m".into(), RetryConfig::default());
        assert_eq!(
            official.messages_url(),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn parse_anthropic_text_response() {
        let provider = AnthropicProvider::new(
            "test-key".into(),
            None,
            "claude-3-5-sonnet".into(),
            RetryConfig::default(),
        );
        let json = serde_json::json!({
            "content": [
                { "type": "text", "text": "Hello! How can I help?" }
            ],
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 10, "output_tokens": 8 }
        });

        let response = provider.parse_response(&json).unwrap();
        assert_eq!(response.content.as_deref(), Some("Hello! How can I help?"));
        assert!(response.tool_calls.is_empty());
    }

    #[test]
    fn parse_anthropic_tool_use_response() {
        let provider = AnthropicProvider::new(
            "test-key".into(),
            None,
            "claude-3-5-sonnet".into(),
            RetryConfig::default(),
        );
        let json = serde_json::json!({
            "content": [
                { "type": "thinking", "thinking": "I need to read the file..." },
                {
                    "type": "tool_use",
                    "id": "toolu_abc",
                    "name": "read_file",
                    "input": { "path": "src/main.rs" }
                }
            ],
            "stop_reason": "tool_use",
            "usage": { "input_tokens": 50, "output_tokens": 30 }
        });

        let response = provider.parse_response(&json).unwrap();
        assert!(response.content.is_none());
        assert_eq!(
            response.thinking.as_deref(),
            Some("I need to read the file...")
        );
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].function.name, "read_file");
    }
}
