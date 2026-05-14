use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ragent_traits::skill::{SkillContent, SkillMetadata};
use regex::Regex;
use serde::Deserialize;
use tracing::{debug, warn};

/// YAML frontmatter in `SKILL.md` (agentskills-style).
#[derive(Debug, Deserialize)]
struct SkillFrontmatter {
    name: String,
    description: String,
    #[serde(default)]
    #[allow(dead_code)]
    license: Option<String>,
    #[serde(default, rename = "allowed-tools")]
    #[allow(dead_code)]
    allowed_tools: Option<Vec<String>>,
    #[serde(default)]
    tags: Vec<String>,
}

/// Discover and load skills from a directory (`SKILL.md` with YAML frontmatter).
#[derive(Debug, Clone)]
pub struct SkillLoader {
    skills_dir: PathBuf,
}

impl SkillLoader {
    pub fn new(skills_dir: PathBuf) -> Self {
        Self { skills_dir }
    }

    pub fn skills_dir(&self) -> &Path {
        &self.skills_dir
    }

    /// Progressive disclosure level 1: metadata block for system prompt (Mini-Agent compatible).
    pub fn skills_metadata_prompt(skills: &[SkillMetadata]) -> String {
        if skills.is_empty() {
            return String::new();
        }
        let mut out = String::from("## Available Skills\n\n");
        out.push_str(
            "You have access to specialized skills. Each skill provides expert guidance for specific tasks.\n",
        );
        out.push_str("Load a skill's full content using the `get_skill` tool when needed.\n\n");
        for s in skills {
            out.push_str(&format!("- `{}`: {}\n", s.name, s.description));
        }
        out
    }

    /// Full formatted skill text for tool results (matches Mini-Agent `Skill::to_prompt`).
    pub fn format_skill_prompt(metadata: &SkillMetadata, body: &str) -> String {
        let skill_root = metadata
            .path
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        format!(
            "\n# Skill: {}\n\n{}\n\n**Skill Root Directory:** `{skill_root}`\n\n\
All files and references in this skill are relative to this directory.\n\n---\n\n{body}\n",
            metadata.name, metadata.description
        )
    }

    pub fn discover(&self) -> Result<Vec<SkillMetadata>> {
        let mut skills = Vec::new();

        if !self.skills_dir.exists() {
            debug!(dir = %self.skills_dir.display(), "skills directory not found");
            return Ok(skills);
        }

        self.scan_dir(&self.skills_dir, &mut skills)?;
        debug!(count = skills.len(), "discovered skills");
        Ok(skills)
    }

    /// Load skill by name; returns formatted prompt text for the model, or `None` if unknown.
    pub fn load_formatted(&self, name: &str) -> Result<Option<String>> {
        let skills = self.discover()?;
        let meta = skills.into_iter().find(|s| s.name == name);
        let Some(meta) = meta else {
            return Ok(None);
        };

        let raw = std::fs::read_to_string(&meta.path)
            .with_context(|| format!("failed to read {}", meta.path.display()))?;
        let body = extract_body_after_frontmatter(&raw)
            .with_context(|| format!("invalid SKILL.md: {}", meta.path.display()))?;
        let skill_root = meta.path.parent().unwrap_or_else(|| Path::new("."));
        let processed = process_skill_paths(body, skill_root);
        Ok(Some(Self::format_skill_prompt(&meta, &processed)))
    }

    /// Load raw [`SkillContent`] (full file text + metadata). Prefer `load_formatted` for LLM tool output.
    pub fn load(&self, name: &str) -> Result<Option<SkillContent>> {
        let skills = self.discover()?;
        let meta = skills.into_iter().find(|s| s.name == name);

        match meta {
            Some(metadata) => {
                let content = std::fs::read_to_string(&metadata.path).with_context(|| {
                    format!("failed to read skill: {}", metadata.path.display())
                })?;
                Ok(Some(SkillContent { metadata, content }))
            }
            None => Ok(None),
        }
    }

    fn scan_dir(&self, dir: &Path, skills: &mut Vec<SkillMetadata>) -> Result<()> {
        let entries = std::fs::read_dir(dir)
            .with_context(|| format!("failed to read directory: {}", dir.display()))?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                let skill_file = path.join("SKILL.md");
                if skill_file.exists() {
                    match metadata_from_skill_file(&skill_file) {
                        Ok(meta) => skills.push(meta),
                        Err(e) => {
                            warn!(
                                path = %skill_file.display(),
                                error = %e,
                                "failed to parse skill metadata"
                            );
                        }
                    }
                }
                self.scan_dir(&path, skills)?;
            }
        }

        Ok(())
    }
}

fn metadata_from_skill_file(path: &Path) -> Result<SkillMetadata> {
    let raw = std::fs::read_to_string(path)?;
    let yaml_src = extract_yaml_frontmatter(&raw)?;
    let fm: SkillFrontmatter = serde_yaml::from_str(yaml_src)
        .with_context(|| format!("invalid YAML frontmatter in {}", path.display()))?;

    Ok(SkillMetadata {
        name: fm.name,
        description: fm.description,
        tags: fm.tags,
        path: path.to_path_buf(),
    })
}

/// Returns markdown body (after closing `---`), trimmed.
fn extract_body_after_frontmatter(raw: &str) -> Result<&str> {
    let (_, body) = split_frontmatter(raw)?;
    Ok(body.trim())
}

fn extract_yaml_frontmatter(raw: &str) -> Result<&str> {
    let (yaml, _) = split_frontmatter(raw)?;
    Ok(yaml)
}

/// Split `---\n yaml \n---\n body` (first document only).
fn split_frontmatter(raw: &str) -> Result<(&str, &str)> {
    let raw = raw.trim_start_matches('\u{feff}').trim_start();
    let rest = raw
        .strip_prefix("---\n")
        .or_else(|| raw.strip_prefix("---\r\n"))
        .ok_or_else(|| anyhow::anyhow!("SKILL.md must start with --- frontmatter"))?;

    let (yaml, body) = rest
        .split_once("\n---\n")
        .or_else(|| rest.split_once("\n---\r\n"))
        .ok_or_else(|| anyhow::anyhow!("SKILL.md missing closing --- before body"))?;

    Ok((yaml.trim(), body.trim_start()))
}

/// Rewrite relative `scripts/` / `references/` / doc links to absolute paths (Mini-Agent parity).
fn process_skill_paths(content: &str, skill_dir: &Path) -> String {
    let mut out = content.to_string();

    let re_dirs =
        Regex::new(r"(python\s+|`)((?:scripts|references|assets)/[^\s`\)]+)").expect("valid regex");
    out = re_dirs
        .replace_all(&out, |caps: &regex::Captures| {
            let prefix = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let rel = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            let abs = skill_dir.join(rel);
            if abs.exists() {
                format!("{prefix}{}", abs.display())
            } else {
                caps.get(0)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default()
            }
        })
        .into_owned();

    let re_docs = Regex::new(
        r"(?i)(see|read|refer to|check)\s+([a-zA-Z0-9_.-]+\.(?:md|txt|json|yaml))([.,;\s])",
    )
    .expect("valid regex");
    out = re_docs
        .replace_all(&out, |caps: &regex::Captures| {
            let prefix = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let filename = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            let suffix = caps.get(3).map(|m| m.as_str()).unwrap_or("");
            let abs = skill_dir.join(filename);
            if abs.exists() {
                format!(
                    "{prefix}`{}` (use read_file to access){suffix}",
                    abs.display()
                )
            } else {
                caps.get(0)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default()
            }
        })
        .into_owned();

    let re_md = Regex::new(
        r"(?i)(?:(Read|See|Check|Refer to|Load|View)\s+)?\[(`?[^`\]]+`?)\]\(((?:\./)?[^)]+\.(?:md|txt|json|yaml|js|py|html))\)",
    )
    .expect("valid regex");
    out = re_md
        .replace_all(&out, |caps: &regex::Captures| {
            let prefix = caps
                .get(1)
                .map(|m| format!("{} ", m.as_str()))
                .unwrap_or_default();
            let link_text = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            let filepath = caps.get(3).map(|m| m.as_str()).unwrap_or("");
            let clean = filepath.strip_prefix("./").unwrap_or(filepath);
            let abs = skill_dir.join(clean);
            if abs.exists() {
                format!(
                    "{prefix}[{link_text}](`{}`) (use read_file to access)",
                    abs.display()
                )
            } else {
                caps.get(0)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default()
            }
        })
        .into_owned();

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_frontmatter_basic() {
        let raw = r#"---
name: "test-skill"
description: "A test skill"
tags:
  - coding
---

# Test Skill
Body line.
"#;
        let (yaml, body) = split_frontmatter(raw).unwrap();
        assert!(yaml.contains("name:"));
        assert!(body.contains("Body line."));
    }

    #[test]
    fn metadata_from_file_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("my-skill");
        std::fs::create_dir(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            r#"---
name: "my-skill"
description: "Does something cool"
---

# My Skill
Content here.
"#,
        )
        .unwrap();

        let meta = metadata_from_skill_file(&skill_dir.join("SKILL.md")).unwrap();
        assert_eq!(meta.name, "my-skill");
        assert_eq!(meta.description, "Does something cool");
    }

    #[test]
    fn skill_loader_discovers_and_load_formatted() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("demo");
        std::fs::create_dir(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            r#"---
name: "demo"
description: "Demo skill"
---

## Steps
Do the thing.
"#,
        )
        .unwrap();

        let loader = SkillLoader::new(dir.path().to_path_buf());
        let skills = loader.discover().unwrap();
        assert_eq!(skills.len(), 1);

        let prompt = loader.load_formatted("demo").unwrap().unwrap();
        assert!(prompt.contains("# Skill: demo"));
        assert!(prompt.contains("Do the thing."));
        assert!(prompt.contains("Skill Root Directory"));

        let meta = SkillLoader::skills_metadata_prompt(&skills);
        assert!(meta.contains("Available Skills"));
        assert!(meta.contains("`demo`"));
    }

    #[test]
    fn skill_loader_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let loader = SkillLoader::new(dir.path().to_path_buf());
        let skills = loader.discover().unwrap();
        assert!(skills.is_empty());
    }

    #[test]
    fn skill_loader_nonexistent_dir() {
        let loader = SkillLoader::new(PathBuf::from("/nonexistent/skills"));
        let skills = loader.discover().unwrap();
        assert!(skills.is_empty());
    }
}
