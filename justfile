# securagent Justfile — Rust workspace 构建命令
set dotenv-required
set dotenv-override

mod gui "apps/secaudit-gui"


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

# 运行 secaudit 非交互 chat 调试模式（JSON 输出）
run-chat *ARGS:
    cargo run -p secaudit -- --mode chat {{ARGS}}

# 运行批量评估（默认 demo manifest，react+reflexion）
eval-batch *ARGS:
    python3 scripts/run_eval_batch.py {{ARGS}}

# 清理构建产物
clean:
    cargo clean
