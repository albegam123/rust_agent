pub mod anthropic;
pub mod openai;
pub mod retry;

use anyhow::{bail, Result};
use ragent_traits::llm::LLMProvider;
use ragent_types::config::LLMConfig;

/// Factory function to create an LLM provider from config.
/// Borrowed from zeroclaw: centralized factory pattern.
/// `enable_native_web_search_tools` activates OpenAI Responses (`/v1/responses`) when the tool list
/// includes hosted `web_search` (Codex-aligned). Ignored unless `provider == "openai"`.
pub fn create_provider(
    config: &LLMConfig,
    enable_native_web_search_tools: bool,
) -> Result<Box<dyn LLMProvider>> {
    let api_key = resolve_api_key(config)?;
    let api_base = config.api_base.clone();

    let openai_responses_mode =
        matches!(config.provider.as_str(), "openai") && enable_native_web_search_tools;

    if enable_native_web_search_tools && config.provider.as_str() != "openai" {
        tracing::warn!(
            provider = config.provider.as_str(),
            "`tools.web_search` is implemented via OpenAI `/v1/responses` hosted tools — other providers skip it."
        );
    }

    match config.provider.as_str() {
        "openai" => Ok(Box::new(openai::OpenAIProvider::new(
            api_key,
            api_base,
            config.model.clone(),
            config.retry.clone(),
            openai_responses_mode,
        ))),
        "anthropic" => Ok(Box::new(anthropic::AnthropicProvider::new(
            api_key,
            api_base,
            config.model.clone(),
            config.retry.clone(),
        ))),
        other => bail!("unsupported LLM provider: {other}"),
    }
}

fn resolve_api_key(config: &LLMConfig) -> Result<String> {
    if let Some(key) = &config.api_key {
        return Ok(key.clone());
    }
    if let Ok(key) = std::env::var("RAGENT_API_KEY") {
        return Ok(key);
    }
    if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        return Ok(key);
    }
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        return Ok(key);
    }
    bail!("no API key found: set api_key in config, or RAGENT_API_KEY / OPENAI_API_KEY / ANTHROPIC_API_KEY env var")
}
