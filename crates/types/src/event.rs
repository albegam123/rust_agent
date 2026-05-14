use crate::message::LLMResponse;
use crate::tool::ToolResult;

/// Events emitted by the agent loop for UI consumption.
/// Borrowed from codex-rs: decouples agent logic from presentation.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    TurnStarted {
        step: u32,
    },
    Thinking {
        content: String,
    },
    Response {
        response: LLMResponse,
    },
    ToolCallStart {
        call_id: String,
        name: String,
        arguments: String,
    },
    ToolCallComplete {
        call_id: String,
        name: String,
        result: ToolResult,
    },
    ContextSummarized {
        original_messages: usize,
        new_messages: usize,
    },
    TurnComplete {
        content: Option<String>,
        total_steps: u32,
    },
    Error {
        message: String,
    },
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_construction() {
        let event = AgentEvent::ToolCallStart {
            call_id: "call-1".into(),
            name: "bash".into(),
            arguments: r#"{"command":"ls"}"#.into(),
        };
        match event {
            AgentEvent::ToolCallStart { call_id, name, .. } => {
                assert_eq!(call_id, "call-1");
                assert_eq!(name, "bash");
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn event_turn_complete() {
        let event = AgentEvent::TurnComplete {
            content: Some("Done!".into()),
            total_steps: 3,
        };
        match event {
            AgentEvent::TurnComplete { content, total_steps } => {
                assert_eq!(content.as_deref(), Some("Done!"));
                assert_eq!(total_steps, 3);
            }
            _ => panic!("unexpected variant"),
        }
    }
}
