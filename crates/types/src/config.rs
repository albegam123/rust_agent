use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub llm: LLMConfig,
    #[serde(default)]
    pub agent: AgentLoopConfig,
    #[serde(default)]
    pub tools: ToolsConfig,
    #[serde(default)]
    pub skills: SkillsConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMConfig {
    pub api_key: Option<String>,
    pub api_base: Option<String>,
    pub model: String,
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default)]
    pub retry: RetryConfig,
}

fn default_provider() -> String {
    "openai".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RetryConfig {
    pub enabled: bool,
    pub max_retries: u32,
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_retries: 3,
            initial_delay_ms: 1000,
            max_delay_ms: 30000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentLoopConfig {
    pub max_steps: u32,
    pub max_context_tokens: u32,
    pub system_prompt_path: Option<PathBuf>,
    pub workspace_dir: Option<PathBuf>,
}

impl Default for AgentLoopConfig {
    fn default() -> Self {
        Self {
            max_steps: 50,
            max_context_tokens: 100_000,
            system_prompt_path: None,
            workspace_dir: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolsConfig {
    pub enable_file_tools: bool,
    pub enable_bash: bool,
    pub enable_note: bool,
    pub enable_mcp: bool,
    pub mcp_config_path: Option<PathBuf>,
    pub mcp: McpConfig,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            enable_file_tools: true,
            enable_bash: true,
            enable_note: true,
            enable_mcp: false,
            mcp_config_path: None,
            mcp: McpConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct McpConfig {
    pub connect_timeout_secs: u64,
    pub execute_timeout_secs: u64,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            connect_timeout_secs: 10,
            execute_timeout_secs: 60,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SkillsConfig {
    pub enabled: bool,
    pub skills_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub level: String,
    /// When true, each `Agent::run` creates a session log file with LLM I/O and tool results.
    pub session_log: bool,
    /// Directory for session logs. If unset, uses `~/.ragent/log` when `session_log` is true.
    pub log_dir: Option<PathBuf>,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            session_log: true,
            log_dir: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn config_deserialize_minimal() {
        let toml_str = r#"
[llm]
model = "gpt-4o"
"#;
        let config: AgentConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.llm.model, "gpt-4o");
        assert_eq!(config.llm.provider, "openai");
        assert_eq!(config.agent.max_steps, 50);
        assert!(config.tools.enable_bash);
        assert!(config.tools.enable_file_tools);
    }

    #[test]
    fn config_deserialize_full() {
        let toml_str = r#"
[llm]
api_key = "test-key"
api_base = "https://api.example.com"
model = "claude-3"
provider = "anthropic"

[llm.retry]
enabled = false
max_retries = 5

[agent]
max_steps = 100
max_context_tokens = 200000

[tools]
enable_file_tools = true
enable_bash = false
enable_note = true

[skills]
enabled = true
skills_dir = "./my-skills"

[logging]
level = "debug"
"#;
        let config: AgentConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.llm.provider, "anthropic");
        assert_eq!(config.llm.api_key.as_deref(), Some("test-key"));
        assert!(!config.llm.retry.enabled);
        assert_eq!(config.llm.retry.max_retries, 5);
        assert_eq!(config.agent.max_steps, 100);
        assert!(!config.tools.enable_bash);
        assert!(config.skills.enabled);
    }

    #[test]
    fn retry_config_defaults() {
        let config = RetryConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.initial_delay_ms, 1000);
        assert_eq!(config.max_delay_ms, 30000);
    }
}
