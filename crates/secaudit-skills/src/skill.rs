//! 简单的 Skill 文件加载器。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use gray_matter::{Matter, engine::YAML};
use secaudit_core::Error;
use secaudit_storage::RuntimeLayout;
use serde_yaml::Value;

const SKILL_FILE: &str = "SKILL.md";

/// 从 Markdown 文件加载的 Skill。
#[derive(Debug, Clone)]
pub struct FileSkill {
    name: String,
    description: String,
    prompt: String,
}

impl FileSkill {
    /// 从 Skill 目录加载 `SKILL.md`。
    ///
    /// # Errors
    ///
    /// 当 `SKILL.md` 读取失败或内容解析失败时返回错误。
    pub fn from_dir(skill_dir: &Path) -> Result<Self, Error> {
        let skill_file = skill_dir.join(SKILL_FILE);
        let content = fs::read_to_string(&skill_file).map_err(|e| {
            Error::Tool(format!(
                "读取 Skill 文件失败「{}」：{e}",
                skill_file.display()
            ))
        })?;

        Self::from_content(&content)
    }

    /// 从 Markdown 内容解析 Skill。
    ///
    /// # Errors
    ///
    /// 当 frontmatter 缺失、YAML 解析失败或缺少必要字段时返回错误。
    pub fn from_content(content: &str) -> Result<Self, Error> {
        let matter = Matter::<YAML>::new();
        let parsed = matter
            .parse::<Value>(content.trim_start())
            .map_err(|e| Error::Tool(format!("解析 Skill frontmatter 失败：{e}")))?;
        let frontmatter = parsed.data.ok_or_else(|| {
            Error::Tool("Skill 文件缺少 YAML frontmatter（需以 `---` 开始）".into())
        })?;

        let name = frontmatter
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Tool("Skill frontmatter 缺少 name".into()))?
            .to_owned();
        let description = frontmatter
            .get("description")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Tool("Skill frontmatter 缺少 description".into()))?
            .to_owned();

        Ok(Self {
            name,
            description,
            prompt: parsed.content.trim().to_owned(),
        })
    }

    /// 获取 Skill 名称。
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 获取 Skill 描述。
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// 构建注入到 Agent 的用户 prompt。
    #[must_use]
    pub fn build_prompt(&self, user_input: &str, _session_id: &str) -> String {
        let arguments = extract_arguments(user_input, &self.name);

        if arguments.is_empty() || self.prompt.contains("$ARGUMENTS") {
            self.prompt.replace("$ARGUMENTS", &arguments)
        } else {
            format!("{}\n\nARGUMENTS: {arguments}", self.prompt)
        }
    }
}

/// Skill 注册表（从目录加载所有 Skills）。
#[derive(Debug, Default)]
pub struct SkillRegistry {
    skills: HashMap<String, FileSkill>,
}

impl SkillRegistry {
    /// 创建空注册表。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 从 `~/.secaudit/skills/` 和 `<base>/.secaudit/skills/` 加载 Skills。
    ///
    /// # Errors
    ///
    /// 当 Skills 目录读取失败时返回错误。
    pub fn load_from_dir(base_dir: &Path) -> Result<Self, Error> {
        let mut registry = Self::new();

        for skills_dir in skill_search_dirs(base_dir) {
            registry.load_skills_dir(&skills_dir)?;
        }

        Ok(registry)
    }

    /// 按名称获取 Skill。
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&FileSkill> {
        self.skills.get(name)
    }

    /// 列出所有 Skill 的名称和描述。
    #[must_use]
    pub fn list(&self) -> Vec<(String, String)> {
        let mut skills = self
            .skills
            .values()
            .map(|skill| (skill.name().to_owned(), skill.description().to_owned()))
            .collect::<Vec<_>>();
        skills.sort_by(|left, right| left.0.cmp(&right.0));
        skills
    }

    /// 根据 `/skill-name` 用户输入匹配 Skill。
    #[must_use]
    pub fn match_command(&self, input: &str) -> Option<&FileSkill> {
        self.skills
            .values()
            .find(|skill| match_skill_name(skill.name(), input))
    }

    fn load_skills_dir(&mut self, skills_dir: &Path) -> Result<(), Error> {
        if !skills_dir.exists() {
            return Ok(());
        }

        for entry in fs::read_dir(skills_dir).map_err(|e| {
            Error::Tool(format!(
                "读取 Skills 目录「{}」失败：{e}",
                skills_dir.display()
            ))
        })? {
            let entry = entry.map_err(|e| Error::Tool(format!("读取目录项失败：{e}")))?;
            let path = entry.path();

            if path.is_dir()
                && let Ok(skill) = FileSkill::from_dir(&path)
            {
                self.skills.insert(skill.name().to_owned(), skill);
            }
        }

        Ok(())
    }
}

fn extract_arguments(input: &str, skill_name: &str) -> String {
    let prefix = format!("/{skill_name}");
    input
        .strip_prefix(&prefix)
        .map_or_else(|| input.trim().to_owned(), |rest| rest.trim().to_owned())
}

fn match_skill_name(name: &str, input: &str) -> bool {
    let prefix = format!("/{name}");
    input
        .strip_prefix(&prefix)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
}

fn skill_search_dirs(base_dir: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Ok(layout) = RuntimeLayout::default_root() {
        dirs.push(layout.user_skills_dir());
    }

    dirs.push(RuntimeLayout::new(base_dir.to_path_buf()).user_skills_dir());
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_SKILL: &str = "\
---
name: test-skill
description: 测试 Skill
---

测试 prompt: $ARGUMENTS";

    #[test]
    fn parse_skill_from_content() {
        let skill = FileSkill::from_content(SAMPLE_SKILL).expect("parse skill");

        assert_eq!(skill.name(), "test-skill");
        assert_eq!(skill.description(), "测试 Skill");
        assert_eq!(
            skill.build_prompt("/test-skill hello", "session"),
            "测试 prompt: hello"
        );
    }

    #[test]
    fn match_command_by_exact_skill_name() {
        let skill = FileSkill::from_content(SAMPLE_SKILL).expect("parse skill");
        let mut registry = SkillRegistry::new();
        registry.skills.insert(skill.name().to_owned(), skill);

        assert!(registry.match_command("/test-skill arg").is_some());
        assert!(registry.match_command("/test-skillname arg").is_none());
    }
}
