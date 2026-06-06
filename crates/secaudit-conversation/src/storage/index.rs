//! 会话索引 JSONL 的读写、锁和压缩。

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use secaudit_storage::RuntimeLayout;

use crate::error::Result;
use crate::model::SessionMetadata;

const INDEX_COMPACT_MIN_BYTES: u64 = 16 * 1024;
const INDEX_COMPACT_DUPLICATE_RATIO: usize = 2;

struct SessionIndex {
    record_count: usize,
    by_id: BTreeMap<String, SessionMetadata>,
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

pub(super) fn list_sessions(index_file: &Path) -> Result<Vec<SessionMetadata>> {
    Ok(read_session_index(index_file)?.into_sorted_sessions())
}

pub(super) fn append_session(index_file: &Path, metadata: &SessionMetadata) -> Result<()> {
    RuntimeLayout::append_jsonl(index_file, metadata)?;
    compact_session_index_if_needed(index_file)?;
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

fn read_session_index(index_file: &Path) -> Result<SessionIndex> {
    let metadata_items = RuntimeLayout::read_jsonl::<SessionMetadata>(index_file)?;
    let mut record_count = 0usize;
    let mut by_id: BTreeMap<String, SessionMetadata> = BTreeMap::new();

    for metadata in metadata_items {
        record_count += 1;
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
    RuntimeLayout::write_jsonl_atomic(index_file, sessions)?;
    Ok(())
}
