//! 工具文件探测辅助函数。

use std::path::Path;

use tokio::fs;

/// 二进制检测缓冲区大小（字节）
const BINARY_CHECK_SIZE: usize = 8192;

/// 异步判断文件是否为二进制文件（前 `BINARY_CHECK_SIZE` 字节中是否含空字节）。
pub async fn is_binary(path: &Path) -> bool {
    fs::read(path).await.map_or(true, |bytes| {
        let check_len = bytes.len().min(BINARY_CHECK_SIZE);
        bytes
            .get(..check_len)
            .is_some_and(|slice| slice.contains(&0))
    })
}
