use anyhow::Result;
use async_trait::async_trait;
use futures_util::StreamExt;
use ragent_traits::llm::LLMProvider;
use ragent_types::config::RetryConfig;
use ragent_types::message::{FunctionCall, LLMResponse, Message, Role, ToolCall, Usage};
use ragent_types::tool::ToolSpec;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use tracing::{debug, warn};

#[derive(Default)]
struct ToolDeltaState {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Default)]
struct ChatStreamAccumulator {
    content: String,
    thinking: Option<String>,
    tool_calls: BTreeMap<u32, ToolDeltaState>,
    finish_reason: Option<String>,
    usage: Option<Usage>,
}

pub struct OpenAIProvider {
    api_key: String,
    api_base: String,
    model: String,
    retry_config: RetryConfig,
    client: reqwest::Client,
    /// When true, use `/v1/responses` because the tool strip includes hosted `web_search`.
    responses_api: bool,
}

impl OpenAIProvider {
    pub fn new(
        api_key: String,
        api_base: Option<String>,
        model: String,
        retry_config: RetryConfig,
        responses_api_for_hosted_tools: bool,
    ) -> Self {
        Self {
            api_key,
            api_base: api_base.unwrap_or_else(|| "https://api.openai.com/v1".to_string()),
            model,
            retry_config,
            client: reqwest::Client::new(),
            responses_api: responses_api_for_hosted_tools,
        }
    }

    fn chat_completions_url(&self) -> String {
        format!("{}/chat/completions", self.api_base.trim_end_matches('/'))
    }

    fn responses_url(&self) -> String {
        format!("{}/responses", self.api_base.trim_end_matches('/'))
    }

    fn uses_hosted_tools(tools: &[ToolSpec]) -> bool {
        tools.iter().any(|t| matches!(t, ToolSpec::WebSearch { .. }))
    }

    fn chat_completion_payload(&self, messages: &[Message], tools: &[ToolSpec], stream: bool) -> Value {
        let api_messages: Vec<Value> = messages.iter().map(|msg| self.convert_message(msg)).collect();

        let mut body = json!({
            "model": self.model,
            "messages": api_messages,
        });

        let function_specs: Vec<&ToolSpec> = tools
            .iter()
            .filter(|t| matches!(t, ToolSpec::Function { .. }))
            .collect();

        if !function_specs.is_empty() {
            let tool_schemas: Vec<Value> = function_specs
                .iter()
                .map(|t| match t {
                    ToolSpec::Function {
                        name,
                        description,
                        parameters,
                    } => json!({
                        "type": "function",
                        "function": {
                            "name": name,
                            "description": description,
                            "parameters": parameters,
                        }
                    }),
                    ToolSpec::WebSearch { .. } => unreachable!(),
                })
                .collect();
            body["tools"] = Value::Array(tool_schemas);
        }

        if stream {
            body["stream"] = json!(true);
            body["stream_options"] = json!({ "include_usage": true });
        }

        body
    }

    fn responses_body(&self, messages: &[Message], tools: &[ToolSpec]) -> Result<Value> {
        let (instructions, input) = self.messages_to_responses(messages)?;

        let tools_json: Vec<Value> = tools
            .iter()
            .map(|spec| serde_json::to_value(spec))
            .collect::<serde_json::Result<_>>()?;

        Ok(json!({
            "model": self.model,
            "instructions": instructions,
            "input": input,
            "parallel_tool_calls": true,
            "tools": tools_json,
        }))
    }

    fn messages_to_responses(&self, messages: &[Message]) -> Result<(String, Vec<Value>)> {
        let mut instructions: Vec<String> = Vec::new();
        let mut input: Vec<Value> = Vec::new();

        for msg in messages {
            match msg.role {
                Role::System => {
                    if let Some(c) = &msg.content {
                        if !c.is_empty() {
                            instructions.push(c.clone());
                        }
                    }
                }
                Role::User => {
                    let text = msg.content.clone().unwrap_or_default();
                    input.push(responses_user_message(&text));
                }
                Role::Assistant => {
                    let mut assembled = String::new();
                    if let Some(th) = msg.thinking.as_ref().filter(|s| !s.is_empty()) {
                        assembled.push_str(th);
                        assembled.push('\n');
                    }
                    if let Some(c) = msg.content.as_ref().filter(|s| !s.is_empty()) {
                        assembled.push_str(c);
                    }
                    assembled = assembled.trim_end().to_string();

                    let has_calls = msg
                        .tool_calls
                        .as_ref()
                        .map(|c| !c.is_empty())
                        .unwrap_or(false);

                    if !assembled.is_empty() {
                        input.push(responses_assistant_message(&assembled));
                    }

                    if has_calls {
                        for tc in msg.tool_calls.as_ref().unwrap() {
                            input.push(json!({
                                "type": "function_call",
                                "call_id": tc.id,
                                "name": tc.function.name,
                                "arguments": tc.function.arguments,
                            }));
                        }
                    }
                }
                Role::Tool => {
                    let call_id = msg
                        .tool_call_id
                        .clone()
                        .ok_or_else(|| anyhow::anyhow!("tool message without tool_call_id"))?;
                    let output = msg.content.clone().unwrap_or_default();
                    input.push(json!({
                        "type": "function_call_output",
                        "call_id": call_id,
                        "output": output,
                    }));
                }
            }
        }

        Ok((instructions.join("\n\n"), input))
    }

    fn convert_message(&self, msg: &Message) -> serde_json::Value {
        match msg.role {
            Role::System => json!({
                "role": "system",
                "content": msg.content.as_deref().unwrap_or(""),
            }),
            Role::User => json!({
                "role": "user",
                "content": msg.content.as_deref().unwrap_or(""),
            }),
            Role::Assistant => {
                let mut obj = json!({ "role": "assistant" });
                if let Some(content) = &msg.content {
                    obj["content"] = Value::String(content.clone());
                }
                if let Some(tool_calls) = &msg.tool_calls {
                    let tc: Vec<serde_json::Value> = tool_calls
                        .iter()
                        .map(|tc| {
                            json!({
                                "id": tc.id,
                                "type": "function",
                                "function": {
                                    "name": tc.function.name,
                                    "arguments": tc.function.arguments,
                                }
                            })
                        })
                        .collect();
                    obj["tool_calls"] = Value::Array(tc);
                }
                obj
            }
            Role::Tool => json!({
                "role": "tool",
                "tool_call_id": msg.tool_call_id.as_deref().unwrap_or(""),
                "content": msg.content.as_deref().unwrap_or(""),
            }),
        }
    }

    #[cfg(test)]
    fn parse_chat_completion_response(response_json: &Value) -> Result<LLMResponse> {
        let choice = response_json["choices"]
            .get(0)
            .ok_or_else(|| anyhow::anyhow!("no choices in response"))?;

        let message = &choice["message"];

        let content = message["content"].as_str().map(String::from);
        let finish_reason = choice["finish_reason"].as_str().map(String::from);

        let tool_calls = match message.get("tool_calls") {
            Some(Value::Array(calls)) => calls
                .iter()
                .filter_map(|tc| {
                    Some(ToolCall {
                        id: tc["id"].as_str()?.to_string(),
                        call_type: "function".to_string(),
                        function: FunctionCall {
                            name: tc["function"]["name"].as_str()?.to_string(),
                            arguments: tc["function"]["arguments"].as_str()?.to_string(),
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

    /// Applies one streamed chat-completion chunk (`choices[0].delta`, `usage`, …).
    fn accumulate_chat_completion_chunk(acc: &mut ChatStreamAccumulator, chunk: &Value) {
        if let Some(u) = chunk.get("usage") {
            acc.usage = Self::parse_usage_from_completion_json(u);
        }

        let Some(choices) = chunk.get("choices").and_then(Value::as_array) else {
            return;
        };
        let Some(choice) = choices.first() else {
            return;
        };

        if let Some(fr) = choice
            .get("finish_reason")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            acc.finish_reason = Some(fr.to_string());
        }

        let delta_payload = choice
            .get("delta")
            .filter(|v| !v.is_null());
        let delta = delta_payload.unwrap_or(choice);

        if let Some(c) = delta.get("content").and_then(Value::as_str) {
            acc.content.push_str(c);
        }

        // Some gateways omit `delta` nesting; still try reasoning / tools from `delta`.
        // collect common shapes defensively without failing strict proxies.
        if let Some(reason) = delta
            .get("reasoning")
            .or_else(|| delta.get("reasoning_content"))
            .and_then(Value::as_str)
        {
            acc.thinking.get_or_insert_with(String::new).push_str(reason);
        }

        let Some(patch_list) = delta.get("tool_calls").and_then(Value::as_array) else {
            return;
        };

        for patch in patch_list {
            let idx = patch
                .get("index")
                .and_then(|i| i.as_u64())
                .unwrap_or(0) as u32;

            let entry = acc.tool_calls.entry(idx).or_insert_with(ToolDeltaState::default);

            if let Some(id) = patch.get("id").and_then(Value::as_str) {
                if !id.is_empty() {
                    entry.id = id.to_string();
                }
            }

            if let Some(fc) = patch.get("function") {
                if let Some(name) = fc.get("name").and_then(Value::as_str) {
                    if !name.is_empty() {
                        entry.name = name.to_string();
                    }
                }
                if let Some(args) = fc.get("arguments").and_then(Value::as_str) {
                    entry.arguments.push_str(args);
                }
            }
        }
    }

    fn finalize_chat_stream(acc: ChatStreamAccumulator) -> LLMResponse {
        let tool_calls: Vec<ToolCall> = acc
            .tool_calls
            .into_iter()
            .map(|(_idx, t)| ToolCall {
                id: t.id,
                call_type: "function".into(),
                function: FunctionCall {
                    name: t.name,
                    arguments: t.arguments,
                },
            })
            .collect();

        let content = if acc.content.is_empty() {
            None
        } else {
            Some(acc.content)
        };

        let thinking = acc.thinking.filter(|s| !s.is_empty());

        let empty_tools = tool_calls.is_empty();

        LLMResponse {
            content,
            thinking,
            tool_calls,
            finish_reason: acc.finish_reason.or_else(|| {
                if empty_tools {
                    Some("stop".into())
                } else {
                    Some("tool_calls".into())
                }
            }),
            usage: acc.usage,
        }
    }

    fn parse_usage_from_completion_json(u: &Value) -> Option<Usage> {
        Some(Usage {
            prompt_tokens: u["prompt_tokens"].as_u64().unwrap_or(0) as u32,
            completion_tokens: u["completion_tokens"].as_u64().unwrap_or(0) as u32,
            total_tokens: u["total_tokens"].as_u64().unwrap_or(0) as u32,
        })
    }

    fn parse_sse_data_payload(acc: &mut ChatStreamAccumulator, payload: &str) -> Result<bool> {
        let payload = payload.trim();
        if payload.is_empty() {
            return Ok(false);
        }

        match payload.trim() {
            "[DONE]" => return Ok(true),
            p if !p.starts_with('{') && !p.starts_with('[') => {
                // OpenAI-compat: stray heartbeats etc.
                return Ok(false);
            }
            p => {
                let chunk: Value = serde_json::from_str(p)?;
                Self::accumulate_chat_completion_chunk(acc, &chunk);
                Ok(false)
            }
        }
    }

    /// Reads `text/event-stream` chunks until `[DONE]` or stream end.
    async fn consume_chat_completion_sse(response: reqwest::Response) -> Result<LLMResponse> {
        let mut stream = response.bytes_stream();
        let mut acc = ChatStreamAccumulator::default();
        let mut line_buf = Vec::<u8>::new();

        while let Some(item) = stream.next().await {
            let chunk = item.map_err(anyhow::Error::from)?;

            line_buf.extend_from_slice(&chunk);

            loop {
                let nl = line_buf.iter().position(|b| *b == b'\n');
                let Some(split_at) = nl else {
                    if line_buf.len() > 4 * 1024 * 1024 {
                        anyhow::bail!("SSE line exceeded buffer limit (possible non-SSE proxy response)");
                    }
                    break;
                };

                let mut raw_line = line_buf[..split_at].to_vec();
                line_buf.drain(..=split_at);

                while raw_line.ends_with(&[b'\r']) {
                    raw_line.pop();
                }

                let line = String::from_utf8(raw_line).map_err(|e| anyhow::anyhow!("invalid UTF-8 in SSE: {e}"))?;

                if line.is_empty() {
                    continue;
                }

                if let Some(rest) = line.strip_prefix(':') {
                    let _comment = rest;
                    continue;
                }

                let Some(rest) = line.strip_prefix("data:") else {
                    continue;
                };
                let data = rest.trim_start();

                if Self::parse_sse_data_payload(&mut acc, data)? {
                    debug!("chat completions stream terminated with [DONE]");
                    return Ok(Self::finalize_chat_stream(acc));
                }
            }
        }

        if !acc.tool_calls.is_empty() || acc.finish_reason.is_some() || !acc.content.is_empty() {
            debug!("chat completions stream closed without explicit [DONE]; finalizing aggregated delta");
            return Ok(Self::finalize_chat_stream(acc));
        }

        anyhow::bail!("empty or incomplete chat completion SSE stream")
    }

    fn parse_responses_output(response_json: &Value) -> Result<LLMResponse> {
        let output = response_json
            .get("output")
            .and_then(|o| o.as_array())
            .ok_or_else(|| anyhow::anyhow!("responses payload missing output array"))?;

        let mut text_parts: Vec<String> = Vec::new();
        let mut thinking_parts: Vec<String> = Vec::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();

        for item in output {
            let Some(ty) = item.get("type").and_then(|t| t.as_str()) else {
                continue;
            };
            match ty {
                "message" => {
                    if item.get("role").and_then(|r| r.as_str()) != Some("assistant") {
                        continue;
                    }
                    if let Some(content) = item.get("content").and_then(|c| c.as_array()) {
                        for span in content {
                            extract_message_span(span, &mut text_parts, &mut thinking_parts);
                        }
                    }
                }
                "function_call" => {
                    let id = item
                        .get("call_id")
                        .or_else(|| item.get("id"))
                        .and_then(|id| id.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = item
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string();
                    let arguments = match item.get("arguments") {
                        Some(Value::String(s)) => s.clone(),
                        Some(v) => v.to_string(),
                        None => "{}".into(),
                    };
                    tool_calls.push(ToolCall {
                        id,
                        call_type: "function".into(),
                        function: FunctionCall { name, arguments },
                    });
                }
                "reasoning" => {
                    if let Some(summary) = item.get("summary").and_then(|s| s.as_array()) {
                        for block in summary {
                            if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                                thinking_parts.push(text.to_string());
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        let usage = response_json.get("usage").map(|u| {
            let prompt = u["input_tokens"]
                .as_u64()
                .or_else(|| u["prompt_tokens"].as_u64())
                .unwrap_or(0) as u32;
            let completion = u["output_tokens"]
                .as_u64()
                .or_else(|| u["completion_tokens"].as_u64())
                .unwrap_or(0) as u32;
            let total = u["total_tokens"].as_u64().unwrap_or(0) as u32;
            Usage {
                prompt_tokens: prompt,
                completion_tokens: completion,
                total_tokens: total,
            }
        });

        let empty_calls = tool_calls.is_empty();

        Ok(LLMResponse {
            content: if text_parts.is_empty() {
                None
            } else {
                Some(text_parts.join("\n"))
            },
            thinking: if thinking_parts.is_empty() {
                None
            } else {
                Some(thinking_parts.join("\n"))
            },
            tool_calls,
            finish_reason: if empty_calls {
                Some("stop".into())
            } else {
                Some("tool_calls".into())
            },
            usage,
        })
    }

    fn assert_no_chat_hosted_tools(tools: &[ToolSpec]) -> Result<()> {
        if Self::uses_hosted_tools(tools) {
            anyhow::bail!(
                "internal error: Chat Completions request included hosted tools; enable responses_api path"
            );
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn build_chat_completion_payload_for_test_streaming(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
    ) -> Value {
        Self::assert_no_chat_hosted_tools(tools).unwrap();
        self.chat_completion_payload(messages, tools, true)
    }

    #[cfg(test)]
    pub(crate) fn aggregate_stream_chunks_for_test(chunks: &[&str]) -> LLMResponse {
        let mut acc = ChatStreamAccumulator::default();
        for c in chunks {
            let chunk: Value = serde_json::from_str(c).expect("chunk json");
            Self::accumulate_chat_completion_chunk(&mut acc, &chunk);
        }
        Self::finalize_chat_stream(acc)
    }

    #[cfg(test)]
    pub(crate) fn build_chat_completion_body_for_test(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
    ) -> Value {
        Self::assert_no_chat_hosted_tools(tools).unwrap();
        self.chat_completion_payload(messages, tools, false)
    }

    #[cfg(test)]
    pub(crate) fn responses_body_for_test(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
    ) -> Result<Value> {
        self.responses_body(messages, tools)
    }

    #[cfg(test)]
    pub(crate) fn parse_responses_body_for_test(v: &Value) -> Result<LLMResponse> {
        Self::parse_responses_output(v)
    }
}

fn responses_user_message(text: &str) -> Value {
    json!({
        "type": "message",
        "role": "user",
        "content": [{ "type": "input_text", "text": text }],
    })
}

fn responses_assistant_message(text: &str) -> Value {
    json!({
        "type": "message",
        "role": "assistant",
        "content": [{ "type": "output_text", "text": text }],
    })
}

fn extract_message_span(span: &Value, text: &mut Vec<String>, thinking: &mut Vec<String>) {
    let Some(sp_type) = span.get("type").and_then(|t| t.as_str()) else {
        return;
    };
    match sp_type {
        "output_text" | "text" => {
            if let Some(t) = span.get("text").and_then(|s| s.as_str()) {
                text.push(t.to_string());
            }
        }
        "input_text" => {
            if let Some(t) = span.get("text").and_then(|s| s.as_str()) {
                text.push(t.to_string());
            }
        }
        "reasoning_text" => {
            if let Some(t) = span.get("text").and_then(|s| s.as_str()) {
                thinking.push(t.to_string());
            }
        }
        _ => {}
    }
}

#[async_trait]
impl LLMProvider for OpenAIProvider {
    async fn chat(&self, messages: &[Message], tools: &[ToolSpec]) -> Result<LLMResponse> {
        debug!(provider = "openai", model = %self.model, responses_api = self.responses_api, "sending chat request");

        let use_responses = self.responses_api && Self::uses_hosted_tools(tools);
        if self.responses_api && !Self::uses_hosted_tools(tools) {
            warn!(
                "OpenAI Responses mode was configured for hosted tools but none are present — using Chat Completions"
            );
        }

        if use_responses {
            let body = self.responses_body(messages, tools)?;
            let url = self.responses_url();

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

                    let response_json: Value = serde_json::from_str(&response_text)?;
                    Ok(response_json)
                }
            };

            let response_json = crate::retry::with_retry(&self.retry_config, make_request)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;

            Ok(Self::parse_responses_output(&response_json)?)
        } else {
            Self::assert_no_chat_hosted_tools(tools)?;
            let body = self.chat_completion_payload(messages, tools, /* stream */ true);
            let url = self.chat_completions_url();

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
                        .header("Accept", "text/event-stream")
                        .json(&body)
                        .send()
                        .await?;

                    let status = resp.status();

                    if !status.is_success() {
                        let err_body = resp.text().await.unwrap_or_default();
                        anyhow::bail!("OpenAI API error ({}): {}", status, err_body);
                    }

                    Self::consume_chat_completion_sse(resp).await
                }
            };

            Ok(crate::retry::with_retry(&self.retry_config, make_request)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?)
        }
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
    use ragent_types::tool::{
        ToolSpec, WebSearchContextSize, WebSearchFilters, WebSearchUserLocation,
        WebSearchUserLocationType,
    };

    #[test]
    fn build_chat_completion_without_tools() {
        let provider = OpenAIProvider::new(
            "test-key".into(),
            None,
            "gpt-4o".into(),
            RetryConfig::default(),
            false,
        );
        let messages = vec![Message::system("You are helpful."), Message::user("Hello")];
        let body = provider.build_chat_completion_body_for_test(&messages, &[]);

        assert_eq!(body["model"], "gpt-4o");
        assert_eq!(body["messages"].as_array().unwrap().len(), 2);
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn build_chat_completion_with_function_tools_only() {
        let provider = OpenAIProvider::new(
            "test-key".into(),
            None,
            "gpt-4o".into(),
            RetryConfig::default(),
            false,
        );
        let messages = vec![Message::user("read my file")];
        let tools = vec![ToolSpec::function(
            "read_file",
            "Read a file",
            serde_json::json!({"type": "object", "properties": {}}),
        )];
        let body = provider.build_chat_completion_body_for_test(&messages, &tools);

        assert_eq!(body["tools"].as_array().unwrap().len(), 1);
        assert_eq!(body["tools"][0]["function"]["name"], "read_file");
    }

    #[test]
    fn streaming_request_body_sets_stream_true() {
        let provider = OpenAIProvider::new(
            "test-key".into(),
            None,
            "gpt-4o".into(),
            RetryConfig::default(),
            false,
        );
        let messages = vec![Message::user("hi")];
        let body = provider.build_chat_completion_payload_for_test_streaming(&messages, &[]);
        assert_eq!(body["stream"], serde_json::json!(true));
        assert_eq!(
            body["stream_options"],
            serde_json::json!({ "include_usage": true })
        );
    }

    #[test]
    fn stream_chunks_aggregate_content_and_finish() {
        let out = OpenAIProvider::aggregate_stream_chunks_for_test(&[
            r#"{"choices":[{"index":0,"delta":{"role":"assistant","content":"Hel"}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"content":"lo"}}]}"#,
            r#"{"choices":[{"index":0,"finish_reason":"stop"}]}"#,
        ]);
        assert_eq!(out.content.as_deref(), Some("Hello"));
        assert!(out.tool_calls.is_empty());
        assert_eq!(out.finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn stream_chunks_aggregate_split_tool_calls() {
        let out = OpenAIProvider::aggregate_stream_chunks_for_test(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-x","type":"function","function":{"name":"bash","arguments":""}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"command\":\""}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"pwd\"}"}}]}}]}"#,
            r#"{"choices":[{"finish_reason":"tool_calls"}]}"#,
        ]);
        assert_eq!(out.tool_calls.len(), 1);
        assert_eq!(out.tool_calls[0].id, "call-x");
        assert_eq!(out.tool_calls[0].function.name, "bash");
        assert_eq!(out.tool_calls[0].function.arguments, r#"{"command":"pwd"}"#);
        assert_eq!(out.finish_reason.as_deref(), Some("tool_calls"));
    }



    #[test]
    fn responses_body_includes_web_search_tool() -> Result<()> {
        let provider = OpenAIProvider::new(
            "k".into(),
            None,
            "gpt-test".into(),
            RetryConfig::default(),
            true,
        );
        let msgs = vec![Message::system("SYS"), Message::user("hi")];
        let tools = vec![
            ToolSpec::function(
                "bash",
                "run",
                serde_json::json!({"type": "object"}),
            ),
            ToolSpec::hosted_web_search(
                true,
                Some(WebSearchContextSize::High),
                Some(WebSearchFilters {
                    allowed_domains: Some(vec!["example.com".into()]),
                }),
                Some(WebSearchUserLocation {
                    location_type: WebSearchUserLocationType::Approximate,
                    country: Some("US".into()),
                    region: None,
                    city: None,
                    timezone: None,
                }),
            ),
        ];
        let body = provider.responses_body_for_test(&msgs, &tools)?;
        assert_eq!(body["model"], "gpt-test");
        let arr = body["tools"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[1]["type"], "web_search");

        assert_eq!(
            arr[1],
            serde_json::json!({
                "type": "web_search",
                "external_web_access": true,
                "search_context_size": "high",
                "filters": { "allowed_domains": ["example.com"]},
                "user_location": {"type":"approximate","country":"US"}
            })
        );

        Ok(())
    }

    #[test]
    fn parse_response_content_only() {
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

        let response = OpenAIProvider::parse_chat_completion_response(&json).unwrap();
        assert_eq!(response.content.as_deref(), Some("Hello!"));
        assert!(response.tool_calls.is_empty());
        assert_eq!(response.usage.unwrap().total_tokens, 15);
    }

    #[test]
    fn parse_response_with_tool_calls() {
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

        let response = OpenAIProvider::parse_chat_completion_response(&json).unwrap();
        assert!(response.content.is_none());
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].function.name, "bash");
        assert_eq!(response.finish_reason.as_deref(), Some("tool_calls"));
    }

    #[test]
    fn parse_responses_roundtrip_shapes() -> Result<()> {
        let resp = serde_json::json!({
            "output": [
                {
                  "type": "message",
                  "role": "assistant",
                  "content": [{"type": "output_text", "text": "done"}],
                },
                {
                   "type": "function_call",
                   "call_id": "call_1",
                   "name": "bash",
                   "arguments": "{\"command\":\"pwd\"}",
                },
            ],
            "usage": { "input_tokens": 1u64, "output_tokens": 2u64, "total_tokens": 3u64 }
        });
        let r = OpenAIProvider::parse_responses_body_for_test(&resp)?;
        assert_eq!(r.content.as_deref(), Some("done"));
        assert_eq!(r.tool_calls.len(), 1);
        assert_eq!(r.tool_calls[0].function.name, "bash");
        assert_eq!(r.finish_reason.as_deref(), Some("tool_calls"));

        Ok(())
    }
}
