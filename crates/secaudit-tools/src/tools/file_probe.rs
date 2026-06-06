//! 工具文件探测辅助函数。

use std::path::Path;

use tokio::fs;
use tokio::io::AsyncReadExt;

/// 二进制检测缓冲区大小（字节）
const BINARY_CHECK_SIZE: usize = 8192;

/// 异步判断文件是否为二进制文件（前 `BINARY_CHECK_SIZE` 字节中是否含空字节）。
pub async fn is_binary(path: &Path) -> bool {
    let Ok(mut file) = fs::File::open(path).await else {
        return true;
    };

    let mut buffer = [0_u8; BINARY_CHECK_SIZE];
    let Ok(bytes_read) = file.read(&mut buffer).await else {
        return true;
    };

    buffer
        .get(..bytes_read)
        .is_some_and(|slice| slice.contains(&0))
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{BINARY_CHECK_SIZE, is_binary};

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
                env::temp_dir().join(format!("secaudit-tools-probe-{}-{suffix}", process::id()));
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

    #[tokio::test]
    async fn detects_null_byte_in_probe_window() {
        let temp = TestDir::new();
        let path = temp.path().join("binary.bin");
        fs::write(&path, b"abc\0def").expect("write binary file");

        assert!(is_binary(&path).await);
    }

    #[tokio::test]
    async fn ignores_null_byte_after_probe_window() {
        let temp = TestDir::new();
        let path = temp.path().join("mostly-text.txt");
        let mut content = vec![b'a'; BINARY_CHECK_SIZE + 1];
        if let Some(byte) = content.get_mut(BINARY_CHECK_SIZE) {
            *byte = 0;
        }
        fs::write(&path, content).expect("write text-like file");

        assert!(!is_binary(&path).await);
    }
}
