use anyhow::Result;
use async_trait::async_trait;
use ragent_traits::tool::Tool;
use ragent_types::tool::{ToolResult, ToolSpec};
use std::time::Duration;
use tokio::process::Command;
use tracing::debug;

const DEFAULT_TIMEOUT_SECS: u64 = 120;
const MAX_OUTPUT_BYTES: usize = 100_000;

pub struct BashTool {
    timeout: Duration,
}

impl BashTool {
    pub fn new() -> Self {
        Self {
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
        }
    }

    pub fn with_timeout(timeout: Duration) -> Self {
        Self { timeout }
    }
}

impl Default for BashTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for BashTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "bash".into(),
            description: "Execute a bash command and return its output. \
                          Commands run with a timeout. Long-running commands \
                          will be terminated after the timeout period."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The bash command to execute"
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": "Optional timeout in seconds (default: 120)"
                    }
                },
                "required": ["command"]
            }),
        }
    }

    async fn call(&self, args: serde_json::Value) -> Result<ToolResult> {
        let command = args["command"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: command"))?;
        let timeout = args["timeout_secs"]
            .as_u64()
            .map(Duration::from_secs)
            .unwrap_or(self.timeout);

        debug!(command, timeout_secs = timeout.as_secs(), "executing bash command");

        let result = tokio::time::timeout(timeout, async {
            Command::new("bash")
                .arg("-c")
                .arg(command)
                .output()
                .await
        })
        .await;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let exit_code = output.status.code().unwrap_or(-1);

                let mut combined = String::new();
                if !stdout.is_empty() {
                    combined.push_str(&stdout);
                }
                if !stderr.is_empty() {
                    if !combined.is_empty() {
                        combined.push('\n');
                    }
                    combined.push_str("[stderr]\n");
                    combined.push_str(&stderr);
                }

                if combined.len() > MAX_OUTPUT_BYTES {
                    combined.truncate(MAX_OUTPUT_BYTES);
                    combined.push_str("\n... [output truncated]");
                }

                if exit_code == 0 {
                    Ok(ToolResult::success(combined))
                } else {
                    Ok(ToolResult::failure(format!(
                        "exit code {exit_code}\n{combined}"
                    )))
                }
            }
            Ok(Err(e)) => Ok(ToolResult::failure(format!(
                "failed to execute command: {e}"
            ))),
            Err(_) => Ok(ToolResult::failure(format!(
                "command timed out after {} seconds",
                timeout.as_secs()
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bash_echo() {
        let tool = BashTool::new();
        let result = tool
            .call(serde_json::json!({"command": "echo hello world"}))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("hello world"));
    }

    #[tokio::test]
    async fn bash_exit_code() {
        let tool = BashTool::new();
        let result = tool
            .call(serde_json::json!({"command": "exit 1"}))
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.error.unwrap().contains("exit code 1"));
    }

    #[tokio::test]
    async fn bash_timeout() {
        let tool = BashTool::with_timeout(Duration::from_millis(100));
        let result = tool
            .call(serde_json::json!({"command": "sleep 10"}))
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.error.unwrap().contains("timed out"));
    }

    #[tokio::test]
    async fn bash_stderr() {
        let tool = BashTool::new();
        let result = tool
            .call(serde_json::json!({"command": "echo err >&2 && echo out"}))
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("out"));
        assert!(result.output.contains("[stderr]"));
        assert!(result.output.contains("err"));
    }
}
