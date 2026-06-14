//! 项目记忆 —— 项目级知识积累，在多次审计会话中逐步完善。

use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::error::Result;

const PROJECT_MEMORY_DIR: &str = "project_memory";
const FACTS_FILE: &str = "facts.json";

/// 一条项目级事实。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectFact {
    /// 事实键，如 `"cwe_stats"`、`"file:src/auth.py"`。
    pub key: String,
    /// 事实内容（自由文本或 JSON）。
    pub content: String,
    /// 更新时间。
    pub updated_at: String,
}

pub(crate) struct ProjectMemoryStore {
    base_dir: PathBuf,
}

impl ProjectMemoryStore {
    pub(crate) fn new(memory_dir: &Path) -> Self {
        Self {
            base_dir: memory_dir.join(PROJECT_MEMORY_DIR),
        }
    }

    fn facts_file(&self) -> PathBuf {
        self.base_dir.join(FACTS_FILE)
    }

    /// 存储/更新一条项目事实（按 key upsert）。
    pub(crate) fn upsert_fact(&self, key: &str, content: &str) -> Result<()> {
        let mut facts = self.load()?;
        let now = Utc::now().to_rfc3339();

        if let Some(existing) = facts.iter_mut().find(|f| f.key == key) {
            existing.content = content.into();
            existing.updated_at = now;
        } else {
            facts.push(ProjectFact {
                key: key.into(),
                content: content.into(),
                updated_at: now,
            });
        }

        self.save(&facts)
    }

    /// 读取全部项目事实。
    pub(crate) fn all_facts(&self) -> Result<Vec<ProjectFact>> {
        self.load()
    }

    fn load(&self) -> Result<Vec<ProjectFact>> {
        let path = self.facts_file();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        Ok(serde_json::from_reader(reader)?)
    }

    fn save(&self, facts: &[ProjectFact]) -> Result<()> {
        let dir = &self.base_dir;
        fs::create_dir_all(dir)?;

        let path = self.facts_file();
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(facts)?;
        fs::write(&tmp, json)?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn upsert_and_read_facts() {
        let temp = TempDir::new().expect("create tempdir");
        let store = ProjectMemoryStore::new(&temp.path().join("memory"));

        store
            .upsert_fact("cwe_stats", r#"{"CWE-89": 3, "CWE-79": 1}"#)
            .expect("upsert");
        store
            .upsert_fact("file:src/auth.py", "历史发现: SQL注入(Fixed), XSS(Pending)")
            .expect("upsert");

        let facts = store.all_facts().expect("read");
        assert_eq!(facts.len(), 2);
    }

    #[test]
    fn upsert_same_key_updates() {
        let temp = TempDir::new().expect("create tempdir");
        let store = ProjectMemoryStore::new(&temp.path().join("memory"));

        store.upsert_fact("key1", "v1").expect("upsert");
        store.upsert_fact("key1", "v2").expect("upsert");

        let facts = store.all_facts().expect("read");
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].content, "v2");
    }

    #[test]
    fn empty_store_returns_empty() {
        let temp = TempDir::new().expect("create tempdir");
        let store = ProjectMemoryStore::new(&temp.path().join("memory"));
        assert!(store.all_facts().expect("read").is_empty());
    }
}
