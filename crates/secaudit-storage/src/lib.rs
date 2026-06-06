//! `secaudit-storage` —— 用户级运行时文件系统布局与底层持久化能力。
//!
//! 提供 `RuntimeLayout` 用于构建 `~/.secaudit` 下的目录和文件路径，
//! 以及原子 JSON 写入、JSONL 读写、文件锁等通用持久化原语。
//!
//! 本 crate 不依赖任何其他 secaudit 内部 crate，可作为最底层基础设施使用。

mod error;
mod layout;
mod lock;

pub use error::{Error, Result};
pub use layout::{
    ACTIVE_DIR, ARCHIVED_DIR, INDEX_FILE, LOGS_DIR, MEMORY_DIR, PROJECT_FILE, PROJECTS_DIR,
    RUNTIME_DIR, RuntimeLayout, SESSIONS_DIR, SKILLS_DIR, TOOL_CONFIG_DIR, atomic_tmp_path,
    canonical_work_dir,
};
pub use lock::FileLock;
