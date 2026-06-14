//! 工具文件路径沙箱。

use std::path::{Component, Path, PathBuf};

use tokio::fs as async_fs;

use crate::error::Error;

/// 单次 `read_file` 允许读取的最大文件大小。
pub const MAX_READ_FILE_SIZE: u64 = 10 * 1024 * 1024;
/// `search_content` 遍历时单个文件允许读取的最大文件大小。
pub const MAX_SEARCH_FILE_SIZE: u64 = 10 * 1024 * 1024;
/// 单次 `write_file` 允许写入的最大内容大小。
pub const MAX_WRITE_CONTENT_SIZE: usize = 5 * 1024 * 1024;

/// 内置保护列表：常见凭据、密钥和本地环境配置文件名。
///
/// 用户配置只能追加敏感路径规则，不能移除这些默认保护项。
const SENSITIVE_COMPONENTS: &[&str] = &[
    ".env",
    ".ssh",
    ".gnupg",
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SensitivePathPolicyConfig {
    pub blocklist: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SensitivePathPolicy {
    config: SensitivePathPolicyConfig,
}

impl SensitivePathPolicy {
    #[must_use]
    pub fn new(config: SensitivePathPolicyConfig) -> Self {
        Self { config }
    }

    #[must_use]
    pub fn has_sensitive_component(&self, path: &Path) -> bool {
        path.components().any(|component| {
            let name = component.as_os_str().to_string_lossy();
            is_sensitive_component_name(&name) || self.matches_user_rule(path, &name)
        })
    }

    fn matches_user_rule(&self, path: &Path, component_name: &str) -> bool {
        let component = component_name.to_ascii_lowercase();
        let path = path.to_string_lossy().to_ascii_lowercase();
        let normalized_path = path.replace('\\', "/");
        self.config.blocklist.iter().any(|rule| {
            let rule = rule.trim().to_ascii_lowercase();
            if rule.is_empty() {
                return false;
            }
            if rule.contains('/') || rule.contains('\\') {
                normalized_path.contains(&rule.replace('\\', "/"))
            } else {
                component == rule
                    || component.starts_with(&format!("{rule}."))
                    || component.ends_with(&format!(".{rule}"))
            }
        })
    }
}

/// 路径解析失败模板
const MSG_PATH_RESOLVE_FAIL: &str = "路径解析失败";
/// 工作目录解析失败提示
const MSG_WORK_DIR_RESOLVE_FAIL: &str = "工作目录解析失败";
/// 超出沙箱范围提示
const MSG_OUTSIDE_SANDBOX: &str = "路径超出工作目录沙箱范围";
/// 写入目标超出沙箱范围提示
const MSG_WRITE_OUTSIDE_SANDBOX: &str = "文件路径超出工作目录范围，禁止写入";

/// 解析已存在路径并校验是否在工作目录沙箱内。
///
/// 读取、列目录、搜索等只读工具使用该入口；目标路径必须已经存在。
pub fn resolve_existing_path(work_dir: &Path, raw: &str) -> Result<PathBuf, Error> {
    let candidate = path_from_user_input(work_dir, raw);
    let resolved = candidate
        .canonicalize()
        .map_err(|e| Error::Tool(format!("{MSG_PATH_RESOLVE_FAIL}「{raw}」：{e}")))?;

    ensure_inside_work_dir(&resolved, work_dir, MSG_OUTSIDE_SANDBOX)?;
    Ok(resolved)
}

/// 解析可写目标路径并校验其已存在祖先目录位于工作目录沙箱内。
///
/// 写入工具允许目标文件或父目录尚不存在，因此不能直接 canonicalize
/// 目标路径，只能校验最近的已存在祖先目录。
pub fn resolve_writable_path(work_dir: &Path, raw: &str) -> Result<PathBuf, Error> {
    let sandbox = canonicalize_work_dir(work_dir)?;
    let target = lexical_normalize(&path_from_user_input(&sandbox, raw));

    ensure_inside_work_dir(&target, &sandbox, MSG_WRITE_OUTSIDE_SANDBOX)?;

    if target.symlink_metadata().is_ok() {
        let resolved_target = target
            .canonicalize()
            .map_err(|e| Error::Tool(format!("{MSG_WRITE_OUTSIDE_SANDBOX}：无法解析目标：{e}")))?;
        ensure_inside_work_dir(&resolved_target, &sandbox, MSG_WRITE_OUTSIDE_SANDBOX)?;
        return Ok(resolved_target);
    }

    let parent = target
        .parent()
        .ok_or_else(|| Error::Tool(format!("{MSG_WRITE_OUTSIDE_SANDBOX}：无法获取父目录")))?;
    let canonical_base = existing_ancestor(parent)
        .and_then(|path| path.canonicalize().ok())
        .ok_or_else(|| Error::Tool(format!("{MSG_WRITE_OUTSIDE_SANDBOX}：无法解析路径")))?;

    ensure_inside_work_dir(&canonical_base, &sandbox, MSG_WRITE_OUTSIDE_SANDBOX)?;
    Ok(target)
}

/// 规范化工作目录路径。
pub fn canonicalize_work_dir(work_dir: &Path) -> Result<PathBuf, Error> {
    work_dir
        .canonicalize()
        .map_err(|e| Error::Tool(format!("{MSG_WORK_DIR_RESOLVE_FAIL}：{e}")))
}

/// 解析搜索目录参数：
/// - 传入 `raw` 时按沙箱规则解析；
/// - 未传入时返回规范化后的工作目录。
pub fn resolve_search_dir(work_dir: &Path, raw: Option<&str>) -> Result<PathBuf, Error> {
    raw.map_or_else(
        || canonicalize_work_dir(work_dir),
        |path| resolve_existing_path(work_dir, path),
    )
}

fn is_sensitive_component_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    SENSITIVE_COMPONENTS
        .iter()
        .any(|sensitive| lower == *sensitive)
        || lower.starts_with(".env.")
}

/// 校验文件大小是否不超过指定上限。
pub async fn check_file_size(path: &Path, max_bytes: u64) -> Result<(), String> {
    let metadata = async_fs::metadata(path)
        .await
        .map_err(|e| format!("无法读取文件元数据：{e}"))?;
    let len = metadata.len();
    if len > max_bytes {
        Err(format!("文件过大：{len} bytes，限制为 {max_bytes} bytes"))
    } else {
        Ok(())
    }
}

/// 校验待写入内容大小是否不超过指定上限。
pub fn check_content_size(content: &str, max_bytes: usize) -> Result<(), String> {
    let len = content.len();
    if len > max_bytes {
        Err(format!(
            "写入内容过大：{len} bytes，限制为 {max_bytes} bytes"
        ))
    } else {
        Ok(())
    }
}

fn path_from_user_input(work_dir: &Path, raw: &str) -> PathBuf {
    let path = Path::new(raw);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        work_dir.join(path)
    }
}

/// 仅按路径组件折叠 `.` 与 `..`，不访问文件系统。
fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(_) | Component::RootDir | Component::Prefix(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

fn ensure_inside_work_dir(path: &Path, work_dir: &Path, message: &str) -> Result<(), Error> {
    let sandbox = canonicalize_work_dir(work_dir)?;
    if path.starts_with(&sandbox) {
        Ok(())
    } else {
        Err(Error::Tool(format!("{message}：{}", path.display())))
    }
}

fn existing_ancestor(path: &Path) -> Option<PathBuf> {
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

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{resolve_existing_path, resolve_writable_path};

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new() -> Self {
            let suffix = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time should be after epoch")
                .as_nanos();
            let path =
                env::temp_dir().join(format!("secaudit-tools-sandbox-{}-{suffix}", process::id()));
            fs::create_dir_all(&path).expect("create temp test dir");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn resolves_existing_path_inside_work_dir() {
        let temp = TestDir::new();
        let work_dir = temp.path().join("project");
        fs::create_dir_all(&work_dir).expect("create work dir");
        let file = work_dir.join("src.txt");
        fs::write(&file, "hello").expect("write file");

        let resolved = resolve_existing_path(&work_dir, "src.txt").expect("resolve path");

        assert_eq!(resolved, file.canonicalize().expect("canonical file"));
    }

    #[test]
    fn rejects_existing_path_outside_work_dir() {
        let temp = TestDir::new();
        let work_dir = temp.path().join("project");
        let outside = temp.path().join("outside.txt");
        fs::create_dir_all(&work_dir).expect("create work dir");
        fs::write(&outside, "secret").expect("write outside");

        let result = resolve_existing_path(&work_dir, outside.to_string_lossy().as_ref());

        assert!(result.is_err(), "outside existing paths must be rejected");
    }

    #[test]
    fn writable_path_may_point_to_new_child_path() {
        let temp = TestDir::new();
        let work_dir = temp.path().join("project");
        fs::create_dir_all(&work_dir).expect("create work dir");

        let target =
            resolve_writable_path(&work_dir, "nested/new.txt").expect("resolve writable path");

        assert_eq!(
            target,
            work_dir.canonicalize().unwrap().join("nested/new.txt")
        );
    }

    #[test]
    fn writable_path_rejects_outside_parent() {
        let temp = TestDir::new();
        let work_dir = temp.path().join("project");
        let outside_dir = temp.path().join("outside");
        fs::create_dir_all(&work_dir).expect("create work dir");
        fs::create_dir_all(&outside_dir).expect("create outside dir");

        let result = resolve_writable_path(&work_dir, "../outside/new.txt");

        assert!(result.is_err(), "outside writable paths must be rejected");
    }
}
