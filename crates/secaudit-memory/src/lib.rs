//! `secaudit-memory` —— 三层记忆系统：短时记忆、长期记忆、项目记忆。

mod error;
mod long_term;
mod project_memory;
mod short_term;

pub use error::{Error, Result};
pub use long_term::{EntityRef, Finding, FindingStatus, SessionSummary};
pub use project_memory::ProjectFact;
pub use short_term::ChatRecord;

use std::path::PathBuf;

use long_term::LongTermStore;
use project_memory::ProjectMemoryStore;
use short_term::ShortTermStore;

// ── L1 Short-Term Memory ──

/// 短时记忆 trait —— 按会话管理的对话摘要。
///
/// # 线程安全
///
/// 此 trait 的方法接受 `&self`（不可变引用）。实现者应保证对**同一 `session_id`**
/// 的并发写入不会导致数据损坏。文件系统实现当前不提供内部互斥锁，
/// 调用方应确保同一会话的写入是串行的。
pub trait ShortTermMemory {
    /// 追加一条对话摘要。
    ///
    /// `importance` 为重要性分数 (0.0–5.0)，在短时记忆截断时优先保留高重要性记录。
    ///
    /// # Errors
    ///
    /// 文件 I/O 或 JSON 序列化失败时返回错误。
    fn record_chat(&self, session_id: &str, content: &str, importance: f64) -> Result<()>;

    /// 读取指定会话最近 N 条摘要（时间倒序）。
    ///
    /// # Errors
    ///
    /// 文件 I/O 或 JSON 反序列化失败时返回错误。
    fn recent_by_session(&self, session_id: &str, limit: usize) -> Result<Vec<ChatRecord>>;

    /// 清理指定会话的短期记忆。
    ///
    /// # Errors
    ///
    /// 文件移除失败时返回错误。
    fn clear_short_term(&self, _session_id: &str) -> Result<()> {
        Ok(())
    }
}

// ── L2 Long-Term Memory ──

/// 长期记忆 trait —— 会话压缩摘要、结构化发现与实体索引。
pub trait LongTermMemory {
    /// 读取全部会话摘要（最新在前）。
    ///
    /// # Errors
    ///
    /// 文件 I/O 或 JSON 反序列化失败时返回错误。
    fn all_summaries(&self) -> Result<Vec<SessionSummary>>;

    /// 增量合并 finding 到 L2（不写 session 内容，只更新 findings + entity index）。
    ///
    /// # Errors
    ///
    /// 文件 I/O 或 JSON 序列化失败时返回错误。
    fn merge_to_long_term(&self, _session_id: &str, _findings: &[Finding]) -> Result<()> {
        Ok(())
    }

    /// 完成 L2 会话最终化：写入 session content。
    ///
    /// # Errors
    ///
    /// 文件 I/O 或 JSON 序列化失败时返回错误。
    fn finalize_long_term(&self, _session_id: &str, _content: &str) -> Result<()> {
        Ok(())
    }

    /// 按文件路径查询跨会话发现。
    ///
    /// # Errors
    ///
    /// 文件 I/O 或 JSON 反序列化失败时返回错误。
    fn summaries_by_file(&self, _file_path: &str) -> Result<Vec<EntityRef>> {
        Ok(Vec::new())
    }

    /// 按 CWE 编号查询跨会话发现。
    ///
    /// # Errors
    ///
    /// 文件 I/O 或 JSON 反序列化失败时返回错误。
    fn summaries_by_cwe(&self, _cwe_id: &str) -> Result<Vec<EntityRef>> {
        Ok(Vec::new())
    }
}

// ── L3 Project Memory ──

/// 项目记忆 trait —— 跨会话积累的项目级知识。
pub trait ProjectMemory {
    /// 存储/更新一条项目级事实。
    ///
    /// # Errors
    ///
    /// 文件 I/O 或 JSON 序列化失败时返回错误。
    fn upsert_project_fact(&self, _key: &str, _content: &str) -> Result<()> {
        Ok(())
    }

    /// 读取全部项目事实。
    ///
    /// # Errors
    ///
    /// 文件 I/O 或 JSON 反序列化失败时返回错误。
    fn project_facts(&self) -> Result<Vec<ProjectFact>> {
        Ok(Vec::new())
    }
}

// ── MemoryStore supertrait ──

/// Memory 存储 trait —— 三层记忆的统一入口。
///
/// 自动为同时实现 [`ShortTermMemory`] + [`LongTermMemory`] + [`ProjectMemory`] 的类型提供。
pub trait MemoryStore: ShortTermMemory + LongTermMemory + ProjectMemory {}

impl<T: ShortTermMemory + LongTermMemory + ProjectMemory> MemoryStore for T {}

// ── FileMemoryStore ──

/// 基于文件系统的 Memory 实现。
#[derive(Debug, Clone)]
pub struct FileMemoryStore {
    memory_dir: PathBuf,
}

impl FileMemoryStore {
    #[must_use]
    pub fn new(memory_dir: PathBuf) -> Self {
        Self { memory_dir }
    }

    fn short_term(&self) -> ShortTermStore {
        ShortTermStore::new(&self.memory_dir)
    }

    fn long_term(&self) -> LongTermStore {
        LongTermStore::new(&self.memory_dir)
    }

    fn project_memory(&self) -> ProjectMemoryStore {
        ProjectMemoryStore::new(&self.memory_dir)
    }
}

impl ShortTermMemory for FileMemoryStore {
    fn record_chat(&self, session_id: &str, content: &str, importance: f64) -> Result<()> {
        self.short_term().record(session_id, content, importance)
    }

    fn recent_by_session(&self, session_id: &str, limit: usize) -> Result<Vec<ChatRecord>> {
        self.short_term().recent_by_session(session_id, limit)
    }

    fn clear_short_term(&self, session_id: &str) -> Result<()> {
        self.short_term().clear_session(session_id)
    }
}

impl LongTermMemory for FileMemoryStore {
    fn all_summaries(&self) -> Result<Vec<SessionSummary>> {
        self.long_term().all_summaries()
    }

    fn merge_to_long_term(&self, session_id: &str, findings: &[Finding]) -> Result<()> {
        self.long_term().merge_to_long_term(session_id, findings)
    }

    fn finalize_long_term(&self, session_id: &str, content: &str) -> Result<()> {
        self.long_term().finalize_long_term(session_id, content)
    }

    fn summaries_by_file(&self, file_path: &str) -> Result<Vec<EntityRef>> {
        self.long_term().summaries_by_file(file_path)
    }

    fn summaries_by_cwe(&self, cwe_id: &str) -> Result<Vec<EntityRef>> {
        self.long_term().summaries_by_cwe(cwe_id)
    }
}

impl ProjectMemory for FileMemoryStore {
    fn upsert_project_fact(&self, key: &str, content: &str) -> Result<()> {
        self.project_memory().upsert_fact(key, content)
    }

    fn project_facts(&self) -> Result<Vec<ProjectFact>> {
        self.project_memory().all_facts()
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn three_layer_integration() {
        let temp = TempDir::new().expect("create tempdir");
        let store = FileMemoryStore::new(temp.path().join("memory"));

        // L1: short-term
        store.record_chat("s1", "发现 SQL 注入", 2.0).expect("rec");

        // L2: long-term with findings
        let findings = vec![Finding {
            cwe_id: Some("CWE-89".into()),
            file_path: Some("src/auth.py".into()),
            line: Some(12),
            status: FindingStatus::Fixed,
            timestamp: "2025-06-01T00:00:00Z".into(),
        }];
        store.merge_to_long_term("s1", &findings).expect("merge");
        store
            .finalize_long_term("s1", "审计 auth.py")
            .expect("finalize");

        let by_file = store.summaries_by_file("src/auth.py").expect("query");
        assert_eq!(by_file.len(), 1);
        assert_eq!(by_file[0].cwe_id.as_deref(), Some("CWE-89"));

        // L3: project memory
        store
            .upsert_project_fact("cwe_stats", r#"{"CWE-89": 1}"#)
            .expect("upsert");
        let facts = store.project_facts().expect("read");
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].key, "cwe_stats");
    }

    #[test]
    fn incremental_merge_and_finalize_workflow() {
        let temp = TempDir::new().expect("create tempdir");
        let store = FileMemoryStore::new(temp.path().join("memory"));

        // 模拟 chat() 增量写入 L1 + L2
        store
            .record_chat("s1", "发现 CWE-89 @ a.py", 2.0)
            .expect("L1");
        store
            .merge_to_long_term(
                "s1",
                &[Finding {
                    cwe_id: Some("CWE-89".into()),
                    file_path: Some("a.py".into()),
                    line: None,
                    status: FindingStatus::Pending,
                    timestamp: "t1".into(),
                }],
            )
            .expect("L2 inc 1");

        store
            .record_chat("s1", "发现 CWE-79 @ b.py", 2.0)
            .expect("L1");
        store
            .merge_to_long_term(
                "s1",
                &[Finding {
                    cwe_id: Some("CWE-79".into()),
                    file_path: Some("b.py".into()),
                    line: None,
                    status: FindingStatus::Fixed,
                    timestamp: "t2".into(),
                }],
            )
            .expect("L2 inc 2");

        // 模拟 finalize_session：拼接 L1 为 session content
        let records = store.recent_by_session("s1", usize::MAX).expect("read L1");
        let content: Vec<_> = records.iter().map(|r| r.content.as_str()).collect();
        let summary = content.join("\n");
        store.finalize_long_term("s1", &summary).expect("finalize");

        // 验证 L2
        let all = store.all_summaries().expect("read L2");
        assert_eq!(all.len(), 1);
        assert!(all[0].content.contains("CWE-89"));
        assert!(all[0].content.contains("CWE-79"));
        assert_eq!(all[0].findings.len(), 2);

        // 验证 entity index
        assert_eq!(store.summaries_by_file("a.py").expect("query").len(), 1);
        assert_eq!(store.summaries_by_file("b.py").expect("query").len(), 1);
    }
}
