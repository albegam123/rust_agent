use anyhow::Result;
use async_trait::async_trait;
use ragent_types::message::{LLMResponse, Message};
use ragent_types::tool::ToolSpec;

/// LLM provider abstraction.
/// Borrowed from zeroclaw: trait-per-subsystem pattern.
/// Each provider (OpenAI, Anthropic, etc.) implements this trait.
#[async_trait]
pub trait LLMProvider: Send + Sync {
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
    ) -> Result<LLMResponse>;

    fn name(&self) -> &str;

    fn model(&self) -> &str;
}
