//! Session file logs: one file per `Agent::run`, recording LLM requests/responses and tool results.
//! Format aligns with Mini-Agent `AgentLogger` (`~/.mini-agent/log/agent_run_*.log`).

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::{Context, Result};
use chrono::Local;
use ragent_types::message::{LLMResponse, Message};
use ragent_types::tool::{ToolResult, ToolSpec};
use serde::Serialize;

#[derive(Debug, Default)]
struct SessionLogState {
    current_file: Option<PathBuf>,
    index: u64,
}

/// Per-run log file under `log_dir` (default `~/.ragent/log`).
#[derive(Debug)]
pub struct SessionLog {
    enabled: bool,
    dir: PathBuf,
    state: Mutex<SessionLogState>,
}

impl SessionLog {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            dir: PathBuf::new(),
            state: Mutex::new(SessionLogState::default()),
        }
    }

    /// When `session_log` is false, returns a no-op logger.
    pub fn from_config(session_log: bool, log_dir: Option<&PathBuf>) -> Result<Self> {
        if !session_log {
            return Ok(Self::disabled());
        }
        let dir = if let Some(p) = log_dir {
            p.clone()
        } else {
            let base = directories::BaseDirs::new().context(
                "cannot resolve home directory; set logging.log_dir in config for session logs",
            )?;
            base.home_dir().join(".ragent").join("log")
        };
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create log directory {}", dir.display()))?;
        Ok(Self {
            enabled: true,
            dir,
            state: Mutex::new(SessionLogState::default()),
        })
    }

    /// New log file for this user turn. Returns the path for UI (like Mini-Agent).
    pub fn start_session(&self) -> Result<Option<PathBuf>> {
        if !self.enabled {
            return Ok(None);
        }
        let timestamp = Local::now().format("%Y%m%d_%H%M%S");
        let path = self.dir.join(format!("agent_run_{timestamp}.log"));

        {
            let mut f = std::fs::File::create(&path)
                .with_context(|| format!("failed to create {}", path.display()))?;
            writeln!(f, "{}", "=".repeat(80))?;
            writeln!(
                f,
                "Agent Run Log - {}",
                Local::now().format("%Y-%m-%d %H:%M:%S")
            )?;
            writeln!(f, "{}", "=".repeat(80))?;
            writeln!(f)?;
        }

        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.current_file = Some(path.clone());
        state.index = 0;
        Ok(Some(path))
    }

    pub fn log_request(&self, messages: &[Message], tool_specs: &[ToolSpec]) {
        if !self.enabled {
            return;
        }
        #[derive(Serialize)]
        struct Req {
            messages: Vec<serde_json::Value>,
            tools: Vec<serde_json::Value>,
        }
        let msg_values: Vec<serde_json::Value> = messages
            .iter()
            .filter_map(|m| serde_json::to_value(m).ok())
            .collect();
        let tools_serial: Vec<serde_json::Value> = tool_specs
            .iter()
            .filter_map(|t| serde_json::to_value(t).ok())
            .collect();
        let body = Req {
            messages: msg_values,
            tools: tools_serial,
        };
        let json = serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".to_string());
        let content = format!("LLM Request:\n\n{json}");
        self.write_block("REQUEST", &content);
    }

    pub fn log_response(&self, response: &LLMResponse) {
        if !self.enabled {
            return;
        }
        let json = serde_json::to_string_pretty(response).unwrap_or_else(|_| "{}".to_string());
        let content = format!("LLM Response:\n\n{json}");
        self.write_block("RESPONSE", &content);
    }

    /// Logged when the LLM HTTP call fails (no parsed response body).
    pub fn log_llm_error(&self, message: &str) {
        if !self.enabled {
            return;
        }
        let v = serde_json::json!({ "error": message });
        let json = serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".to_string());
        let content = format!("LLM Response (error):\n\n{json}");
        self.write_block("RESPONSE", &content);
    }

    pub fn log_tool_result(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
        result: &ToolResult,
    ) {
        if !self.enabled {
            return;
        }
        let mut obj = serde_json::json!({
            "tool_name": tool_name,
            "arguments": arguments,
            "success": result.success,
        });
        if result.success {
            obj["result"] = serde_json::Value::String(result.output.clone());
        } else {
            obj["error"] = serde_json::Value::String(result.error.clone().unwrap_or_default());
        }
        let json = serde_json::to_string_pretty(&obj).unwrap_or_else(|_| "{}".to_string());
        let content = format!("Tool Execution:\n\n{json}");
        self.write_block("TOOL_RESULT", &content);
    }

    fn write_block(&self, log_type: &str, content: &str) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let Some(path) = state.current_file.clone() else {
            return;
        };
        state.index += 1;
        let idx = state.index;
        drop(state);

        let now = Local::now();
        let ts = format!(
            "{}.{:03}",
            now.format("%Y-%m-%d %H:%M:%S"),
            now.timestamp_subsec_millis()
        );

        let mut f = match OpenOptions::new().create(true).append(true).open(&path) {
            Ok(f) => f,
            Err(_) => return,
        };
        let _ = writeln!(f);
        let _ = writeln!(f, "{}", "-".repeat(80));
        let _ = writeln!(f, "[{idx}] {log_type}");
        let _ = writeln!(f, "Timestamp: {ts}");
        let _ = writeln!(f, "{}", "-".repeat(80));
        let _ = writeln!(f, "{content}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ragent_types::message::Message;
    use tempfile::tempdir;

    #[test]
    fn disabled_no_file() {
        let log = SessionLog::disabled();
        assert!(log.start_session().unwrap().is_none());
        log.log_request(&[Message::user("hi")], &[]);
    }

    #[test]
    fn session_writes_request_and_response() {
        let dir = tempdir().unwrap();
        let log = SessionLog::from_config(true, Some(&dir.path().to_path_buf())).unwrap();
        let path = log.start_session().unwrap().unwrap();

        log.log_request(&[Message::user("hello")], &[]);
        log.log_response(&LLMResponse {
            content: Some("hi".into()),
            thinking: None,
            tool_calls: vec![],
            finish_reason: Some("end_turn".into()),
            usage: None,
        });

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("LLM Request:"));
        assert!(text.contains("hello"));
        assert!(text.contains("LLM Response:"));
        assert!(text.contains("hi"));
        assert!(text.contains("[1] REQUEST"));
        assert!(text.contains("[2] RESPONSE"));
    }
}
