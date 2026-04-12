//! 工具共享模块 — 沙箱路径校验与二进制文件检测。

use std::path::{Path, PathBuf};

use tokio::fs;

use crate::error::Error;

// —— 提示消息 ——

/// 路径解析失败模板
const MSG_PATH_RESOLVE_FAIL: &str = "路径解析失败";
/// 工作目录解析失败提示
const MSG_WORK_DIR_RESOLVE_FAIL: &str = "工作目录解析失败";
/// 超出沙箱范围提示
const MSG_OUTSIDE_SANDBOX: &str = "路径超出工作目录沙箱范围";

// —— 二进制检测 ——

/// 二进制检测缓冲区大小（字节）
const BINARY_CHECK_SIZE: usize = 8192;

/// 解析路径并校验是否在沙箱工作目录内，返回规范化后的路径。
pub fn resolve_sandbox_path(work_dir: &Path, raw: &str) -> Result<PathBuf, Error> {
    let candidate = if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        work_dir.join(raw)
    };

    let resolved = candidate
        .canonicalize()
        .map_err(|e| Error::Tool(format!("{MSG_PATH_RESOLVE_FAIL}「{raw}」：{e}")))?;

    let sandbox = work_dir
        .canonicalize()
        .map_err(|e| Error::Tool(format!("{MSG_WORK_DIR_RESOLVE_FAIL}：{e}")))?;

    if !resolved.starts_with(&sandbox) {
        return Err(Error::Tool(format!(
            "{MSG_OUTSIDE_SANDBOX}：{}",
            resolved.display()
        )));
    }

    Ok(resolved)
}

/// 异步判断文件是否为二进制文件（前 `BINARY_CHECK_SIZE` 字节中是否含空字节）。
pub async fn is_binary(path: &Path) -> bool {
    fs::read(path)
        .await
        .map(|bytes| {
            let check_len = bytes.len().min(BINARY_CHECK_SIZE);
            bytes
                .get(..check_len)
                .is_some_and(|slice| slice.contains(&0))
        })
        .unwrap_or(true)
}
