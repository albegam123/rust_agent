use anyhow::Result;
use async_trait::async_trait;
use ragent_traits::tool::Tool;
use ragent_types::tool::{ToolResult, ToolSpec};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::debug;

const NOTE_FILE: &str = ".ragent_notes.json";

pub struct NoteTool {
    notes_path: Option<PathBuf>,
}

impl NoteTool {
    pub fn new() -> Self {
        Self { notes_path: None }
    }

    pub fn with_path(path: PathBuf) -> Self {
        Self {
            notes_path: Some(path),
        }
    }

    fn resolve_path(&self) -> PathBuf {
        self.notes_path
            .clone()
            .unwrap_or_else(|| PathBuf::from(NOTE_FILE))
    }
}

impl Default for NoteTool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NoteEntry {
    key: String,
    content: String,
    timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct NotesStore {
    notes: Vec<NoteEntry>,
}

impl NotesStore {
    async fn load(path: &Path) -> Self {
        match tokio::fs::read_to_string(path).await {
            Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    async fn save(&self, path: &Path) -> Result<()> {
        let data = serde_json::to_string_pretty(self)?;
        tokio::fs::write(path, data).await?;
        Ok(())
    }
}

#[async_trait]
impl Tool for NoteTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "note".into(),
            description: "Save or recall session notes. Use action 'save' to store a note \
                          with a key, or 'recall' to retrieve notes matching a query."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["save", "recall"],
                        "description": "Whether to save or recall notes"
                    },
                    "key": {
                        "type": "string",
                        "description": "Key/topic for the note (used for save and recall)"
                    },
                    "content": {
                        "type": "string",
                        "description": "Note content (required for save)"
                    }
                },
                "required": ["action", "key"]
            }),
        }
    }

    async fn call(&self, args: serde_json::Value) -> Result<ToolResult> {
        let action = args["action"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: action"))?;
        let key = args["key"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: key"))?;

        let path = self.resolve_path();

        match action {
            "save" => {
                let content = args["content"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("missing required parameter: content"))?;

                debug!(key, "saving note");

                let mut store = NotesStore::load(&path).await;
                store.notes.push(NoteEntry {
                    key: key.to_string(),
                    content: content.to_string(),
                    timestamp: chrono_now(),
                });
                store.save(&path).await?;

                Ok(ToolResult::success(format!("note saved with key: {key}")))
            }
            "recall" => {
                debug!(key, "recalling notes");

                let store = NotesStore::load(&path).await;
                let matches: Vec<&NoteEntry> = store
                    .notes
                    .iter()
                    .filter(|n| {
                        n.key.contains(key) || n.content.contains(key)
                    })
                    .collect();

                if matches.is_empty() {
                    Ok(ToolResult::success(format!(
                        "no notes found matching: {key}"
                    )))
                } else {
                    let output: Vec<String> = matches
                        .iter()
                        .map(|n| {
                            format!("[{}] {}: {}", n.timestamp, n.key, n.content)
                        })
                        .collect();
                    Ok(ToolResult::success(output.join("\n")))
                }
            }
            other => Ok(ToolResult::failure(format!(
                "unknown action: {other} (expected 'save' or 'recall')"
            ))),
        }
    }
}

fn chrono_now() -> String {
    use std::time::SystemTime;
    let duration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}", duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn note_save_and_recall() {
        let dir = tempfile::tempdir().unwrap();
        let notes_path = dir.path().join("test_notes.json");
        let tool = NoteTool::with_path(notes_path);

        let save_result = tool
            .call(serde_json::json!({
                "action": "save",
                "key": "project-setup",
                "content": "The project uses tokio for async runtime"
            }))
            .await
            .unwrap();
        assert!(save_result.success);

        let recall_result = tool
            .call(serde_json::json!({
                "action": "recall",
                "key": "project"
            }))
            .await
            .unwrap();
        assert!(recall_result.success);
        assert!(recall_result.output.contains("tokio"));
    }

    #[tokio::test]
    async fn note_recall_empty() {
        let dir = tempfile::tempdir().unwrap();
        let notes_path = dir.path().join("empty_notes.json");
        let tool = NoteTool::with_path(notes_path);

        let result = tool
            .call(serde_json::json!({
                "action": "recall",
                "key": "nonexistent"
            }))
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("no notes found"));
    }
}
