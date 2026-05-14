use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;
use ragent_traits::tool::Tool;
use ragent_types::tool::{ToolResult, ToolSpec};

use crate::skill_loader::SkillLoader;

/// Tool: load full skill body on demand (progressive disclosure level 2).
pub struct GetSkillTool {
    loader: Arc<SkillLoader>,
}

impl GetSkillTool {
    pub fn new(loader: Arc<SkillLoader>) -> Self {
        Self { loader }
    }
}

#[async_trait]
impl Tool for GetSkillTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "get_skill".into(),
            description: "Get complete content and guidance for a specified skill, used for executing specific types of tasks".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "skill_name": {
                        "type": "string",
                        "description": "Name of the skill to retrieve (see Available Skills in the system prompt)"
                    }
                },
                "required": ["skill_name"]
            }),
        }
    }

    async fn call(&self, args: serde_json::Value) -> Result<ToolResult> {
        let skill_name = args
            .get("skill_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing required argument: skill_name"))?;

        match self.loader.load_formatted(skill_name)? {
            Some(text) => Ok(ToolResult::success(text)),
            None => {
                let names: Vec<String> = self
                    .loader
                    .discover()?
                    .into_iter()
                    .map(|s| s.name)
                    .collect();
                let available = names.join(", ");
                Ok(ToolResult::failure(format!(
                    "Skill '{skill_name}' does not exist. Available skills: {available}"
                )))
            }
        }
    }
}
