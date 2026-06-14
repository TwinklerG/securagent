//! 短时记忆 —— 按会话分文件存储，惰性截断，每文件最多保留最近 `TRUNCATE_THRESHOLD` 条。

use std::cmp::Ordering;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::error::Result;

const SHORT_TERM_DIR: &str = "short_term";
const SUMMARIES_WINDOW: usize = 8;
const TRUNCATE_THRESHOLD: usize = 24;
const RECENT_GUARANTEE: usize = 4;

/// 一轮对话记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRecord {
    pub session_id: String,
    pub created_at: String,
    pub content: String,
    /// 重要性分数 (0.0–5.0)，数值越高越重要，用于截断排序。
    #[serde(default = "default_importance")]
    pub importance: f64,
}

fn default_importance() -> f64 {
    1.0
}

impl ChatRecord {
    #[must_use]
    pub fn new(session_id: String, content: String, importance: f64) -> Self {
        Self {
            session_id,
            created_at: Utc::now().to_rfc3339(),
            content,
            importance,
        }
    }
}

/// 基于重要性 + 最近性截断：保证最近 `RECENT_GUARANTEE` 条不被淘汰，
/// 之后按 importance 降序选择剩余槽位。
fn truncate_by_importance(records: Vec<ChatRecord>) -> Vec<ChatRecord> {
    if records.len() <= SUMMARIES_WINDOW {
        return records;
    }

    let split_at = records.len().saturating_sub(RECENT_GUARANTEE);
    let (older, recent) = records.split_at(split_at);
    let mut keep: Vec<ChatRecord> = recent.to_vec();

    let remaining_slots = SUMMARIES_WINDOW.saturating_sub(RECENT_GUARANTEE);
    if remaining_slots > 0 && !older.is_empty() {
        let mut older_sorted = older.to_vec();
        older_sorted.sort_by(|a, b| {
            b.importance
                .partial_cmp(&a.importance)
                .unwrap_or(Ordering::Equal)
                .then_with(|| b.created_at.cmp(&a.created_at))
        });
        keep.extend(older_sorted.into_iter().take(remaining_slots));
    }
    keep.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    keep
}

pub(crate) struct ShortTermStore {
    base_dir: PathBuf,
}

impl ShortTermStore {
    pub(crate) fn new(memory_dir: &Path) -> Self {
        Self {
            base_dir: memory_dir.join(SHORT_TERM_DIR),
        }
    }

    fn session_file(&self, session_id: &str) -> PathBuf {
        self.base_dir.join(format!("{session_id}.jsonl"))
    }

    /// 清理指定会话的全部短期记忆。
    pub(crate) fn clear_session(&self, session_id: &str) -> Result<()> {
        let path = self.session_file(session_id);
        if path.exists() {
            fs::remove_file(&path)?;
        }
        Ok(())
    }

    /// 追加一条对话记录，超过阈值时惰性截断至最近 `SUMMARIES_WINDOW` 条，
    /// 保证最近 `RECENT_GUARANTEE` 条不被淘汰，其余按 importance 降序保留。
    pub(crate) fn record(&self, session_id: &str, content: &str, importance: f64) -> Result<()> {
        let path = self.session_file(session_id);
        fs::create_dir_all(&self.base_dir)?;
        let record = ChatRecord::new(session_id.into(), content.into(), importance);
        {
            let mut file = File::options().create(true).append(true).open(&path)?;
            let line = serde_json::to_string(&record)?;
            file.write_all(line.as_bytes())?;
            file.write_all(b"\n")?;
            file.sync_all()?;
        }

        // 惰性截断：超过阈值时按重要性+时间加权截断
        let records = self.read_all(session_id)?;
        if records.len() > TRUNCATE_THRESHOLD {
            let trimmed = truncate_by_importance(records);
            self.write_all(session_id, &trimmed)?;
        }
        Ok(())
    }

    /// 读取指定会话最近 N 条记录（时间倒序）。
    pub(crate) fn recent_by_session(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<ChatRecord>> {
        let mut records = self.read_all(session_id)?;
        records.reverse();
        if records.len() > limit {
            records.truncate(limit);
        }
        Ok(records)
    }

    fn read_all(&self, session_id: &str) -> Result<Vec<ChatRecord>> {
        let path = self.session_file(session_id);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let file = File::open(&path)?;
        let reader = BufReader::new(file);
        let records: Vec<ChatRecord> = reader
            .lines()
            .filter_map(|line| match line {
                Ok(l) => Some(l),
                Err(e) => {
                    tracing::warn!("[memory] 跳过无法读取的 JSONL 行: {e}");
                    None
                }
            })
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| {
                serde_json::from_str::<ChatRecord>(&line)
                    .inspect_err(|e| tracing::warn!("[memory] 跳过损坏的 JSONL 行: {e}"))
                    .ok()
            })
            .collect();
        Ok(records)
    }

    fn write_all(&self, session_id: &str, records: &[ChatRecord]) -> Result<()> {
        let path = self.session_file(session_id);
        let tmp = path.with_extension("jsonl.tmp");
        {
            let mut file = File::create(&tmp)?;
            for r in records {
                let line = serde_json::to_string(r)?;
                file.write_all(line.as_bytes())?;
                file.write_all(b"\n")?;
            }
            file.sync_all()?;
        }
        fs::rename(&tmp, &path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn record_and_read_by_session() {
        let temp = TempDir::new().expect("create tempdir");
        let store = ShortTermStore::new(&temp.path().join("memory"));

        store.record("s1", "s1: 发现 SQL 注入", 1.0).expect("rec");
        store.record("s1", "s1: 确认修复", 1.0).expect("rec");

        let by_s1 = store.recent_by_session("s1", 8).expect("rec");
        assert_eq!(by_s1.len(), 2);
        assert_eq!(by_s1[0].content, "s1: 确认修复");
        assert_eq!(by_s1[1].content, "s1: 发现 SQL 注入");
    }

    #[test]
    fn recent_by_session_respects_limit() {
        let temp = TempDir::new().expect("create tempdir");
        let store = ShortTermStore::new(&temp.path().join("memory"));

        for i in 0..12 {
            store.record("s1", &format!("记录 {i}"), 1.0).expect("rec");
        }

        let records = store.recent_by_session("s1", 8).expect("rec");
        assert_eq!(records.len(), 8);
        assert_eq!(records[0].content, "记录 11");
    }

    #[test]
    fn lazy_truncation_triggers_at_threshold() {
        let temp = TempDir::new().expect("create tempdir");
        let store = ShortTermStore::new(&temp.path().join("memory"));

        for i in 0..30 {
            store.record("s1", &format!("记录 {i}"), 1.0).expect("rec");
        }

        // 超过 TRUNCATE_THRESHOLD(24) 时触发重要性截断到 SUMMARIES_WINDOW(8) 条，
        // 后续追加后计数在 8-24 之间（等 importance 全 1.0 时 ≈ 最近 4 条 + 最旧 4 条 + 新增）
        let all = store.read_all("s1").expect("read");
        assert!(all.len() < TRUNCATE_THRESHOLD);
        assert!(all.len() > SUMMARIES_WINDOW);
    }

    #[test]
    fn truncate_by_importance_keeps_high_importance() {
        // 24 条低重要性 + 4 条高重要性 = 28 → 触发截断
        let mut records: Vec<ChatRecord> = (0..24)
            .map(|i| ChatRecord::new("s".into(), format!("low {i}"), 0.1))
            .collect();
        let high: Vec<ChatRecord> = (0..4)
            .map(|i| ChatRecord::new("s".into(), format!("high {i}"), 5.0))
            .collect();
        records.extend(high.clone());

        let result = truncate_by_importance(records);
        assert_eq!(result.len(), SUMMARIES_WINDOW);
        // 最近 4 条（高重要性）全部保留
        for h in &high {
            assert!(result.iter().any(|r| r.content == h.content));
        }
    }

    #[test]
    fn truncation_preserves_chronological_order() {
        // 30 条记录，全部 importance=1.0，截断后最近的那条应在 recent_by_session 的最前面
        let temp = TempDir::new().expect("create tempdir");
        let store = ShortTermStore::new(&temp.path().join("memory"));

        for i in 0..30 {
            store.record("s1", &format!("记录 {i}"), 1.0).expect("rec");
        }

        let recent = store.recent_by_session("s1", 1).expect("rec");
        assert_eq!(recent[0].content, "记录 29", "最新的记录应在最前面");
    }

    #[test]
    fn sessions_are_isolated() {
        let temp = TempDir::new().expect("create tempdir");
        let store = ShortTermStore::new(&temp.path().join("memory"));

        store.record("s1", "s1 的记录", 1.0).expect("rec");
        store.record("s2", "s2 的记录", 1.0).expect("rec");

        assert_eq!(store.recent_by_session("s1", 8).expect("rec").len(), 1);
        assert_eq!(store.recent_by_session("s2", 8).expect("rec").len(), 1);
    }

    #[test]
    fn clear_session_removes_file() {
        let temp = TempDir::new().expect("create tempdir");
        let store = ShortTermStore::new(&temp.path().join("memory"));

        store.record("s1", "内容", 1.0).expect("rec");
        assert!(!store.recent_by_session("s1", 8).expect("read").is_empty());

        store.clear_session("s1").expect("clear");
        assert!(store.recent_by_session("s1", 8).expect("read").is_empty());
    }

    #[test]
    fn empty_session_returns_empty() {
        let temp = TempDir::new().expect("create tempdir");
        let store = ShortTermStore::new(&temp.path().join("memory"));
        assert!(store.recent_by_session("none", 8).expect("rec").is_empty());
    }

    #[test]
    fn truncation_not_triggered_at_exact_threshold() {
        let temp = TempDir::new().expect("create tempdir");
        let store = ShortTermStore::new(&temp.path().join("memory"));

        // TRUNCATE_THRESHOLD = 24，恰好 24 条不触发截断
        for i in 0..24 {
            store.record("s1", &format!("记录 {i}"), 1.0).expect("rec");
        }
        let all = store.read_all("s1").expect("read");
        assert_eq!(all.len(), 24, "恰好阈值不应截断");
    }

    #[test]
    fn truncation_triggers_above_threshold() {
        let temp = TempDir::new().expect("create tempdir");
        let store = ShortTermStore::new(&temp.path().join("memory"));

        // TRUNCATE_THRESHOLD = 24，25 条触发截断到 SUMMARIES_WINDOW = 8
        for i in 0..25 {
            store.record("s1", &format!("记录 {i}"), 1.0).expect("rec");
        }
        let all = store.read_all("s1").expect("read");
        assert_eq!(all.len(), 8, "超过阈值应截断到 SUMMARIES_WINDOW 条");
    }

    #[test]
    fn damaged_jsonl_line_is_skipped() {
        use std::fs;
        use std::io::Write;

        let temp = TempDir::new().expect("create tempdir");
        let store = ShortTermStore::new(&temp.path().join("memory"));

        // 写入一条正常记录
        store.record("s1", "正常记录", 1.0).expect("rec");

        // 手动追加一行损坏的 JSON
        let path = store.session_file("s1");
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open");
        writeln!(file, "这不是合法的 JSON").expect("write corrupt line");

        // 再追加一条正常记录
        store.record("s1", "另一条正常", 1.0).expect("rec");

        let records = store.recent_by_session("s1", 10).expect("read");
        assert_eq!(records.len(), 2, "损坏行应被跳过，不丢失正常记录");
        assert!(records.iter().any(|r| r.content == "正常记录"));
        assert!(records.iter().any(|r| r.content == "另一条正常"));
    }
}
