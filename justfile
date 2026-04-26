# securagent Justfile — Rust workspace 构建命令
set dotenv-required
set dotenv-override

default:
    @just --list

# 构建（release 模式）
build:
    cargo build --release

# 调试构建
build-debug:
    cargo build

# 运行 clippy 检查
clippy:
    cargo clippy --workspace --all-targets -- -D warnings

# 运行测试
test:
    cargo test

# 格式化代码
fmt:
    cargo fmt --all

# 格式化检查（不修改）
fmt-check:
    cargo fmt --all --check

# 质量检查（格式 + Clippy）
check: fmt-check clippy

# 运行 secaudit CLI
run *ARGS:
    cargo run -p secaudit -- {{ARGS}}

# 运行 secaudit Web 模式
run-web PORT="8080":
    cargo run -p secaudit -- -m web -p {{PORT}}

# 清理构建产物
clean:
    cargo clean
