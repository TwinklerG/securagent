use std::fs::{self, OpenOptions};
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime};

use crate::error::Result;

const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(10);
const LOCK_MAX_POLLS: usize = 1_000;
const LOCK_STALE_AFTER: Duration = Duration::from_mins(5);

/// 基于文件的排他锁。
///
/// 在目标文件旁创建一个 `<filename>.lock` 文件来协调并发写入。
/// 持有锁期间，写入者可以安全操作目标文件。
pub struct FileLock {
    path: PathBuf,
}

impl FileLock {
    /// 为目标文件获取排他锁。
    ///
    /// 会轮询等待直到锁可用，或超过 `LOCK_MAX_POLLS` 次尝试后超时。
    ///
    /// # Errors
    ///
    /// 锁文件创建失败或等待超时时返回错误。
    pub fn acquire(target_file: &Path) -> Result<Self> {
        let lock_path = target_file.with_extension("lock");

        for _ in 0..LOCK_MAX_POLLS {
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock_path)
            {
                Ok(_) => return Ok(Self { path: lock_path }),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                    remove_stale_lock(&lock_path)?;
                    thread::sleep(LOCK_POLL_INTERVAL);
                }
                Err(error) => return Err(error.into()),
            }
        }

        Err(io::Error::new(
            ErrorKind::TimedOut,
            format!("等待文件锁超时：{}", lock_path.display()),
        )
        .into())
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn remove_stale_lock(lock_path: &Path) -> Result<()> {
    let Ok(metadata) = fs::metadata(lock_path) else {
        return Ok(());
    };
    let Ok(modified) = metadata.modified() else {
        return Ok(());
    };
    let Ok(age) = SystemTime::now().duration_since(modified) else {
        return Ok(());
    };
    if age <= LOCK_STALE_AFTER {
        return Ok(());
    }

    match fs::remove_file(lock_path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::TempDir;

    use super::FileLock;

    const DATA_FILE: &str = "data.json";

    #[test]
    fn acquire_and_release_lock() {
        let temp = TempDir::new().expect("create tempdir");
        let target = temp.path().join(DATA_FILE);
        fs::write(&target, "{}").expect("write target");

        let lock_path = target.with_extension("lock");

        {
            let _lock = FileLock::acquire(&target).expect("acquire lock");
            assert!(lock_path.exists(), "lock file should exist while held");
        }

        assert!(!lock_path.exists(), "lock file should be removed on drop");
    }

    #[test]
    fn acquire_without_lock_succeeds() {
        let temp = TempDir::new().expect("create tempdir");
        let target = temp.path().join(DATA_FILE);
        fs::write(&target, "{}").expect("write target");

        // No lock file present — acquire should succeed immediately
        let _lock = FileLock::acquire(&target).expect("acquire without existing lock");
    }

    #[test]
    fn lock_for_nonexistent_directory_errors() {
        let target = Path::new("/nonexistent/directory/file.json");
        let result = FileLock::acquire(target);
        assert!(result.is_err(), "should fail for nonexistent directory");
    }
}
