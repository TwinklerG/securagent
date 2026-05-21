use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime};

use chrono::Utc;
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::model::{
    ManagedSession, ProjectKey, ProjectMetadata, SCHEMA_VERSION, SessionListItem, SessionMetadata,
    SessionStatus, StoredSession, has_persistable_messages,
};

const RUNTIME_DIR: &str = ".secaudit";
const PROJECTS_DIR: &str = "projects";
const PROJECT_FILE: &str = "project.json";
const SESSIONS_DIR: &str = "sessions";
const INDEX_FILE: &str = "index.jsonl";
const MEMORY_DIR: &str = "memory";
const TOOL_CONFIG_DIR: &str = "tool-config";
const SKILLS_DIR: &str = "skills";
const INDEX_COMPACT_MIN_BYTES: u64 = 16 * 1024;
const INDEX_COMPACT_DUPLICATE_RATIO: usize = 2;
const INDEX_LOCK_POLL_INTERVAL: Duration = Duration::from_millis(10);
const INDEX_LOCK_MAX_POLLS: usize = 1_000;
const INDEX_LOCK_STALE_AFTER: Duration = Duration::from_mins(5);

/// 用户级运行时存储布局。
#[derive(Debug, Clone)]
pub struct StorageLayout {
    root: PathBuf,
}

impl StorageLayout {
    /// 使用默认 `~/.secaudit` 根目录。
    ///
    /// # Errors
    ///
    /// 无法推导用户 home 目录时返回错误。
    pub fn default_root() -> Result<Self> {
        let home_dir = dirs::home_dir().ok_or(Error::MissingHome)?;
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

    /// 项目目录。
    #[must_use]
    pub fn project_dir(&self, key: &ProjectKey) -> PathBuf {
        self.root.join(PROJECTS_DIR).join(key.as_str())
    }

    /// 项目元数据文件。
    #[must_use]
    pub fn project_file(&self, key: &ProjectKey) -> PathBuf {
        self.project_dir(key).join(PROJECT_FILE)
    }

    /// 会话目录。
    #[must_use]
    pub fn sessions_dir(&self, key: &ProjectKey) -> PathBuf {
        self.project_dir(key).join(SESSIONS_DIR)
    }

    /// 项目记忆目录。
    #[must_use]
    pub fn memory_dir(&self, key: &ProjectKey) -> PathBuf {
        self.project_dir(key).join(MEMORY_DIR)
    }

    /// 项目工具动态配置目录。
    #[must_use]
    pub fn tool_config_dir(&self, key: &ProjectKey) -> PathBuf {
        self.project_dir(key).join(TOOL_CONFIG_DIR)
    }

    /// 项目 Skill 动态配置目录。
    #[must_use]
    pub fn skills_dir(&self, key: &ProjectKey) -> PathBuf {
        self.project_dir(key).join(SKILLS_DIR)
    }

    /// 会话索引文件。
    #[must_use]
    pub fn session_index_file(&self, key: &ProjectKey) -> PathBuf {
        self.sessions_dir(key).join(INDEX_FILE)
    }

    /// active/archived 会话子目录。
    #[must_use]
    pub fn session_status_dir(&self, key: &ProjectKey, status: SessionStatus) -> PathBuf {
        self.sessions_dir(key).join(status.directory())
    }

    /// 会话文件路径。
    #[must_use]
    pub fn session_file(
        &self,
        key: &ProjectKey,
        status: SessionStatus,
        session_id: &str,
    ) -> PathBuf {
        self.session_status_dir(key, status)
            .join(format!("{session_id}.json"))
    }

    /// 初始化项目目录与元数据。
    ///
    /// # Errors
    ///
    /// 创建目录或写入项目元数据失败时返回错误。
    pub fn ensure_project(&self, work_dir: &Path) -> Result<ProjectMetadata> {
        let canonical_path = canonical_work_dir(work_dir);
        let key = self.project_key_for_path(&canonical_path)?;
        let now = now_rfc3339();

        fs::create_dir_all(self.session_status_dir(&key, SessionStatus::Active))?;
        fs::create_dir_all(self.session_status_dir(&key, SessionStatus::Archived))?;
        fs::create_dir_all(self.memory_dir(&key))?;
        fs::create_dir_all(self.tool_config_dir(&key))?;
        fs::create_dir_all(self.skills_dir(&key))?;

        let project_file = self.project_file(&key);
        let metadata = if project_file.exists() {
            let content = fs::read_to_string(&project_file)?;
            let mut metadata: ProjectMetadata = serde_json::from_str(&content)?;
            metadata.updated_at = now;
            metadata
        } else {
            ProjectMetadata {
                schema_version: SCHEMA_VERSION,
                project_key: key,
                canonical_path,
                created_at: now.clone(),
                updated_at: now,
            }
        };

        Self::write_json_atomic(&project_file, &metadata)?;
        Ok(metadata)
    }

    fn project_key_for_path(&self, canonical_path: &Path) -> Result<ProjectKey> {
        let base_key = ProjectKey::from_path(canonical_path);
        let base_file = self.project_file(&base_key);
        if !base_file.exists() {
            return Ok(base_key);
        }

        let metadata = read_project_metadata(&base_file)?;
        if metadata.canonical_path == canonical_path {
            return Ok(base_key);
        }

        let suffix = stable_path_suffix(canonical_path);
        for counter in 0..100u8 {
            let candidate_suffix = if counter == 0 {
                suffix.clone()
            } else {
                format!("{suffix}-{counter}")
            };
            let candidate = base_key.with_suffix(&candidate_suffix);
            let candidate_file = self.project_file(&candidate);
            if !candidate_file.exists() {
                return Ok(candidate);
            }

            let metadata = read_project_metadata(&candidate_file)?;
            if metadata.canonical_path == canonical_path {
                return Ok(candidate);
            }
        }

        Err(Error::PathConflict {
            path: self.project_dir(&base_key),
        })
    }

    /// 创建新会话。
    ///
    /// # Errors
    ///
    /// 初始化项目目录失败时返回错误。
    pub fn create_session(&self, work_dir: &Path) -> Result<ManagedSession> {
        let project = self.ensure_project(work_dir)?;
        let session = secaudit_agent::Session::new(project.canonical_path.clone());
        let title = "未命名会话".to_owned();
        Ok(ManagedSession::new(
            session,
            project.project_key,
            SessionStatus::Active,
            title,
            None,
        ))
    }

    /// 保存托管会话。
    ///
    /// # Errors
    ///
    /// 写入会话文件或索引失败时返回错误。
    pub fn save_session(&self, session: &ManagedSession) -> Result<SessionMetadata> {
        validate_session_id(session.id())?;
        if !has_persistable_messages(session.session().messages()) {
            return Err(Error::EmptySession {
                session_id: session.id().to_owned(),
            });
        }

        let updated_at = now_rfc3339();
        let stored = session.to_stored_session(updated_at);
        let path = self.session_file(session.project_key(), stored.status, &stored.id);

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        Self::write_json_atomic(&path, &stored)?;
        self.remove_opposite_status_file(session.project_key(), stored.status, &stored.id)?;
        let metadata = SessionMetadata::from_stored_session(&stored);
        self.append_session_index(session.project_key(), &metadata)?;
        Ok(metadata)
    }

    /// 加载 active 或 archived 会话。
    ///
    /// # Errors
    ///
    /// 会话不存在、ID 不合法或文件解析失败时返回错误。
    pub fn load_session(&self, work_dir: &Path, session_id: &str) -> Result<ManagedSession> {
        validate_session_id(session_id)?;
        let project = self.ensure_project(work_dir)?;
        let stored = self.read_stored_session(&project.project_key, session_id)?;
        Ok(ManagedSession::from_stored(stored))
    }

    /// 列出当前项目会话元数据。
    ///
    /// # Errors
    ///
    /// 初始化项目或读取索引失败时返回错误。
    pub fn list_sessions(&self, work_dir: &Path) -> Result<Vec<SessionMetadata>> {
        let project = self.ensure_project(work_dir)?;
        self.list_sessions_for_project(&project.project_key)
    }

    /// 列出当前项目会话，并附带最近消息预览。
    ///
    /// # Errors
    ///
    /// 初始化项目、读取索引或读取会话文件失败时返回错误。
    pub fn list_sessions_with_preview(&self, work_dir: &Path) -> Result<Vec<SessionListItem>> {
        let project = self.ensure_project(work_dir)?;
        self.list_sessions_with_preview_for_project(&project.project_key)
    }

    /// 归档 active 会话。
    ///
    /// # Errors
    ///
    /// 会话不存在、已归档或移动文件失败时返回错误。
    pub fn archive_session(&self, work_dir: &Path, session_id: &str) -> Result<SessionMetadata> {
        validate_session_id(session_id)?;
        let project = self.ensure_project(work_dir)?;
        let active_path =
            self.session_file(&project.project_key, SessionStatus::Active, session_id);
        if !active_path.exists() {
            let archived_path =
                self.session_file(&project.project_key, SessionStatus::Archived, session_id);
            if archived_path.exists() {
                return Err(Error::InvalidSessionStatus {
                    session_id: session_id.to_owned(),
                    status: SessionStatus::Archived.to_string(),
                });
            }
            return Err(Error::SessionNotFound {
                session_id: session_id.to_owned(),
            });
        }

        let content = fs::read_to_string(&active_path)?;
        let mut stored: StoredSession = serde_json::from_str(&content)?;
        stored.status = SessionStatus::Archived;
        stored.updated_at = now_rfc3339();

        let archived_path =
            self.session_file(&project.project_key, SessionStatus::Archived, session_id);
        if let Some(parent) = archived_path.parent() {
            fs::create_dir_all(parent)?;
        }
        if archived_path.exists() {
            return Err(Error::PathConflict {
                path: archived_path,
            });
        }

        Self::write_json_atomic(&archived_path, &stored)?;
        fs::remove_file(&active_path)?;

        let metadata = SessionMetadata::from_stored_session(&stored);
        self.append_session_index(&project.project_key, &metadata)?;
        Ok(metadata)
    }

    /// 生成会话管理投影需要的会话文件路径。
    #[must_use]
    pub fn session_path(&self, session: &ManagedSession) -> PathBuf {
        self.session_file(session.project_key(), session.status(), session.id())
    }

    fn list_sessions_for_project(&self, key: &ProjectKey) -> Result<Vec<SessionMetadata>> {
        let index_file = self.session_index_file(key);
        Ok(read_session_index(&index_file)?.into_sorted_sessions())
    }

    fn list_sessions_with_preview_for_project(
        &self,
        key: &ProjectKey,
    ) -> Result<Vec<SessionListItem>> {
        self.list_sessions_for_project(key)?
            .into_iter()
            .map(|metadata| {
                let stored = self.read_stored_session(key, &metadata.session_id)?;
                Ok(SessionListItem::from_stored_session(&stored))
            })
            .collect()
    }

    fn read_stored_session(&self, key: &ProjectKey, session_id: &str) -> Result<StoredSession> {
        for status in [SessionStatus::Active, SessionStatus::Archived] {
            let path = self.session_file(key, status, session_id);
            if path.exists() {
                let content = fs::read_to_string(path)?;
                return Ok(serde_json::from_str(&content)?);
            }
        }

        Err(Error::SessionNotFound {
            session_id: session_id.to_owned(),
        })
    }

    fn append_session_index(&self, key: &ProjectKey, metadata: &SessionMetadata) -> Result<()> {
        let index_file = self.session_index_file(key);
        if let Some(parent) = index_file.parent() {
            fs::create_dir_all(parent)?;
        }

        let _lock = IndexWriteLock::acquire(&index_file)?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&index_file)?;
        let line = serde_json::to_string(metadata)?;
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        file.flush()?;
        drop(file);
        Self::compact_session_index_if_needed(&index_file)?;
        Ok(())
    }

    fn compact_session_index_if_needed(index_file: &Path) -> Result<()> {
        let Ok(metadata) = fs::metadata(index_file) else {
            return Ok(());
        };
        if metadata.len() < INDEX_COMPACT_MIN_BYTES {
            return Ok(());
        }

        let index = read_session_index(index_file)?;
        if !index.should_compact() {
            return Ok(());
        }

        write_session_index_atomic(index_file, &index.into_sorted_sessions())
    }

    fn remove_opposite_status_file(
        &self,
        key: &ProjectKey,
        status: SessionStatus,
        session_id: &str,
    ) -> Result<()> {
        let opposite = match status {
            SessionStatus::Active => SessionStatus::Archived,
            SessionStatus::Archived => SessionStatus::Active,
        };
        let path = self.session_file(key, opposite, session_id);
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }

    fn write_json_atomic<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
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
}

struct SessionIndex {
    record_count: usize,
    by_id: BTreeMap<String, SessionMetadata>,
}

struct IndexWriteLock {
    path: PathBuf,
}

impl IndexWriteLock {
    fn acquire(index_file: &Path) -> Result<Self> {
        let lock_path = index_file.with_extension("jsonl.lock");
        for _ in 0..INDEX_LOCK_MAX_POLLS {
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock_path)
            {
                Ok(_) => return Ok(Self { path: lock_path }),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                    remove_stale_index_lock(&lock_path)?;
                    thread::sleep(INDEX_LOCK_POLL_INTERVAL);
                }
                Err(error) => return Err(error.into()),
            }
        }

        Err(io::Error::new(
            ErrorKind::TimedOut,
            format!("等待会话索引锁超时：{}", lock_path.display()),
        )
        .into())
    }
}

impl Drop for IndexWriteLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn remove_stale_index_lock(lock_path: &Path) -> Result<()> {
    let Ok(metadata) = fs::metadata(lock_path) else {
        return Ok(());
    };
    let Ok(modified) = metadata.modified() else {
        return Ok(());
    };
    let Ok(age) = SystemTime::now().duration_since(modified) else {
        return Ok(());
    };
    if age <= INDEX_LOCK_STALE_AFTER {
        return Ok(());
    }

    match fs::remove_file(lock_path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

impl SessionIndex {
    fn should_compact(&self) -> bool {
        let retained_count = self.by_id.len().max(1);
        self.record_count > retained_count.saturating_mul(INDEX_COMPACT_DUPLICATE_RATIO)
    }

    fn into_sorted_sessions(self) -> Vec<SessionMetadata> {
        let mut sessions: Vec<SessionMetadata> = self.by_id.into_values().collect();
        sessions.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
        sessions
    }
}

fn read_session_index(index_file: &Path) -> Result<SessionIndex> {
    if !index_file.exists() {
        return Ok(SessionIndex {
            record_count: 0,
            by_id: BTreeMap::new(),
        });
    }

    let file = File::open(index_file)?;
    let reader = BufReader::new(file);
    let mut record_count = 0usize;
    let mut by_id: BTreeMap<String, SessionMetadata> = BTreeMap::new();

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        record_count += 1;
        let metadata: SessionMetadata = serde_json::from_str(&line)?;
        if metadata.message_count == 0 {
            continue;
        }
        by_id.insert(metadata.session_id.clone(), metadata);
    }

    Ok(SessionIndex {
        record_count,
        by_id,
    })
}

fn write_session_index_atomic(index_file: &Path, sessions: &[SessionMetadata]) -> Result<()> {
    let tmp_path = atomic_tmp_path(index_file);
    {
        let mut file = File::create(&tmp_path)?;
        for session in sessions {
            serde_json::to_writer(&mut file, session)?;
            file.write_all(b"\n")?;
        }
        file.sync_all()?;
    }
    fs::rename(tmp_path, index_file)?;
    Ok(())
}

fn canonical_work_dir(work_dir: &Path) -> PathBuf {
    work_dir
        .canonicalize()
        .unwrap_or_else(|_| work_dir.to_path_buf())
}

fn read_project_metadata(path: &Path) -> Result<ProjectMetadata> {
    let content = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&content)?)
}

fn stable_path_suffix(path: &Path) -> String {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = FNV_OFFSET;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }

    format!("{:08x}", hash & 0xffff_ffff)
}

fn atomic_tmp_path(path: &Path) -> PathBuf {
    let tmp_name = format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("secaudit"),
        Uuid::new_v4()
    );
    path.with_file_name(tmp_name)
}

fn validate_session_id(session_id: &str) -> Result<()> {
    let is_safe = !session_id.is_empty()
        && session_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'));

    if is_safe {
        Ok(())
    } else {
        Err(Error::InvalidSessionId {
            session_id: session_id.to_owned(),
        })
    }
}

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::TempDir;

    use super::StorageLayout;
    use crate::model::{ProjectKey, SessionStatus};

    #[test]
    fn project_key_is_readable_path_encoding() {
        let key = ProjectKey::from_path(Path::new("/workspace/team/Sample Project"));

        assert_eq!(key.as_str(), "-workspace-team-Sample-Project");
    }

    #[test]
    fn creates_lists_loads_and_archives_session() {
        let temp = TempDir::new().expect("create tempdir");
        let work_dir = temp.path().join("project");
        fs::create_dir_all(&work_dir).expect("create work dir");
        let storage = StorageLayout::new(temp.path().join("runtime"));

        let session = storage.create_session(&work_dir).expect("create session");
        let session_id = session.id().to_owned();

        assert!(
            storage
                .list_sessions(&work_dir)
                .expect("list sessions")
                .is_empty(),
            "empty sessions should not be listed"
        );

        let mut session = session;
        session
            .session_mut()
            .push_message(secaudit_agent::ChatMessage::user("hello"));
        storage.save_session(&session).expect("save session");

        let listed = storage.list_sessions(&work_dir).expect("list sessions");
        assert_eq!(listed.len(), 1);
        assert_eq!(
            listed.first().map(|item| item.session_id.as_str()),
            Some(session_id.as_str())
        );
        assert_eq!(
            listed.first().map(|item| item.status),
            Some(SessionStatus::Active)
        );

        let loaded = storage
            .load_session(&work_dir, &session_id)
            .expect("load session");
        assert_eq!(loaded.id(), session_id);

        let archived = storage
            .archive_session(&work_dir, &session_id)
            .expect("archive session");
        assert_eq!(archived.status, SessionStatus::Archived);

        let listed = storage.list_sessions(&work_dir).expect("list archived");
        assert_eq!(listed.len(), 1);
        assert_eq!(
            listed.first().map(|item| item.status),
            Some(SessionStatus::Archived)
        );
    }

    #[test]
    fn repeated_session_saves_compact_index_to_latest_metadata() {
        let temp = TempDir::new().expect("create tempdir");
        let work_dir = temp.path().join("project");
        fs::create_dir_all(&work_dir).expect("create work dir");
        let storage = StorageLayout::new(temp.path().join("runtime"));
        let project = storage.ensure_project(&work_dir).expect("ensure project");
        let mut session = storage.create_session(&work_dir).expect("create session");
        let session_id = session.id().to_owned();

        session
            .session_mut()
            .push_message(secaudit_agent::ChatMessage::user("first"));
        let stale_metadata = storage.save_session(&session).expect("save session");
        let index_file = storage.session_index_file(&project.project_key);
        let stale_line = serde_json::to_string(&stale_metadata).expect("serialize metadata");
        let mut bloated_index = String::new();
        for _ in 0..140 {
            bloated_index.push_str(&stale_line);
            bloated_index.push('\n');
        }
        fs::write(&index_file, bloated_index).expect("inflate index");

        session
            .session_mut()
            .push_message(secaudit_agent::ChatMessage::user("second"));
        storage.save_session(&session).expect("save session again");

        let index = fs::read_to_string(index_file).expect("read compacted index");
        let lines = index
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect::<Vec<_>>();
        assert_eq!(lines.len(), 1);

        let listed = storage.list_sessions(&work_dir).expect("list sessions");
        assert_eq!(listed.len(), 1);
        assert_eq!(
            listed.first().map(|item| item.session_id.as_str()),
            Some(session_id.as_str())
        );
        assert_eq!(listed.first().map(|item| item.message_count), Some(2));
    }

    #[test]
    fn rejects_path_like_session_id() {
        let temp = TempDir::new().expect("create tempdir");
        let work_dir = temp.path().join("project");
        fs::create_dir_all(&work_dir).expect("create work dir");
        let storage = StorageLayout::new(temp.path().join("runtime"));

        let result = storage.load_session(&work_dir, "../bad").err();

        assert!(result.is_some(), "path-like session ids should be rejected");
    }

    #[test]
    fn rejects_empty_session_save() {
        let temp = TempDir::new().expect("create tempdir");
        let work_dir = temp.path().join("project");
        fs::create_dir_all(&work_dir).expect("create work dir");
        let storage = StorageLayout::new(temp.path().join("runtime"));

        let session = storage.create_session(&work_dir).expect("create session");

        let result = storage.save_session(&session).err();

        assert!(result.is_some(), "empty sessions should not persist");
    }

    #[test]
    fn ensure_project_creates_shared_runtime_dirs() {
        let temp = TempDir::new().expect("create tempdir");
        let work_dir = temp.path().join("project");
        fs::create_dir_all(&work_dir).expect("create work dir");
        let storage = StorageLayout::new(temp.path().join("runtime"));

        let project = storage.ensure_project(&work_dir).expect("ensure project");

        assert!(storage.memory_dir(&project.project_key).is_dir());
        assert!(storage.tool_config_dir(&project.project_key).is_dir());
        assert!(storage.skills_dir(&project.project_key).is_dir());
    }

    #[test]
    fn colliding_readable_project_keys_get_disambiguated() {
        let temp = TempDir::new().expect("create tempdir");
        let first = temp.path().join("a-b");
        let second = temp.path().join("a").join("b");
        fs::create_dir_all(&first).expect("create first work dir");
        fs::create_dir_all(&second).expect("create second work dir");
        let storage = StorageLayout::new(temp.path().join("runtime"));

        let first_project = storage.ensure_project(&first).expect("ensure first");
        let second_project = storage.ensure_project(&second).expect("ensure second");
        let second_again = storage
            .ensure_project(&second)
            .expect("ensure second again");

        assert_ne!(first_project.project_key, second_project.project_key);
        assert_eq!(second_project.project_key, second_again.project_key);
        assert!(second_project.project_key.as_str().contains("--"));
        assert_eq!(
            second_project.canonical_path,
            second.canonicalize().expect("canonical second")
        );
    }
}
