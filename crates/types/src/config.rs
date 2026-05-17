use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::tool::{WebSearchContextSize, WebSearchFilters, WebSearchUserLocation};

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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum WebSearchMode {
    #[default]
    Disabled,
    /// Cached index (`external_web_access = false`).
    Cached,
    Live,
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
    /// Mode: use `web_search_mode` in config to avoid clashing with `[tools.web_search]` (options).
    #[serde(rename = "web_search_mode")]
    pub web_search_mode: WebSearchMode,
    #[serde(default)]
    pub web_search: Option<WebSearchToolOptions>,
}

/// Optional tuning forwarded to OpenAI Responses `tools[]` (`type = "web_search"`), codex-compatible.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebSearchToolOptions {
    #[serde(rename = "context_size")]
    pub search_context_size: Option<WebSearchContextSize>,
    pub allowed_domains: Option<Vec<String>>,
    pub location: Option<WebSearchUserLocationToml>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSearchUserLocationToml {
    pub country: Option<String>,
    pub region: Option<String>,
    pub city: Option<String>,
    pub timezone: Option<String>,
}

impl ToolsConfig {
    /// Codex-aligned hosted `web_search` tool specs for `/v1/responses` when using OpenAI.
    pub fn hosted_web_search_tool_specs(&self) -> Vec<crate::tool::ToolSpec> {
        let external_web_access = match self.web_search_mode {
            WebSearchMode::Disabled => return Vec::new(),
            WebSearchMode::Cached => false,
            WebSearchMode::Live => true,
        };

        let filters = self
            .web_search
            .as_ref()
            .and_then(|o| o.allowed_domains.as_ref())
            .map(|domains| WebSearchFilters {
                allowed_domains: Some(domains.clone()),
            });

        let user_location = self.web_search.as_ref().and_then(|o| {
            o.location.as_ref().map(|loc| WebSearchUserLocation {
                location_type: crate::tool::WebSearchUserLocationType::Approximate,
                country: loc.country.clone(),
                region: loc.region.clone(),
                city: loc.city.clone(),
                timezone: loc.timezone.clone(),
            })
        });

        vec![crate::tool::ToolSpec::hosted_web_search(
            external_web_access,
            self.web_search.as_ref().and_then(|o| o.search_context_size),
            filters,
            user_location,
        )]
    }
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
            web_search_mode: WebSearchMode::Disabled,
            web_search: None,
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
    fn tools_web_search_toml_hosts_spec() {
        let toml_str = r#"
[tools]
web_search_mode = "live"

[tools.web_search]
context_size = "high"
allowed_domains = ["example.com"]

[tools.web_search.location]
country = "US"
city = "New York"
timezone = "America/New_York"

[llm]
model = "gpt-5"
"#;
        let cfg: AgentConfig = toml::from_str(toml_str).unwrap();
        let specs = cfg.tools.hosted_web_search_tool_specs();
        assert_eq!(specs.len(), 1);
        match &specs[0] {
            crate::tool::ToolSpec::WebSearch {
                external_web_access,
                search_context_size,
                filters,
                user_location,
            } => {
                assert!(*external_web_access);
                assert_eq!(
                    *search_context_size,
                    Some(crate::tool::WebSearchContextSize::High)
                );
                assert_eq!(
                    filters.as_ref().unwrap().allowed_domains.as_ref().unwrap()[0],
                    "example.com"
                );
                let ul = user_location.as_ref().unwrap();
                assert_eq!(ul.country.as_deref(), Some("US"));
                assert_eq!(ul.city.as_deref(), Some("New York"));
            }
            _ => panic!("expected WebSearch variant"),
        }
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
