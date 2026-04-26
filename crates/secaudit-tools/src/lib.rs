//! `secaudit-tools` —— 安全审计工具系统（工具 trait + 内置工具实现）。

pub mod error {
    pub use secaudit_core::Error;
}

pub mod tools;

pub use tools::*;
