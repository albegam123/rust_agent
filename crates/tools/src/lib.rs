pub mod bash;
pub mod file_edit;
pub mod file_read;
pub mod file_write;
pub mod mcp;
pub mod note;

use ragent_traits::tool::Tool;
use ragent_types::config::ToolsConfig;

/// Factory function to create tools based on config.
/// Borrowed from zeroclaw: config-driven tool registration.
pub fn create_tools(config: &ToolsConfig) -> Vec<Box<dyn Tool>> {
    let mut tools: Vec<Box<dyn Tool>> = Vec::new();

    if config.enable_file_tools {
        tools.push(Box::new(file_read::FileReadTool));
        tools.push(Box::new(file_write::FileWriteTool));
        tools.push(Box::new(file_edit::FileEditTool));
    }

    if config.enable_bash {
        tools.push(Box::new(bash::BashTool::new()));
    }

    if config.enable_note {
        tools.push(Box::new(note::NoteTool::new()));
    }

    tools
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_tools_default_config() {
        let config = ToolsConfig::default();
        let tools = create_tools(&config);
        let names: Vec<String> = tools
            .iter()
            .filter_map(|t| t.spec().callable_name().map(ToString::to_string))
            .collect();
        assert!(names.iter().any(|n| n == "read_file"));
        assert!(names.iter().any(|n| n == "write_file"));
        assert!(names.iter().any(|n| n == "edit_file"));
        assert!(names.iter().any(|n| n == "bash"));
        assert!(names.iter().any(|n| n == "note"));
    }

    #[test]
    fn create_tools_bash_disabled() {
        let config = ToolsConfig {
            enable_bash: false,
            ..ToolsConfig::default()
        };
        let tools = create_tools(&config);
        let names: Vec<String> = tools
            .iter()
            .filter_map(|t| t.spec().callable_name().map(ToString::to_string))
            .collect();
        assert!(!names.iter().any(|n| n == "bash"));
        assert!(names.iter().any(|n| n == "read_file"));
    }
}
