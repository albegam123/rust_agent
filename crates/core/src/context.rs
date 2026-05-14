use ragent_types::message::{LLMResponse, Message, Role};
use ragent_types::tool::ToolResult;

/// Conversation context: manages message history and token estimation.
pub struct Context {
    messages: Vec<Message>,
    system_prompt: String,
}

impl Context {
    pub fn new(system_prompt: String) -> Self {
        let messages = vec![Message::system(&system_prompt)];
        Self {
            messages,
            system_prompt,
        }
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn system_prompt(&self) -> &str {
        &self.system_prompt
    }

    pub fn add_user_message(&mut self, content: &str) {
        self.messages.push(Message::user(content));
    }

    pub fn add_assistant_message(&mut self, response: &LLMResponse) {
        if response.tool_calls.is_empty() {
            self.messages.push(Message::assistant(
                response.content.as_deref().unwrap_or(""),
            ));
        } else {
            self.messages.push(Message::assistant_with_tool_calls(
                response.content.clone(),
                response.thinking.clone(),
                response.tool_calls.clone(),
            ));
        }
    }

    pub fn add_tool_result(&mut self, call_id: &str, name: &str, result: &ToolResult) {
        let output = if result.success {
            result.output.clone()
        } else {
            format!(
                "Error: {}",
                result.error.as_deref().unwrap_or("unknown error")
            )
        };
        self.messages
            .push(Message::tool_result(call_id, name, output));
    }

    /// Rough token estimation: ~4 chars per token (simple heuristic).
    pub fn estimate_tokens(&self) -> u32 {
        let total_chars: usize = self
            .messages
            .iter()
            .map(|m| {
                let content_len = m.content.as_ref().map_or(0, String::len);
                let thinking_len = m.thinking.as_ref().map_or(0, String::len);
                let tool_calls_len = m.tool_calls.as_ref().map_or(0, |calls| {
                    calls
                        .iter()
                        .map(|c| c.function.arguments.len() + c.function.name.len())
                        .sum()
                });
                content_len + thinking_len + tool_calls_len
            })
            .sum();

        (total_chars / 4) as u32
    }

    pub fn clear(&mut self) {
        self.messages.clear();
        self.messages.push(Message::system(&self.system_prompt));
    }

    /// Summarize conversation when context is too long.
    /// Keeps the system prompt and last user message,
    /// replaces middle messages with a summary.
    pub fn summarize_with(&mut self, summary: &str) -> (usize, usize) {
        let original_count = self.messages.len();

        let last_user_idx = self.messages.iter().rposition(|m| m.role == Role::User);

        let mut new_messages = vec![Message::system(&self.system_prompt)];

        new_messages.push(Message::system(format!(
            "[Previous conversation summary]\n{summary}"
        )));

        if let Some(idx) = last_user_idx {
            for msg in &self.messages[idx..] {
                new_messages.push(msg.clone());
            }
        }

        let new_count = new_messages.len();
        self.messages = new_messages;

        (original_count, new_count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use ragent_types::message::{FunctionCall, ToolCall};

    #[test]
    fn context_new_has_system_message() {
        let ctx = Context::new("You are helpful.".into());
        assert_eq!(ctx.messages().len(), 1);
        assert_eq!(ctx.messages()[0].role, Role::System);
    }

    #[test]
    fn context_add_messages() {
        let mut ctx = Context::new("system".into());
        ctx.add_user_message("hello");
        assert_eq!(ctx.messages().len(), 2);
        assert_eq!(ctx.messages()[1].role, Role::User);

        let response = LLMResponse {
            content: Some("hi there".into()),
            thinking: None,
            tool_calls: vec![],
            finish_reason: Some("stop".into()),
            usage: None,
        };
        ctx.add_assistant_message(&response);
        assert_eq!(ctx.messages().len(), 3);
        assert_eq!(ctx.messages()[2].role, Role::Assistant);
    }

    #[test]
    fn context_add_tool_result() {
        let mut ctx = Context::new("system".into());

        let response = LLMResponse {
            content: None,
            thinking: None,
            tool_calls: vec![ToolCall {
                id: "call-1".into(),
                call_type: "function".into(),
                function: FunctionCall {
                    name: "bash".into(),
                    arguments: "{}".into(),
                },
            }],
            finish_reason: None,
            usage: None,
        };
        ctx.add_assistant_message(&response);

        let result = ToolResult::success("output text");
        ctx.add_tool_result("call-1", "bash", &result);

        assert_eq!(ctx.messages().len(), 3);
        assert_eq!(ctx.messages()[2].role, Role::Tool);
        assert_eq!(ctx.messages()[2].tool_call_id.as_deref(), Some("call-1"));
    }

    #[test]
    fn context_estimate_tokens() {
        let mut ctx = Context::new("short".into());
        ctx.add_user_message("a]".repeat(100).as_str());
        let tokens = ctx.estimate_tokens();
        assert!(tokens > 0);
    }

    #[test]
    fn context_clear_resets() {
        let mut ctx = Context::new("system prompt".into());
        ctx.add_user_message("hello");
        ctx.add_user_message("world");
        assert_eq!(ctx.messages().len(), 3);

        ctx.clear();
        assert_eq!(ctx.messages().len(), 1);
        assert_eq!(ctx.messages()[0].role, Role::System);
    }

    #[test]
    fn context_summarize() {
        let mut ctx = Context::new("system".into());
        ctx.add_user_message("first question");
        let resp = LLMResponse {
            content: Some("first answer".into()),
            thinking: None,
            tool_calls: vec![],
            finish_reason: None,
            usage: None,
        };
        ctx.add_assistant_message(&resp);
        ctx.add_user_message("second question");

        let (orig, new) = ctx.summarize_with("The user asked about setup.");
        assert_eq!(orig, 4);
        assert!(new < orig);
        assert_eq!(ctx.messages()[0].role, Role::System);
        assert!(ctx.messages().iter().any(|m| m.role == Role::User));
    }
}
