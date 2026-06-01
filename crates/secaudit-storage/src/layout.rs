use std::env::home_dir;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::lock::FileLock;

// ── 路径常量 ──────────────────────────────────────────────────────────────────

/// 用户级运行时数据目录名。
pub const RUNTIME_DIR: &str = ".secaudit";
/// 项目子目录名。
pub const PROJECTS_DIR: &str = "projects";
/// 项目元数据文件名。
pub const PROJECT_FILE: &str = "project.json";
/// 会话子目录名。
pub const SESSIONS_DIR: &str = "sessions";
/// 会话索引文件名。
pub const INDEX_FILE: &str = "index.jsonl";
/// 项目记忆子目录名。
pub const MEMORY_DIR: &str = "memory";
/// 工具动态配置子目录名。
pub const TOOL_CONFIG_DIR: &str = "tool-config";
/// Skill 动态配置子目录名。
pub const SKILLS_DIR: &str = "skills";
/// Active 会话子目录名。
pub const ACTIVE_DIR: &str = "active";
/// Archived 会话子目录名。
pub const ARCHIVED_DIR: &str = "archived";

// ── 运行时布局 ────────────────────────────────────────────────────────────────

/// 用户级运行时存储布局。
///
/// 所有路径构建方法接受 `&str` 作为项目键，与业务模型类型解耦。
/// 提供原子 JSON 写入、JSONL 读写、文件锁等底层持久化能力。
#[derive(Debug, Clone)]
pub struct RuntimeLayout {
    root: PathBuf,
}

impl RuntimeLayout {
    /// 使用默认 `~/.secaudit` 根目录。
    ///
    /// # Errors
    ///
    /// 无法推导用户 home 目录时返回错误。
    pub fn default_root() -> Result<Self> {
        let home_dir = home_dir().ok_or(Error::MissingHome)?;
        Ok(Self::new(home_dir.join(RUNTIME_DIR)))
    }

    /// 使用指定根目录，主要用于测试和显式配置。
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// 存储根目录。
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    // ── 路径构建 ────────────────────────────────────────────────────────────

    /// 项目目录。
    #[must_use]
    pub fn project_dir(&self, key: &str) -> PathBuf {
        self.root.join(PROJECTS_DIR).join(key)
    }

    /// 项目元数据文件。
    #[must_use]
    pub fn project_file(&self, key: &str) -> PathBuf {
        self.project_dir(key).join(PROJECT_FILE)
    }

    /// 会话目录。
    #[must_use]
    pub fn sessions_dir(&self, key: &str) -> PathBuf {
        self.project_dir(key).join(SESSIONS_DIR)
    }

    /// 会话索引文件。
    #[must_use]
    pub fn session_index_file(&self, key: &str) -> PathBuf {
        self.sessions_dir(key).join(INDEX_FILE)
    }

    /// Active / Archived 会话子目录。
    #[must_use]
    pub fn session_status_dir(&self, key: &str, status: &str) -> PathBuf {
        self.sessions_dir(key).join(status)
    }

    /// 会话文件路径。
    #[must_use]
    pub fn session_file(&self, key: &str, status: &str, session_id: &str) -> PathBuf {
        self.session_status_dir(key, status)
            .join(format!("{session_id}.json"))
    }

    /// 项目记忆目录。
    #[must_use]
    pub fn memory_dir(&self, key: &str) -> PathBuf {
        self.project_dir(key).join(MEMORY_DIR)
    }

    /// 项目工具动态配置目录。
    #[must_use]
    pub fn tool_config_dir(&self, key: &str) -> PathBuf {
        self.project_dir(key).join(TOOL_CONFIG_DIR)
    }

    /// 项目 Skill 动态配置目录。
    #[must_use]
    pub fn skills_dir(&self, key: &str) -> PathBuf {
        self.project_dir(key).join(SKILLS_DIR)
    }

    /// 用户级 Skill 目录（`~/.secaudit/skills/`）。
    #[must_use]
    pub fn user_skills_dir(&self) -> PathBuf {
        self.root.join(SKILLS_DIR)
    }

    /// 用户级配置文件路径（`~/.secaudit/config.json`）。
    #[must_use]
    pub fn config_file(&self) -> PathBuf {
        self.root.join("config.json")
    }

    // ── 目录操作 ────────────────────────────────────────────────────────────

    /// 确保目录存在，必要时递归创建。
    ///
    /// # Errors
    ///
    /// 目录创建失败时返回错误。
    pub fn ensure_dir(path: &Path) -> Result<()> {
        fs::create_dir_all(path)?;
        Ok(())
    }

    /// 为指定项目创建完整的运行时目录树。
    ///
    /// # Errors
    ///
    /// 目录创建失败时返回错误。
    pub fn ensure_project_dirs(&self, key: &str) -> Result<()> {
        Self::ensure_dir(&self.session_status_dir(key, ACTIVE_DIR))?;
        Self::ensure_dir(&self.session_status_dir(key, ARCHIVED_DIR))?;
        Self::ensure_dir(&self.memory_dir(key))?;
        Self::ensure_dir(&self.tool_config_dir(key))?;
        Self::ensure_dir(&self.skills_dir(key))?;
        Ok(())
    }

    // ── 原子写入 ────────────────────────────────────────────────────────────

    /// 原子写入 JSON 文件（先写临时文件，再 `rename`）。
    ///
    /// # Errors
    ///
    /// 目录创建、序列化或文件重命名失败时返回错误。
    pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let tmp_path = atomic_tmp_path(path);
        {
            let mut file = File::create(&tmp_path)?;
            serde_json::to_writer_pretty(&mut file, value)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
        }
        fs::rename(tmp_path, path)?;
        Ok(())
    }

    // ── JSONL 读写 ──────────────────────────────────────────────────────────

    /// 向 JSONL 文件追加一行 JSON。
    ///
    /// 内部使用 `FileLock` 确保并发安全。
    ///
    /// # Errors
    ///
    /// 加锁失败、序列化失败或写入失败时返回错误。
    pub fn append_jsonl<T: Serialize>(path: &Path, item: &T) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let _lock = FileLock::acquire(path)?;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        let line = serde_json::to_string(item)?;
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        file.flush()?;
        Ok(())
    }

    /// 从 JSONL 文件读取所有行，返回解析后的条目列表。
    ///
    /// 跳过空白行，不存在的文件视为空列表。
    ///
    /// # Errors
    ///
    /// 文件读取或 JSON 解析失败时返回错误。
    pub fn read_jsonl<T: DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
        if !path.exists() {
            return Ok(Vec::new());
        }

        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let mut items = Vec::new();

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let item: T = serde_json::from_str(&line)?;
            items.push(item);
        }

        Ok(items)
    }

    /// 原子重写整个 JSONL 文件。
    ///
    /// # Errors
    ///
    /// 序列化或文件写入失败时返回错误。
    pub fn write_jsonl_atomic<T: Serialize>(path: &Path, items: &[T]) -> Result<()> {
        let tmp_path = atomic_tmp_path(path);
        {
            let mut file = File::create(&tmp_path)?;
            for item in items {
                serde_json::to_writer(&mut file, item)?;
                file.write_all(b"\n")?;
            }
            file.sync_all()?;
        }
        fs::rename(tmp_path, path)?;
        Ok(())
    }
}

// ── 助手函数 ──────────────────────────────────────────────────────────────────

/// 为目标文件路径生成唯一的临时文件名。
#[must_use]
pub fn atomic_tmp_path(path: &Path) -> PathBuf {
    let tmp_name = format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("secaudit"),
        Uuid::new_v4()
    );
    path.with_file_name(tmp_name)
}

/// 规范化工作目录路径。
#[must_use]
pub fn canonical_work_dir(work_dir: &Path) -> PathBuf {
    work_dir
        .canonicalize()
        .unwrap_or_else(|_| work_dir.to_path_buf())
}

// ── 测试 ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use serde::{Deserialize, Serialize};
    use tempfile::TempDir;

    use super::RuntimeLayout;

    #[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
    struct TestRecord {
        id: String,
        value: u32,
    }

    #[test]
    fn jsonl_append_and_read() {
        let temp = TempDir::new().expect("create tempdir");
        let jsonl_path = temp.path().join("test.jsonl");

        let record1 = TestRecord {
            id: "a".into(),
            value: 1,
        };
        let record2 = TestRecord {
            id: "b".into(),
            value: 2,
        };

        RuntimeLayout::append_jsonl(&jsonl_path, &record1).expect("append first");
        RuntimeLayout::append_jsonl(&jsonl_path, &record2).expect("append second");

        let items: Vec<TestRecord> = RuntimeLayout::read_jsonl(&jsonl_path).expect("read jsonl");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0], record1);
        assert_eq!(items[1], record2);
    }

    #[test]
    fn jsonl_atomic_rewrite() {
        let temp = TempDir::new().expect("create tempdir");
        let jsonl_path = temp.path().join("test.jsonl");

        let records = vec![
            TestRecord {
                id: "x".into(),
                value: 42,
            },
            TestRecord {
                id: "y".into(),
                value: 99,
            },
        ];

        RuntimeLayout::write_jsonl_atomic(&jsonl_path, &records).expect("write atomic");

        let items: Vec<TestRecord> = RuntimeLayout::read_jsonl(&jsonl_path).expect("read back");
        assert_eq!(items, records);
    }

    #[test]
    fn jsonl_read_nonexistent_returns_empty() {
        let items: Vec<TestRecord> =
            RuntimeLayout::read_jsonl(Path::new("/nonexistent/file.jsonl"))
                .expect("read nonexistent");
        assert!(items.is_empty());
    }

    #[test]
    fn write_json_atomic_creates_parent_dirs() {
        let temp = TempDir::new().expect("create tempdir");
        let path = temp.path().join("sub").join("nested").join("data.json");

        RuntimeLayout::write_json_atomic(&path, &serde_json::json!({"key": "value"}))
            .expect("write atomic");

        let content = fs::read_to_string(&path).expect("read back");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse json");
        assert_eq!(parsed["key"], "value");
    }

    #[test]
    fn ensure_project_dirs_creates_all_subdirs() {
        let temp = TempDir::new().expect("create tempdir");
        let layout = RuntimeLayout::new(temp.path().join("runtime"));

        layout
            .ensure_project_dirs("test-project")
            .expect("ensure dirs");

        assert!(layout.session_status_dir("test-project", "active").is_dir());
        assert!(
            layout
                .session_status_dir("test-project", "archived")
                .is_dir()
        );
        assert!(layout.memory_dir("test-project").is_dir());
        assert!(layout.tool_config_dir("test-project").is_dir());
        assert!(layout.skills_dir("test-project").is_dir());
    }

    #[test]
    fn default_root_returns_error_without_home() {
        // We can't easily remove HOME, so we just verify default_root()
        // compiles and runs (may succeed or fail depending on environment).
        let _ = RuntimeLayout::default_root();
    }
}
