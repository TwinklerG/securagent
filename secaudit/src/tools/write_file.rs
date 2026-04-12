//! 文件写入工具 — 创建或覆写文件，用于生成修复补丁、PoC 或审计报告。

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::fs;

use crate::error::Error;
use crate::tools::{ConfirmFn, Tool};

// —— 工具元信息 ——

const TOOL_NAME: &str = "write_file";
const TOOL_DESC: &str = "写入或创建文件，可用于生成修复补丁、PoC 或报告";

// —— 参数字段名 ——

const PARAM_PATH: &str = "path";
const PARAM_CONTENT: &str = "content";

// —— 提示消息 ——

const MSG_MISSING_PATH: &str = "缺少 path 参数";
const MSG_MISSING_CONTENT: &str = "缺少 content 参数";
const MSG_OUTSIDE_SANDBOX: &str = "文件路径超出工作目录范围，禁止写入";
const MSG_USER_DENIED: &str = "用户拒绝覆写文件";

/// 文件写入工具，支持沙箱路径校验与覆写确认。
pub struct WriteFile {
    /// 沙箱工作目录
    work_dir: PathBuf,
    /// 用户确认回调
    confirm: ConfirmFn,
}

impl WriteFile {
    /// 创建实例。
    pub fn new(work_dir: PathBuf, confirm: ConfirmFn) -> Self {
        Self { work_dir, confirm }
    }
}

/// 校验目标路径是否在沙箱工作目录内。
fn validate_sandbox(target: &Path, work_dir: &Path) -> Result<(), Error> {
    let parent = target.parent().ok_or_else(|| {
        Error::Tool(format!("{MSG_OUTSIDE_SANDBOX}：无法获取父目录"))
    })?;

    // 父目录可能尚不存在，逐级向上查找已存在的祖先目录进行 canonicalize
    let canonical_base = find_existing_ancestor(parent)
        .and_then(|p| p.canonicalize().ok())
        .ok_or_else(|| {
            Error::Tool(format!("{MSG_OUTSIDE_SANDBOX}：无法解析路径"))
        })?;

    let canonical_work = work_dir.canonicalize().map_err(|e| {
        Error::Tool(format!("无法解析工作目录：{e}"))
    })?;

    if !canonical_base.starts_with(&canonical_work) {
        return Err(Error::Tool(format!(
            "{MSG_OUTSIDE_SANDBOX}：{} 不在 {} 内",
            canonical_base.display(),
            canonical_work.display()
        )));
    }

    Ok(())
}

/// 沿路径向上查找第一个已存在的祖先目录。
fn find_existing_ancestor(path: &Path) -> Option<PathBuf> {
    let mut current = path.to_path_buf();
    loop {
        if current.exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

#[async_trait]
impl Tool for WriteFile {
    fn name(&self) -> &'static str {
        TOOL_NAME
    }

    fn description(&self) -> &'static str {
        TOOL_DESC
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                PARAM_PATH: {
                    "type": "string",
                    "description": "文件路径"
                },
                PARAM_CONTENT: {
                    "type": "string",
                    "description": "文件内容"
                }
            },
            "required": [PARAM_PATH, PARAM_CONTENT]
        })
    }

    async fn execute(&self, params: &Value) -> Result<String, Error> {
        let path_str = params
            .get(PARAM_PATH)
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Tool(MSG_MISSING_PATH.into()))?;

        let content = params
            .get(PARAM_CONTENT)
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Tool(MSG_MISSING_CONTENT.into()))?;

        // 解析为绝对路径（相对路径基于工作目录）
        let target = if Path::new(path_str).is_absolute() {
            PathBuf::from(path_str)
        } else {
            self.work_dir.join(path_str)
        };

        // 沙箱校验
        validate_sandbox(&target, &self.work_dir)?;

        // 文件已存在时需用户确认覆写
        if target.exists() {
            let prompt = format!("文件 {} 已存在，是否覆写？", target.display());
            if !(self.confirm)(&prompt) {
                return Err(Error::Tool(MSG_USER_DENIED.into()));
            }
        }

        // 创建父目录
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).await.map_err(|e| {
                Error::Tool(format!("创建目录��败：{e}"))
            })?;
        }

        // 写入文件
        let bytes_written = content.len();
        fs::write(&target, content).await.map_err(|e| {
            Error::Tool(format!("写入文件失败：{e}"))
        })?;

        Ok(format!(
            "文件已写入：{}（{bytes_written} 字节）",
            target.display()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_existing_ancestor() {
        // 根目录一定存在
        let ancestor = find_existing_ancestor(Path::new("/tmp/a/b/c"));
        assert!(ancestor.is_some(), "应能找到已存在的祖先目录");
    }
}
