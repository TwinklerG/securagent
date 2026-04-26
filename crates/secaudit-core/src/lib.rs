//! `secaudit-core` —— 跨 crate 共享的核心类型（配置与错误定义）。

pub mod config;
pub mod error;

pub use config::Config;
pub use error::Error;
