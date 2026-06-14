//! 长期记忆 —— 每个会话的压缩摘要 + 结构化发现 + 实体索引。

use std::fmt::{self, Display, Formatter};
use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::result::Result as StdResult;
use std::str::FromStr;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::error::Result;

const LONG_TERM_DIR: &str = "long_term";
const SESSIONS_SUBDIR: &str = "sessions";
const ENTITIES_SUBDIR: &str = "entities";
const LEGACY_SUMMARIES_FILE: &str = "summaries.json";

// ── 结构化发现类型 ──

/// 发现状态。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum FindingStatus {
    #[serde(rename = "fixed")]
    Fixed,
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "confirmed")]
    Confirmed,
    #[serde(rename = "false_positive")]
    FalsePositive,
    #[serde(rename = "needs_analysis")]
    NeedsAnalysis,
}

impl Display for FindingStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Fixed => "已修复",
            Self::Pending => "待修复",
            Self::Confirmed => "已确认",
            Self::FalsePositive => "误报",
            Self::NeedsAnalysis => "需分析",
        };
        f.write_str(s)
    }
}

impl FromStr for FindingStatus {
    type Err = String;

    fn from_str(s: &str) -> StdResult<Self, Self::Err> {
        match s {
            "fixed" => Ok(Self::Fixed),
            "confirmed" => Ok(Self::Confirmed),
            "false_positive" => Ok(Self::FalsePositive),
            "needs_analysis" => Ok(Self::NeedsAnalysis),
            _ => Ok(Self::Pending),
        }
    }
}

/// 一条结构化漏洞发现。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    /// CWE 编号，如 "CWE-89"。
    pub cwe_id: Option<String>,
    /// 漏洞所在文件路径。
    pub file_path: Option<String>,
    /// 行号。
    pub line: Option<u32>,
    /// 发现状态。
    pub status: FindingStatus,
    /// 记录时间。
    pub timestamp: String,
}

impl Finding {
    fn primary_key(&self) -> Option<(String, String, Option<u32>)> {
        match (&self.cwe_id, &self.file_path) {
            (Some(cwe), Some(file)) => Some((cwe.clone(), file.clone(), self.line)),
            (Some(cwe), None) => Some((cwe.clone(), String::new(), self.line)),
            (None, Some(file)) => Some((String::new(), file.clone(), self.line)),
            (None, None) => None,
        }
    }
}

// ── 实体索引类型 ──

/// 实体引用：从某个 session 的某个 finding 指向一个实体。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityRef {
    pub session_id: String,
    pub finding_index: usize,
    pub cwe_id: Option<String>,
    pub status: FindingStatus,
    pub timestamp: String,
}

/// 实体索引文件内容。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityIndex {
    pub entity_type: String,
    pub entity_key: String,
    pub references: Vec<EntityRef>,
}

// ── 会话摘要 ──

/// 一个会话的压缩摘要。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    /// 会话 ID。
    pub session_id: String,
    /// LLM 生成的会话总结。
    pub content: String,
    /// 创建时间。
    pub created_at: String,
    /// 结构化发现列表。
    #[serde(default)]
    pub findings: Vec<Finding>,
}

// ── LongTermStore ──

pub(crate) struct LongTermStore {
    base_dir: PathBuf,
}

impl LongTermStore {
    pub(crate) fn new(memory_dir: &Path) -> Self {
        let base_dir = memory_dir.join(LONG_TERM_DIR);
        let store = Self { base_dir };
        store.migrate_if_needed();
        store
    }

    // ── Migration ──

    fn migrate_if_needed(&self) {
        let legacy_path = self.base_dir.join(LEGACY_SUMMARIES_FILE);
        if !legacy_path.exists() {
            return;
        }

        let result: StdResult<(), String> = (|| {
            let file = File::open(&legacy_path).map_err(|e| format!("{e}"))?;
            let reader = BufReader::new(file);
            let summaries: Vec<SessionSummary> =
                serde_json::from_reader(reader).map_err(|e| format!("{e}"))?;

            for summary in &summaries {
                if let Err(e) = self.write_session_file(summary) {
                    tracing::warn!("[memory] 迁移会话 {} 失败: {e}", summary.session_id);
                }
            }

            fs::remove_file(&legacy_path).map_err(|e| format!("{e}"))?;
            tracing::info!(
                "[memory] 已从 {} 迁移 {} 条长期记忆到 per-session 文件",
                legacy_path.display(),
                summaries.len()
            );
            Ok(())
        })();

        if let Err(e) = result {
            tracing::warn!("[memory] 迁移长期记忆失败，保留旧文件: {e}");
        }
    }

    // ── Paths ──

    fn sessions_dir(&self) -> PathBuf {
        self.base_dir.join(SESSIONS_SUBDIR)
    }

    fn entities_dir(&self) -> PathBuf {
        self.base_dir.join(ENTITIES_SUBDIR)
    }

    fn session_file(&self, session_id: &str) -> PathBuf {
        self.sessions_dir().join(format!("{session_id}.json"))
    }

    fn entity_index_file(&self, entity_type: &str, entity_key: &str) -> PathBuf {
        let encoded = encode_entity_key(entity_key);
        self.entities_dir()
            .join(format!("{entity_type}_{encoded}.json"))
    }

    // ── Public API ──

    /// 读取全部会话摘要（最新在前）。
    pub(crate) fn all_summaries(&self) -> Result<Vec<SessionSummary>> {
        let dir = self.sessions_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }

        let mut summaries = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    tracing::warn!("[memory] 跳过无法读取的 long-term 目录条目: {e}");
                    continue;
                }
            };
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            match Self::read_session_file(&path) {
                Ok(summary) => summaries.push(summary),
                Err(e) => {
                    tracing::warn!("[memory] 跳过损坏的长期记忆文件 {}: {e}", path.display());
                }
            }
        }

        summaries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(summaries)
    }

    // ── Incremental L2 update ──

    /// 增量合并 finding 到 L2，每次 `chat()` 后调用。
    ///
    /// 注意：此方法每次调用会写入 session 文件 + N 个 entity index 文件（N = findings 数）。
    /// 高频调用时产生大量小文件 IO，适合低频审计场景。若未来需要高频写入，考虑
    /// 在 `finalize_long_term` 时批量写入 entity index。
    pub(crate) fn merge_to_long_term(
        &self,
        session_id: &str,
        new_findings: &[Finding],
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let existing = self.load_session(session_id).unwrap_or(None);
        let old_findings = existing
            .as_ref()
            .map(|s| s.findings.clone())
            .unwrap_or_default();

        let merged = Self::merge_findings(old_findings, new_findings);

        let mut summary = existing.unwrap_or(SessionSummary {
            session_id: session_id.into(),
            content: String::new(),
            created_at: now,
            findings: Vec::new(),
        });
        summary.findings = merged;

        self.write_session_file(&summary)?;
        self.update_entity_indexes(&summary)?;
        Ok(())
    }

    /// 最终化 L2：写入 session 文本总结。
    pub(crate) fn finalize_long_term(&self, session_id: &str, content: &str) -> Result<()> {
        let mut summary = self.load_session(session_id)?.unwrap_or(SessionSummary {
            session_id: session_id.into(),
            content: String::new(),
            created_at: Utc::now().to_rfc3339(),
            findings: Vec::new(),
        });
        summary.content = content.into();
        self.write_session_file(&summary)?;
        // entity indexes already up-to-date from merge_to_long_term
        Ok(())
    }

    // ── Entity queries ──

    /// 查询指定文件的历史发现（跨会话）。
    pub(crate) fn summaries_by_file(&self, file_path: &str) -> Result<Vec<EntityRef>> {
        self.read_entity_index("file", file_path)
    }

    /// 查询指定 CWE 的历史发现（跨会话）。
    pub(crate) fn summaries_by_cwe(&self, cwe_id: &str) -> Result<Vec<EntityRef>> {
        self.read_entity_index("cwe", cwe_id)
    }

    // ── Merge logic (Mem0-style) ──

    /// 合并新旧发现：
    /// - 新发现无匹配 old → ADD
    /// - 匹配 + 同状态 → NOOP（保留旧记录）
    /// - 匹配 + 不同状态 → UPDATE（更新 status + timestamp）
    fn merge_findings(old: Vec<Finding>, new: &[Finding]) -> Vec<Finding> {
        let mut merged: Vec<Finding> = old;
        let now_str = Utc::now().to_rfc3339();

        for new_finding in new {
            let key = new_finding.primary_key();
            let matched = key.as_ref().and_then(|(cwe, file, line)| {
                merged.iter_mut().find(|f| {
                    f.primary_key()
                        .as_ref()
                        .is_some_and(|(oc, of, ol)| oc == cwe && of == file && ol == line)
                })
            });

            match matched {
                None => {
                    // ADD
                    merged.push(new_finding.clone());
                }
                Some(existing) => {
                    if existing.status != new_finding.status {
                        // UPDATE
                        existing.status = new_finding.status.clone();
                        existing.timestamp.clone_from(&now_str);
                        existing.line = new_finding.line;
                    }
                    // else: NOOP — keep old record as-is
                }
            }
        }

        merged
    }

    // ── Entity index management ──

    fn update_entity_indexes(&self, summary: &SessionSummary) -> Result<()> {
        // Clear old entity refs for this session
        self.remove_session_from_entities(&summary.session_id)?;

        for (i, finding) in summary.findings.iter().enumerate() {
            let entity_ref = EntityRef {
                session_id: summary.session_id.clone(),
                finding_index: i,
                cwe_id: finding.cwe_id.clone(),
                status: finding.status.clone(),
                timestamp: finding.timestamp.clone(),
            };

            if let Some(cwe) = &finding.cwe_id {
                self.upsert_entity_ref("cwe", cwe, &entity_ref)?;
            }
            if let Some(file) = &finding.file_path {
                self.upsert_entity_ref("file", file, &entity_ref)?;
            }
        }
        Ok(())
    }

    fn remove_session_from_entities(&self, session_id: &str) -> Result<()> {
        let dir = self.entities_dir();
        if !dir.exists() {
            return Ok(());
        }
        for entry in fs::read_dir(dir)? {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    tracing::warn!("[memory] 跳过无法读取的实体索引目录条目: {e}");
                    continue;
                }
            };
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let mut index = match Self::read_entity_index_file(&path) {
                Ok(idx) => idx,
                Err(e) => {
                    tracing::warn!("[memory] 跳过损坏的实体索引文件 {}: {e}", path.display());
                    continue;
                }
            };
            let before = index.references.len();
            index.references.retain(|r| r.session_id != session_id);
            if index.references.is_empty() {
                fs::remove_file(&path)?;
            } else if index.references.len() != before {
                self.write_entity_index_file(&path, &index)?;
            }
        }
        Ok(())
    }

    fn upsert_entity_ref(
        &self,
        entity_type: &str,
        entity_key: &str,
        ref_to_add: &EntityRef,
    ) -> Result<()> {
        let file = self.entity_index_file(entity_type, entity_key);
        let mut index = if file.exists() {
            match Self::read_entity_index_file(&file) {
                Ok(idx) => idx,
                Err(e) => {
                    tracing::warn!("[memory] entity index 损坏 {}: {e}", file.display());
                    return Err(e);
                }
            }
        } else {
            EntityIndex {
                entity_type: entity_type.into(),
                entity_key: entity_key.into(),
                references: Vec::new(),
            }
        };

        // NOOP if same session + same finding_index already exists
        let exists = index.references.iter().any(|r| {
            r.session_id == ref_to_add.session_id && r.finding_index == ref_to_add.finding_index
        });
        if !exists {
            index.references.push(ref_to_add.clone());
        }

        self.write_entity_index_file(&file, &index)
    }

    fn read_entity_index(&self, entity_type: &str, entity_key: &str) -> Result<Vec<EntityRef>> {
        let file = self.entity_index_file(entity_type, entity_key);
        if !file.exists() {
            return Ok(Vec::new());
        }
        let index = Self::read_entity_index_file(&file)?;
        Ok(index.references)
    }

    // ── Internal helpers ──

    fn load_session(&self, session_id: &str) -> Result<Option<SessionSummary>> {
        let file = self.session_file(session_id);
        if !file.exists() {
            return Ok(None);
        }
        Self::read_session_file(&file).map(Some)
    }

    fn read_session_file(path: &Path) -> Result<SessionSummary> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        Ok(serde_json::from_reader(reader)?)
    }

    fn write_session_file(&self, summary: &SessionSummary) -> Result<()> {
        let dir = self.sessions_dir();
        fs::create_dir_all(dir)?;

        let file_path = self.session_file(&summary.session_id);
        let tmp_path = file_path.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(summary)?;
        fs::write(&tmp_path, json)?;
        fs::rename(&tmp_path, &file_path)?;
        Ok(())
    }

    fn read_entity_index_file(path: &Path) -> Result<EntityIndex> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        Ok(serde_json::from_reader(reader)?)
    }

    fn write_entity_index_file(&self, path: &Path, index: &EntityIndex) -> Result<()> {
        let dir = self.entities_dir();
        fs::create_dir_all(dir)?;

        let tmp_path = path.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(index)?;
        fs::write(&tmp_path, json)?;
        fs::rename(&tmp_path, path)?;
        Ok(())
    }
}

/// 编码 entity key 为文件系统安全文件名。
/// 超长 key（>240 字节编码后）使用前 200 字符 + hash 截断。
fn encode_entity_key(key: &str) -> String {
    let encoded: String = key
        .bytes()
        .map(|byte| {
            let ch = byte as char;
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch.to_string()
            } else {
                format!("_{byte:02x}_")
            }
        })
        .collect::<String>();

    if encoded.len() <= 240 {
        return encoded;
    }

    // 超长 key：取前 200 字符 + FNV-1a hash 后缀
    let hash = key
        .bytes()
        .fold(0xcbf2u32, |h, b| h.wrapping_mul(0x0100_0193) ^ u32::from(b));
    format!("{}_{:08x}", &encoded[..200], hash)
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    fn make_finding(cwe: &str, file: &str, status: FindingStatus) -> Finding {
        Finding {
            cwe_id: Some(cwe.into()),
            file_path: Some(file.into()),
            line: None,
            status,
            timestamp: "2025-06-01T00:00:00Z".into(),
        }
    }

    // ── merge_findings tests ──

    #[test]
    fn merge_add_new_finding() {
        let old = vec![make_finding("CWE-89", "a.py", FindingStatus::Fixed)];
        let new = vec![make_finding("CWE-79", "b.py", FindingStatus::Pending)];
        let merged = LongTermStore::merge_findings(old, &new);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn merge_update_changed_status() {
        let old = vec![make_finding("CWE-89", "a.py", FindingStatus::Pending)];
        let new = vec![make_finding("CWE-89", "a.py", FindingStatus::Fixed)];
        let merged = LongTermStore::merge_findings(old, &new);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].status, FindingStatus::Fixed);
    }

    #[test]
    fn merge_noop_same_status() {
        let old = vec![make_finding("CWE-89", "a.py", FindingStatus::Fixed)];
        let new = vec![make_finding("CWE-89", "a.py", FindingStatus::Fixed)];
        let merged = LongTermStore::merge_findings(old, &new);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].status, FindingStatus::Fixed);
    }

    #[test]
    fn merge_keeps_same_cwe_file_on_different_lines() {
        let mut old_finding = make_finding("CWE-89", "a.py", FindingStatus::Pending);
        old_finding.line = Some(10);
        let mut new_finding = make_finding("CWE-89", "a.py", FindingStatus::Pending);
        new_finding.line = Some(20);

        let merged = LongTermStore::merge_findings(vec![old_finding], &[new_finding]);

        assert_eq!(merged.len(), 2);
        assert!(merged.iter().any(|finding| finding.line == Some(10)));
        assert!(merged.iter().any(|finding| finding.line == Some(20)));
    }

    #[test]
    fn merge_matches_partial_key_cwe_only() {
        let old = vec![Finding {
            cwe_id: Some("CWE-89".into()),
            file_path: None,
            line: None,
            status: FindingStatus::Pending,
            timestamp: "t1".into(),
        }];
        // same CWE, no file → should UPDATE, not ADD
        let new = vec![Finding {
            cwe_id: Some("CWE-89".into()),
            file_path: None,
            line: None,
            status: FindingStatus::Fixed,
            timestamp: "t2".into(),
        }];
        let merged = LongTermStore::merge_findings(old, &new);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].status, FindingStatus::Fixed);
    }

    #[test]
    fn merge_matches_partial_key_file_only() {
        let old = vec![Finding {
            cwe_id: None,
            file_path: Some("a.py".into()),
            line: Some(10),
            status: FindingStatus::Pending,
            timestamp: "t1".into(),
        }];
        let new = vec![Finding {
            cwe_id: None,
            file_path: Some("a.py".into()),
            line: Some(10),
            status: FindingStatus::Confirmed,
            timestamp: "t2".into(),
        }];
        let merged = LongTermStore::merge_findings(old, &new);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].status, FindingStatus::Confirmed);
    }

    #[test]
    fn merge_both_add_and_update() {
        let old = vec![make_finding("CWE-89", "a.py", FindingStatus::Pending)];
        let new = vec![
            make_finding("CWE-89", "a.py", FindingStatus::Fixed), // UPDATE
            make_finding("CWE-79", "b.py", FindingStatus::Pending), // ADD
        ];
        let merged = LongTermStore::merge_findings(old, &new);
        assert_eq!(merged.len(), 2);
        let cwe89 = merged
            .iter()
            .find(|f| f.cwe_id.as_deref() == Some("CWE-89"))
            .unwrap();
        assert_eq!(cwe89.status, FindingStatus::Fixed);
        assert!(merged.iter().any(|f| f.cwe_id.as_deref() == Some("CWE-79")));
    }

    // ── Entity index tests ──

    #[test]
    fn entity_index_crud() {
        let temp = TempDir::new().expect("create tempdir");
        let store = LongTermStore::new(&temp.path().join("memory"));

        let findings = vec![make_finding("CWE-89", "src/auth.py", FindingStatus::Fixed)];
        store.merge_to_long_term("s1", &findings).expect("merge");

        let by_file = store.summaries_by_file("src/auth.py").expect("query file");
        assert_eq!(by_file.len(), 1);
        assert_eq!(by_file[0].cwe_id.as_deref(), Some("CWE-89"));

        let by_cwe = store.summaries_by_cwe("CWE-89").expect("query cwe");
        assert_eq!(by_cwe.len(), 1);
        assert_eq!(by_cwe[0].session_id, "s1");
    }

    #[test]
    fn entity_index_file_deleted_when_all_refs_removed() {
        let temp = TempDir::new().expect("create tempdir");
        let store = LongTermStore::new(&temp.path().join("memory"));

        let findings = vec![make_finding("CWE-89", "a.py", FindingStatus::Fixed)];
        store.merge_to_long_term("s1", &findings).expect("merge");

        // 验证 entity index 文件存在
        let entity_file = store.entity_index_file("file", "a.py");
        assert!(entity_file.exists());

        // 清除该 session 的所有 entity refs
        store.remove_session_from_entities("s1").expect("remove");

        // entity index 文件应被删除
        assert!(!entity_file.exists(), "空 entity index 文件应被删除");
    }

    #[test]
    fn entity_index_query_returns_empty_for_unknown() {
        let temp = TempDir::new().expect("create tempdir");
        let store = LongTermStore::new(&temp.path().join("memory"));

        let refs = store.summaries_by_file("nonexistent.py").expect("query");
        assert!(refs.is_empty());
    }

    // ── Session summary tests ──

    #[test]
    fn upsert_and_read_summaries() {
        let temp = TempDir::new().expect("create tempdir");
        let store = LongTermStore::new(&temp.path().join("memory"));

        store
            .finalize_long_term("s1", "审计了 auth.py")
            .expect("finalize");
        store
            .finalize_long_term("s2", "审计了 utils.py")
            .expect("finalize");

        let all = store.all_summaries().expect("read");
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn upsert_same_session_updates() {
        let temp = TempDir::new().expect("create tempdir");
        let store = LongTermStore::new(&temp.path().join("memory"));

        store.finalize_long_term("s1", "第一轮").expect("finalize");
        store
            .finalize_long_term("s1", "第二轮覆盖")
            .expect("finalize");

        let all = store.all_summaries().expect("read");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].content, "第二轮覆盖");
    }

    #[test]
    fn empty_store_returns_empty() {
        let temp = TempDir::new().expect("create tempdir");
        let store = LongTermStore::new(&temp.path().join("memory"));
        assert!(store.all_summaries().expect("read").is_empty());
    }

    #[test]
    fn migration_from_legacy_json() {
        let temp = TempDir::new().expect("create tempdir");
        let memory_dir = temp.path().join("memory");

        let long_term_dir = memory_dir.join(LONG_TERM_DIR);
        fs::create_dir_all(&long_term_dir).expect("create dir");
        let legacy = SessionSummary {
            session_id: "old-session".into(),
            content: "旧摘要".into(),
            created_at: "2025-01-01T00:00:00Z".into(),
            findings: vec![],
        };
        let json = serde_json::to_string_pretty(&vec![legacy]).expect("serialize");
        fs::write(long_term_dir.join(LEGACY_SUMMARIES_FILE), json).expect("write legacy");

        let store = LongTermStore::new(&memory_dir);
        let all = store.all_summaries().expect("read");

        assert_eq!(all.len(), 1);
        assert_eq!(all[0].content, "旧摘要");
        assert!(
            !long_term_dir.join(LEGACY_SUMMARIES_FILE).exists(),
            "旧文件应被删除"
        );
        assert!(
            long_term_dir
                .join(SESSIONS_SUBDIR)
                .join("old-session.json")
                .exists(),
            "新 per-session 文件应存在"
        );
    }

    // ── merge_to_long_term tests ──

    #[test]
    fn incremental_merge_accumulates_findings() {
        let temp = TempDir::new().expect("create tempdir");
        let store = LongTermStore::new(&temp.path().join("memory"));

        // 第一次增量写入
        store
            .merge_to_long_term(
                "s1",
                &[make_finding("CWE-89", "a.py", FindingStatus::Pending)],
            )
            .expect("merge 1");

        // 第二次增量写入
        store
            .merge_to_long_term(
                "s1",
                &[make_finding("CWE-79", "b.py", FindingStatus::Fixed)],
            )
            .expect("merge 2");

        // L2 应该累积两个 finding
        let all = store.all_summaries().expect("read");
        assert_eq!(all.len(), 1, "同一 session 不应新增文件");
        assert_eq!(all[0].findings.len(), 2, "两个 finding 都应存在");
    }

    #[test]
    fn incremental_merge_update_noop_works() {
        let temp = TempDir::new().expect("create tempdir");
        let store = LongTermStore::new(&temp.path().join("memory"));

        // 写入 pending
        store
            .merge_to_long_term(
                "s1",
                &[make_finding("CWE-89", "a.py", FindingStatus::Pending)],
            )
            .expect("merge 1");

        // 相同 finding，NOOP
        store
            .merge_to_long_term(
                "s1",
                &[make_finding("CWE-89", "a.py", FindingStatus::Pending)],
            )
            .expect("merge 2");
        assert_eq!(
            store.all_summaries().expect("read")[0].findings.len(),
            1,
            "NOOP 不应新增"
        );

        // UPDATE 状态
        store
            .merge_to_long_term(
                "s1",
                &[make_finding("CWE-89", "a.py", FindingStatus::Fixed)],
            )
            .expect("merge 3");
        assert_eq!(
            store.all_summaries().expect("read")[0].findings[0].status,
            FindingStatus::Fixed,
            "状态应更新为 Fixed"
        );
    }

    #[test]
    fn incremental_merge_builds_entity_index() {
        let temp = TempDir::new().expect("create tempdir");
        let store = LongTermStore::new(&temp.path().join("memory"));

        store
            .merge_to_long_term(
                "s1",
                &[make_finding("CWE-89", "a.py", FindingStatus::Fixed)],
            )
            .expect("merge");

        let by_file = store.summaries_by_file("a.py").expect("query");
        assert_eq!(by_file.len(), 1);
        assert_eq!(by_file[0].cwe_id.as_deref(), Some("CWE-89"));
    }

    // ── finalize_long_term tests ──

    #[test]
    fn finalize_writes_content_preserves_findings() {
        let temp = TempDir::new().expect("create tempdir");
        let store = LongTermStore::new(&temp.path().join("memory"));

        store
            .merge_to_long_term(
                "s1",
                &[make_finding("CWE-89", "a.py", FindingStatus::Fixed)],
            )
            .expect("merge");

        store
            .finalize_long_term("s1", "本次审计了 a.py，发现 CWE-89 已修复")
            .expect("finalize");

        let all = store.all_summaries().expect("read");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].content, "本次审计了 a.py，发现 CWE-89 已修复");
        assert_eq!(all[0].findings.len(), 1, "findings 不应丢失");
    }

    #[test]
    fn finalize_without_merge_creates_minimal_record() {
        let temp = TempDir::new().expect("create tempdir");
        let store = LongTermStore::new(&temp.path().join("memory"));

        store.finalize_long_term("s1", "空审计").expect("finalize");

        let all = store.all_summaries().expect("read");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].content, "空审计");
        assert!(all[0].findings.is_empty());
    }

    // ── encode_entity_key tests ──

    #[test]
    fn encode_normal_key_unchanged() {
        assert_eq!(encode_entity_key("CWE-89"), "CWE-89");
        assert_eq!(encode_entity_key("src_auth_py"), "src_auth_py");
    }

    #[test]
    fn encode_special_chars() {
        let encoded = encode_entity_key("src/auth.py");
        assert!(!encoded.contains('/'), "路径分隔符应被编码");
        assert!(!encoded.contains('.'), "点应被编码");
        assert!(encoded.starts_with("src"), "普通字符不变");
    }

    #[test]
    fn encode_non_ascii_uses_utf8_bytes() {
        assert_eq!(encode_entity_key("Ā"), "_c4__80_");
        assert_ne!(encode_entity_key("Ā"), encode_entity_key("\0"));
    }

    #[test]
    fn encode_roundtrip_does_not_need_decode() {
        // entity key 仅用作文件查找键，不需要可逆解码
        let a = encode_entity_key("src/auth.py");
        let b = encode_entity_key("src/auth.py");
        assert_eq!(a, b, "相同输入产生相同编码");
        let c = encode_entity_key("src\\auth.py");
        assert_ne!(a, c, "不同路径不应冲突");
    }
}
