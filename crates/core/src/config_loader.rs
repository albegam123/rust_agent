use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ragent_types::config::AgentConfig;
use tracing::info;

/// Config search paths, in priority order.
fn config_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    paths.push(PathBuf::from("config/config.toml"));

    if let Some(home) = dirs_home() {
        paths.push(home.join(".ragent").join("config.toml"));
    }

    paths
}

fn dirs_home() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf())
}

/// Load config from the first found config file.
pub fn load_config(explicit_path: Option<&Path>) -> Result<AgentConfig> {
    if let Some(path) = explicit_path {
        return load_config_from(path);
    }

    for path in config_search_paths() {
        if path.exists() {
            return load_config_from(&path);
        }
    }

    anyhow::bail!(
        "no config file found. Searched: {:?}. \
         Copy config/config.example.toml to config/config.toml to get started.",
        config_search_paths()
    )
}

fn load_config_from(path: &Path) -> Result<AgentConfig> {
    info!(path = %path.display(), "loading config");
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config: {}", path.display()))?;
    let mut config: AgentConfig = toml::from_str(&content)
        .with_context(|| format!("failed to parse config: {}", path.display()))?;

    if config.llm.api_key.is_none() {
        config.llm.api_key = std::env::var("RAGENT_API_KEY")
            .or_else(|_| std::env::var("OPENAI_API_KEY"))
            .or_else(|_| std::env::var("ANTHROPIC_API_KEY"))
            .ok();
    }

    Ok(config)
}

/// Load system prompt from file path or return default.
pub fn load_system_prompt(path: Option<&Path>) -> Result<String> {
    if let Some(p) = path {
        let content = std::fs::read_to_string(p)
            .with_context(|| format!("failed to read system prompt: {}", p.display()))?;
        return Ok(content);
    }

    for candidate in &[PathBuf::from("config/system_prompt.md")] {
        if candidate.exists() {
            let content = std::fs::read_to_string(candidate)?;
            return Ok(content);
        }
    }

    Ok(default_system_prompt().to_string())
}

fn default_system_prompt() -> &'static str {
    "You are a helpful AI coding assistant. You have access to tools that let you \
     read files, write files, edit files, and run bash commands.\n\n\
     When helping the user:\n\
     1. Read relevant files first to understand context before making changes.\n\
     2. Make precise, targeted edits rather than rewriting entire files.\n\
     3. Run commands to verify your changes work correctly.\n\
     4. Explain what you're doing and why."
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_system_prompt_not_empty() {
        let prompt = default_system_prompt();
        assert!(!prompt.is_empty());
        assert!(prompt.contains("helpful"));
    }

    #[test]
    fn config_search_paths_not_empty() {
        let paths = config_search_paths();
        assert!(!paths.is_empty());
    }

    #[test]
    fn load_config_from_string() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_config.toml");
        std::fs::write(
            &path,
            r#"
[llm]
model = "test-model"
provider = "openai"
"#,
        )
        .unwrap();

        let config = load_config_from(&path).unwrap();
        assert_eq!(config.llm.model, "test-model");
        assert_eq!(config.llm.provider, "openai");
    }
}
