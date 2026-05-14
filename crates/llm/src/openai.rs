use anyhow::Result;
use async_trait::async_trait;
use ragent_traits::llm::LLMProvider;
use ragent_types::config::RetryConfig;
use ragent_types::message::{
    FunctionCall, LLMResponse, Message, Role, ToolCall, Usage,
};
use ragent_types::tool::ToolSpec;
use tracing::debug;

pub struct OpenAIProvider {
    api_key: String,
    api_base: String,
    model: String,
    retry_config: RetryConfig,
    client: reqwest::Client,
}

impl OpenAIProvider {
    pub fn new(
        api_key: String,
        api_base: Option<String>,
        model: String,
        retry_config: RetryConfig,
    ) -> Self {
        Self {
            api_key,
            api_base: api_base.unwrap_or_else(|| "https://api.openai.com/v1".to_string()),
            model,
            retry_config,
            client: reqwest::Client::new(),
        }
    }

    fn build_request_body(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
    ) -> serde_json::Value {
        let api_messages: Vec<serde_json::Value> = messages
            .iter()
            .map(|msg| self.convert_message(msg))
            .collect();

        let mut body = serde_json::json!({
            "model": self.model,
            "messages": api_messages,
        });

        if !tools.is_empty() {
            let tool_schemas: Vec<serde_json::Value> = tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters,
                        }
                    })
                })
                .collect();
            body["tools"] = serde_json::Value::Array(tool_schemas);
        }

        body
    }

    fn convert_message(&self, msg: &Message) -> serde_json::Value {
        match msg.role {
            Role::System => serde_json::json!({
                "role": "system",
                "content": msg.content.as_deref().unwrap_or(""),
            }),
            Role::User => serde_json::json!({
                "role": "user",
                "content": msg.content.as_deref().unwrap_or(""),
            }),
            Role::Assistant => {
                let mut obj = serde_json::json!({
                    "role": "assistant",
                });
                if let Some(content) = &msg.content {
                    obj["content"] = serde_json::Value::String(content.clone());
                }
                if let Some(tool_calls) = &msg.tool_calls {
                    let tc: Vec<serde_json::Value> = tool_calls
                        .iter()
                        .map(|tc| {
                            serde_json::json!({
                                "id": tc.id,
                                "type": "function",
                                "function": {
                                    "name": tc.function.name,
                                    "arguments": tc.function.arguments,
                                }
                            })
                        })
                        .collect();
                    obj["tool_calls"] = serde_json::Value::Array(tc);
                }
                obj
            }
            Role::Tool => serde_json::json!({
                "role": "tool",
                "tool_call_id": msg.tool_call_id.as_deref().unwrap_or(""),
                "content": msg.content.as_deref().unwrap_or(""),
            }),
        }
    }

    fn parse_response(&self, response_json: &serde_json::Value) -> Result<LLMResponse> {
        let choice = response_json["choices"]
            .get(0)
            .ok_or_else(|| anyhow::anyhow!("no choices in response"))?;

        let message = &choice["message"];

        let content = message["content"].as_str().map(String::from);
        let finish_reason = choice["finish_reason"].as_str().map(String::from);

        let tool_calls = match message.get("tool_calls") {
            Some(serde_json::Value::Array(calls)) => calls
                .iter()
                .filter_map(|tc| {
                    Some(ToolCall {
                        id: tc["id"].as_str()?.to_string(),
                        call_type: "function".to_string(),
                        function: FunctionCall {
                            name: tc["function"]["name"].as_str()?.to_string(),
                            arguments: tc["function"]["arguments"]
                                .as_str()?
                                .to_string(),
                        },
                    })
                })
                .collect(),
            _ => vec![],
        };

        let usage = response_json.get("usage").map(|u| Usage {
            prompt_tokens: u["prompt_tokens"].as_u64().unwrap_or(0) as u32,
            completion_tokens: u["completion_tokens"].as_u64().unwrap_or(0) as u32,
            total_tokens: u["total_tokens"].as_u64().unwrap_or(0) as u32,
        });

        Ok(LLMResponse {
            content,
            thinking: None,
            tool_calls,
            finish_reason,
            usage,
        })
    }
}

#[async_trait]
impl LLMProvider for OpenAIProvider {
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
    ) -> Result<LLMResponse> {
        let body = self.build_request_body(messages, tools);
        let url = format!("{}/chat/completions", self.api_base);

        debug!(provider = "openai", model = %self.model, "sending chat request");

        let make_request = || {
            let client = self.client.clone();
            let url = url.clone();
            let api_key = self.api_key.clone();
            let body = body.clone();
            async move {
                let resp = client
                    .post(&url)
                    .header("Authorization", format!("Bearer {api_key}"))
                    .header("Content-Type", "application/json")
                    .json(&body)
                    .send()
                    .await?;

                let status = resp.status();
                let response_text = resp.text().await?;

                if !status.is_success() {
                    anyhow::bail!("OpenAI API error ({}): {}", status, response_text);
                }

                let response_json: serde_json::Value =
                    serde_json::from_str(&response_text)?;
                Ok(response_json)
            }
        };

        let response_json =
            crate::retry::with_retry(&self.retry_config, make_request)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;

        self.parse_response(&response_json)
    }

    fn name(&self) -> &str {
        "openai"
    }

    fn model(&self) -> &str {
        &self.model
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_request_body_without_tools() {
        let provider = OpenAIProvider::new(
            "test-key".into(),
            None,
            "gpt-4o".into(),
            RetryConfig::default(),
        );
        let messages = vec![
            Message::system("You are helpful."),
            Message::user("Hello"),
        ];
        let body = provider.build_request_body(&messages, &[]);

        assert_eq!(body["model"], "gpt-4o");
        assert_eq!(body["messages"].as_array().unwrap().len(), 2);
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn build_request_body_with_tools() {
        let provider = OpenAIProvider::new(
            "test-key".into(),
            None,
            "gpt-4o".into(),
            RetryConfig::default(),
        );
        let messages = vec![Message::user("read my file")];
        let tools = vec![ToolSpec {
            name: "read_file".into(),
            description: "Read a file".into(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }];
        let body = provider.build_request_body(&messages, &tools);

        assert_eq!(body["tools"].as_array().unwrap().len(), 1);
        assert_eq!(body["tools"][0]["function"]["name"], "read_file");
    }

    #[test]
    fn parse_response_content_only() {
        let provider = OpenAIProvider::new(
            "test-key".into(),
            None,
            "gpt-4o".into(),
            RetryConfig::default(),
        );
        let json = serde_json::json!({
            "choices": [{
                "message": { "role": "assistant", "content": "Hello!" },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15
            }
        });

        let response = provider.parse_response(&json).unwrap();
        assert_eq!(response.content.as_deref(), Some("Hello!"));
        assert!(response.tool_calls.is_empty());
        assert_eq!(response.usage.unwrap().total_tokens, 15);
    }

    #[test]
    fn parse_response_with_tool_calls() {
        let provider = OpenAIProvider::new(
            "test-key".into(),
            None,
            "gpt-4o".into(),
            RetryConfig::default(),
        );
        let json = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_abc123",
                        "type": "function",
                        "function": {
                            "name": "bash",
                            "arguments": "{\"command\":\"ls -la\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {
                "prompt_tokens": 50,
                "completion_tokens": 20,
                "total_tokens": 70
            }
        });

        let response = provider.parse_response(&json).unwrap();
        assert!(response.content.is_none());
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].function.name, "bash");
        assert_eq!(response.finish_reason.as_deref(), Some("tool_calls"));
    }
}
