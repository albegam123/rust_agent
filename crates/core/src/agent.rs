use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use ragent_traits::llm::LLMProvider;
use ragent_traits::tool::Tool;
use ragent_types::config::AgentLoopConfig;
use ragent_types::event::AgentEvent;
use ragent_types::message::ToolCall;
use ragent_types::tool::{ToolResult, ToolSpec};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};

use crate::context::Context;
use crate::session_log::SessionLog;

/// Core agent: orchestrates LLM calls and tool execution.
/// Design borrowed from codex-rs (event channel pattern)
/// and zeroclaw (trait-based provider/tool injection).
pub struct Agent {
    provider: Box<dyn LLMProvider>,
    tools: HashMap<String, Box<dyn Tool>>,
    tool_specs: Vec<ToolSpec>,
    config: AgentLoopConfig,
    event_tx: mpsc::UnboundedSender<AgentEvent>,
    session_log: Arc<SessionLog>,
}

impl Agent {
    pub fn new(
        provider: Box<dyn LLMProvider>,
        tools: Vec<Box<dyn Tool>>,
        config: AgentLoopConfig,
        event_tx: mpsc::UnboundedSender<AgentEvent>,
        session_log: Arc<SessionLog>,
    ) -> Self {
        let tool_specs: Vec<ToolSpec> =
            tools.iter().map(|t| t.spec()).collect();
        let tools: HashMap<String, Box<dyn Tool>> = tools
            .into_iter()
            .map(|t| (t.spec().name, t))
            .collect();

        Self {
            provider,
            tools,
            tool_specs,
            config,
            event_tx,
            session_log,
        }
    }

    /// Run the agent loop for a single user turn.
    /// Returns the final assistant message content, if any.
    pub async fn run(
        &self,
        user_message: &str,
        context: &mut Context,
        cancel: CancellationToken,
    ) -> Result<Option<String>> {
        context.add_user_message(user_message);

        if let Some(path) = self.session_log.start_session()? {
            info!(log_file = %path.display(), "session log");
        }

        for step in 0..self.config.max_steps {
            if cancel.is_cancelled() {
                self.emit(AgentEvent::Cancelled);
                return Ok(None);
            }

            self.emit(AgentEvent::TurnStarted { step });

            if context.estimate_tokens() > self.config.max_context_tokens {
                self.auto_summarize(context).await?;
            }

            debug!(
                step,
                provider = self.provider.name(),
                model = self.provider.model(),
                "calling LLM"
            );

            self.session_log
                .log_request(context.messages(), &self.tool_specs);

            let response = match self
                .provider
                .chat(context.messages(), &self.tool_specs)
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    self.session_log.log_llm_error(&e.to_string());
                    return Err(e);
                }
            };

            self.session_log.log_response(&response);

            self.emit(AgentEvent::Response {
                response: response.clone(),
            });

            if let Some(thinking) = &response.thinking {
                self.emit(AgentEvent::Thinking {
                    content: thinking.clone(),
                });
            }

            context.add_assistant_message(&response);

            if response.tool_calls.is_empty() {
                info!(step = step + 1, "agent turn complete (no tool calls)");
                self.emit(AgentEvent::TurnComplete {
                    content: response.content.clone(),
                    total_steps: step + 1,
                });
                return Ok(response.content);
            }

            for tool_call in &response.tool_calls {
                if cancel.is_cancelled() {
                    self.emit(AgentEvent::Cancelled);
                    return Ok(None);
                }

                self.emit(AgentEvent::ToolCallStart {
                    call_id: tool_call.id.clone(),
                    name: tool_call.function.name.clone(),
                    arguments: tool_call.function.arguments.clone(),
                });

                let result = self.execute_tool(tool_call).await;

                let args: serde_json::Value =
                    serde_json::from_str(&tool_call.function.arguments).unwrap_or_else(|_| {
                        serde_json::Value::String(tool_call.function.arguments.clone())
                    });
                self.session_log
                    .log_tool_result(&tool_call.function.name, &args, &result);

                self.emit(AgentEvent::ToolCallComplete {
                    call_id: tool_call.id.clone(),
                    name: tool_call.function.name.clone(),
                    result: result.clone(),
                });

                context.add_tool_result(
                    &tool_call.id,
                    &tool_call.function.name,
                    &result,
                );
            }
        }

        info!(max_steps = self.config.max_steps, "agent reached max steps");
        self.emit(AgentEvent::TurnComplete {
            content: None,
            total_steps: self.config.max_steps,
        });
        Ok(None)
    }

    async fn execute_tool(&self, tool_call: &ToolCall) -> ToolResult {
        let tool_name = &tool_call.function.name;

        let Some(tool) = self.tools.get(tool_name) else {
            return ToolResult::failure(format!("unknown tool: {tool_name}"));
        };

        let args = match serde_json::from_str(&tool_call.function.arguments) {
            Ok(v) => v,
            Err(e) => {
                return ToolResult::failure(format!("invalid arguments: {e}"));
            }
        };

        match tool.call(args).await {
            Ok(result) => result,
            Err(e) => {
                error!(tool = tool_name, error = %e, "tool execution failed");
                ToolResult::failure(format!("tool execution error: {e}"))
            }
        }
    }

    async fn auto_summarize(&self, context: &mut Context) -> Result<()> {
        let summary_prompt = "Provide a concise summary of the conversation so far, \
                              focusing on key decisions, findings, and current task state.";
        let summary_messages = vec![
            ragent_types::message::Message::system(
                "You are a summarizer. Summarize the conversation concisely.",
            ),
            ragent_types::message::Message::user(summary_prompt),
        ];

        self.session_log
            .log_request(&summary_messages, &[]);

        let response = match self.provider.chat(&summary_messages, &[]).await {
            Ok(r) => r,
            Err(e) => {
                self.session_log.log_llm_error(&e.to_string());
                return Err(e);
            }
        };

        self.session_log.log_response(&response);

        let summary = response.content.unwrap_or_default();
        let (orig, new) = context.summarize_with(&summary);

        self.emit(AgentEvent::ContextSummarized {
            original_messages: orig,
            new_messages: new,
        });

        Ok(())
    }

    fn emit(&self, event: AgentEvent) {
        if self.event_tx.send(event).is_err() {
            debug!("event receiver dropped");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_log::SessionLog;
    use ragent_types::config::AgentLoopConfig;
    use ragent_types::message::{FunctionCall, LLMResponse, Message};
    use ragent_types::tool::ToolSpec;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    struct MockProvider {
        responses: std::sync::Mutex<Vec<LLMResponse>>,
    }

    impl MockProvider {
        fn new(responses: Vec<LLMResponse>) -> Self {
            Self {
                responses: std::sync::Mutex::new(responses),
            }
        }
    }

    #[async_trait::async_trait]
    impl LLMProvider for MockProvider {
        async fn chat(
            &self,
            _messages: &[Message],
            _tools: &[ToolSpec],
        ) -> Result<LLMResponse> {
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                anyhow::bail!("no more mock responses");
            }
            Ok(responses.remove(0))
        }

        fn name(&self) -> &str {
            "mock"
        }

        fn model(&self) -> &str {
            "mock-model"
        }
    }

    struct CounterTool {
        call_count: Arc<AtomicU32>,
    }

    #[async_trait::async_trait]
    impl Tool for CounterTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "counter".into(),
                description: "test counter".into(),
                parameters: serde_json::json!({"type": "object", "properties": {}}),
            }
        }

        async fn call(&self, _args: serde_json::Value) -> Result<ToolResult> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(ToolResult::success("counted"))
        }
    }

    #[tokio::test]
    async fn agent_simple_response() {
        let provider = MockProvider::new(vec![LLMResponse {
            content: Some("Hello!".into()),
            thinking: None,
            tool_calls: vec![],
            finish_reason: Some("stop".into()),
            usage: None,
        }]);

        let (tx, mut rx) = mpsc::unbounded_channel();
        let agent = Agent::new(
            Box::new(provider),
            vec![],
            AgentLoopConfig::default(),
            tx,
            Arc::new(SessionLog::disabled()),
        );

        let mut ctx = Context::new("system".into());
        let cancel = CancellationToken::new();

        let result = agent.run("hi", &mut ctx, cancel).await.unwrap();
        assert_eq!(result.as_deref(), Some("Hello!"));

        let mut events = vec![];
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        assert!(events.iter().any(|e| matches!(e, AgentEvent::TurnStarted { .. })));
        assert!(events.iter().any(|e| matches!(e, AgentEvent::TurnComplete { .. })));
    }

    #[tokio::test]
    async fn agent_tool_call_then_response() {
        let call_count = Arc::new(AtomicU32::new(0));

        let provider = MockProvider::new(vec![
            LLMResponse {
                content: None,
                thinking: None,
                tool_calls: vec![ToolCall {
                    id: "call-1".into(),
                    call_type: "function".into(),
                    function: FunctionCall {
                        name: "counter".into(),
                        arguments: "{}".into(),
                    },
                }],
                finish_reason: Some("tool_calls".into()),
                usage: None,
            },
            LLMResponse {
                content: Some("Done counting!".into()),
                thinking: None,
                tool_calls: vec![],
                finish_reason: Some("stop".into()),
                usage: None,
            },
        ]);

        let (tx, mut rx) = mpsc::unbounded_channel();
        let tools: Vec<Box<dyn Tool>> = vec![Box::new(CounterTool {
            call_count: call_count.clone(),
        })];
        let agent = Agent::new(
            Box::new(provider),
            tools,
            AgentLoopConfig::default(),
            tx,
            Arc::new(SessionLog::disabled()),
        );

        let mut ctx = Context::new("system".into());
        let cancel = CancellationToken::new();

        let result = agent.run("count", &mut ctx, cancel).await.unwrap();
        assert_eq!(result.as_deref(), Some("Done counting!"));
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        let mut events = vec![];
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        assert!(events.iter().any(|e| matches!(e, AgentEvent::ToolCallStart { .. })));
        assert!(events.iter().any(|e| matches!(e, AgentEvent::ToolCallComplete { .. })));
    }

    #[tokio::test]
    async fn agent_cancellation() {
        let provider = MockProvider::new(vec![]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let agent = Agent::new(
            Box::new(provider),
            vec![],
            AgentLoopConfig::default(),
            tx,
            Arc::new(SessionLog::disabled()),
        );

        let mut ctx = Context::new("system".into());
        let cancel = CancellationToken::new();
        cancel.cancel();

        let result = agent.run("hi", &mut ctx, cancel).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn agent_unknown_tool() {
        let provider = MockProvider::new(vec![
            LLMResponse {
                content: None,
                thinking: None,
                tool_calls: vec![ToolCall {
                    id: "call-1".into(),
                    call_type: "function".into(),
                    function: FunctionCall {
                        name: "nonexistent".into(),
                        arguments: "{}".into(),
                    },
                }],
                finish_reason: None,
                usage: None,
            },
            LLMResponse {
                content: Some("ok".into()),
                thinking: None,
                tool_calls: vec![],
                finish_reason: Some("stop".into()),
                usage: None,
            },
        ]);

        let (tx, mut rx) = mpsc::unbounded_channel();
        let agent = Agent::new(
            Box::new(provider),
            vec![],
            AgentLoopConfig::default(),
            tx,
            Arc::new(SessionLog::disabled()),
        );

        let mut ctx = Context::new("system".into());
        let cancel = CancellationToken::new();

        let _result = agent.run("do something", &mut ctx, cancel).await.unwrap();

        let mut events = vec![];
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        let tool_complete = events.iter().find(|e| matches!(e, AgentEvent::ToolCallComplete { .. }));
        assert!(tool_complete.is_some());
        if let Some(AgentEvent::ToolCallComplete { result, .. }) = tool_complete {
            assert!(!result.success);
            assert!(result.error.as_ref().unwrap().contains("unknown tool"));
        }
    }
}
