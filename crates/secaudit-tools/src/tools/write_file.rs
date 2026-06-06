//! 文件写入工具 — 创建或覆写文件，用于生成修复补丁、PoC 或审计报告。

use std::borrow::Cow;
use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::fs;

use super::sandbox::resolve_writable_path;
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

#[async_trait]
impl Tool for WriteFile {
    fn name(&self) -> Cow<'_, str> {
        Cow::Borrowed(TOOL_NAME)
    }

    fn description(&self) -> Cow<'_, str> {
        Cow::Borrowed(TOOL_DESC)
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

        let target = resolve_writable_path(&self.work_dir, path_str)?;

        // 文件已存在时需用户确认覆写
        if target.exists() {
            let prompt = format!("文件 {} 已存在，是否覆写？", target.display());
            if !(self.confirm)(&prompt) {
                return Err(Error::Tool(MSG_USER_DENIED.into()));
            }
        }

        // 创建父目录
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| Error::Tool(format!("创建目录失败：{e}")))?;
        }

        // 写入文件
        let bytes_written = content.len();
        fs::write(&target, content)
            .await
            .map_err(|e| Error::Tool(format!("写入文件失败：{e}")))?;

        Ok(format!(
            "文件已写入：{}（{bytes_written} 字节）",
            target.display()
        ))
    }
}
