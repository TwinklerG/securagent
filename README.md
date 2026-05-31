# securagent — 安全代码审计 LLM Agent

基于 ReAct/Reflexion 推理框架的安全代码审计 Agent，支持 CLI 交互 / 单文件审计 / 非交互 chat 调试三种模式。

## 架构概览

```
securagent/
├── secaudit/                  # 应用入口（CLI / 非交互调试 / 输出渲染）
│   └── src/
│       ├── main.rs            # 命令行入口与运行模式分发
│       ├── interactive.rs     # 交互式 REPL
│       ├── headless.rs        # 非交互 chat 结果聚合与结构化输出
│       └── output/            # CLI/JSON/Markdown 输出
└── crates/
    ├── secaudit-core/         # 共享核心类型（Config / Error）
    ├── secaudit-llm/          # 通用 LLM 客户端与对话类型
    ├── secaudit-tools/        # 工具系统（Tool trait + 内置工具）
    │   └── src/tools/         # read/list/search/find/write/exec/semgrep/deps/nvd
    ├── secaudit-agent/        # 推理引擎（ReAct/Reflexion + Session + Prompt + Trajectory）
    └── secaudit-conversation/ # 会话服务、历史持久化与滑动窗口
```

## 快速开始

### 环境变量

```bash
# 方式一：从 .env.example 复制并填写
cp .env.example .env
# 编辑 .env 文件填入你的 API Key

# 方式二：直接导出环境变量
export SECAUDIT_API_KEY="your-api-key"
export SECAUDIT_API_BASE_URL="https://api.openai.com/v1"  # 可选，默认 OpenAI
export SECAUDIT_MODEL="gpt-4o"                             # 可选
export SECAUDIT_STRATEGY="react"                            # react / reflexion
export SECAUDIT_MAX_ITERATIONS="40"                         # 可选，默认 40
```

### CLI 模式

```bash
# 审计单个文件
just run test.py

# 指定语言和输出格式
just run test.py -l python -f markdown

# 导出 trajectory
just run test.py -o trajectory.json

# 使用 Reflexion 策略
just run test.py -s reflexion
```

### 交互模式

```bash
# 省略文件参数进入 REPL
just run
```

交互模式下 Agent 拥有完整工具集（文件读写、命令执行、搜索等），可对整个项目进行多轮审计。

### 非交互 chat 调试模式

```bash
# 直接传入单条消息（JSON 输出）
just run-chat --message "审计当前项目的高风险问题"

# 从 stdin 读取单条消息（便于脚本/外部 agent 对接）
echo "检查 src/main.rs 的命令注入风险" | just run-chat

# 传入多轮消息（JSON 数组）
just run-chat --messages-json '["先审计目录结构","继续检查命令执行风险","最后给出总结"]'

# 确认策略（默认 deny）：deny / allow / ask
just run-chat --message "运行 cargo clippy 并总结结果" --confirm-mode ask

# 文本输出（便于人读）
just run-chat --message "审计当前目录" --output-format text

# 会话管理：列出、恢复、归档
just run-chat --list-sessions
just run-chat --session <session-id> --message "继续上一轮审计"
just run-chat --archive-session <session-id>
```

非交互 chat 适用于外部 agent 或脚本调用，执行一次完整对话后输出结构化结果：

输出字段包括：
- `status`：`success`/`error`
- `final_message` 或 `error`
- `turns`：每轮用户输入、助手输出、错误与耗时
- `trace`：`state_history`、`think_events`、`tool_calls`、`confirm_events`
- `session`：`id`、`created_at`、`messages`（可直接用于评估）
- `metrics`：`token_usage`（`prompt_tokens`、`completion_tokens`、`total_tokens`）
- `duration_ms`、`work_dir`、`confirm_mode`
- `session_management`：可选会话管理信息（项目键、会话文件、状态、存储根目录）

评估输入契约文档见：`docs/contracts/agent-evaluation-contract.md`
持久化与历史对话规范见：`docs/conversation-persistence.md`

### 批量评估（演示样本）

仓库内置演示样本清单：`demo-projects/eval_manifest.json`

```bash
# 查看执行计划（不实际运行）
just eval-batch --dry-run

# 小规模实跑示例（仅 Python 样本）
just eval-batch --strategies react --runs 1 --case-filter python --continue-on-error

# 项目级评估示例（chat 模式，审计整个仓库）
just eval-batch --strategies react --runs 1 --case-filter project-securagent-root-chat --continue-on-error
```

输出目录：`outputs/eval-batch/`

- `summary.json`：策略级汇总指标
- `case-metrics.jsonl`：逐样本指标明细

## 工具集

Agent 根据运行模式加载不同工具集：

**交互 / 非交互 chat 模式**（9 个工具）：

| 工具 | 描述 | 外部交互 | 需确认 |
|------|------|---------|--------|
| `read_file` | 读取文件内容（带行号、支持行范围） | 否 | 否 |
| `list_directory` | 列出目录内容（支持递归） | 否 | 否 |
| `search_content` | 正则搜索文件内容（异步递归） | 否 | 否 |
| `find_files` | glob 模式查找文件 | 否 | 否 |
| `write_file` | 写入/创建文件 | 否 | 是 |
| `execute_command` | 执行 shell 命令（有黑名单） | 是 | 是 |
| `semgrep_scanner` | Semgrep 静态分析扫描 | 是（CLI） | 否 |
| `dependency_checker` | 依赖漏洞审计（cargo/npm/pip） | 是（CLI） | 否 |
| `nvd_lookup` | NVD 漏洞数据库 CVE 查询 | 是（API） | 否 |

**单文件审计模式**（3 个只读工具）：`semgrep_scanner`、`dependency_checker`、`nvd_lookup`

所有文件操作工具共享沙箱路径校验逻辑（`crates/secaudit-tools/src/tools/shared.rs`），确保路径不逃逸出工作目录。
所有运行模式统一使用**进程启动目录（cwd）**作为工作目录。

## 推理策略

- **ReAct**：标准 Observe-Reason-Act 循环，适合快速审计
- **Reflexion**：在 ReAct 基础上累积反思记忆，多轮深入审计

## LLM 客户端架构

统一的 LLM 类型和客户端定义在 `crates/secaudit-llm` 中：

- **`HttpLlmClient`**：基于 `async-openai`，支持两种调用模式
  - `generate(prompt)` — 单轮文本生成
  - `chat(messages, tools)` — 多轮对话 + 工具调用（Agent 交互）
- **核心类型**：`ChatMessage`、`Role`、`ToolCallResponse`、`FunctionCall`、`ToolDefinition`

`crates/secaudit-agent/src/llm.rs` 为薄重导出层，提供 `create_client(config)` 工厂函数。

## 构建与测试

```bash
just build       # Release 构建
just check       # Clippy + 格式检查
just test        # 运行测试
```

## CI/CD（GitLab）

项目已提供 `.gitlab-ci.yml`，默认在 **Push / Merge Request / Tag** 触发流水线：

- `rust-check`：Rust 工作区格式、Clippy 与测试
- `gui-frontend-check`：GUI 前端依赖安装、Biome 与类型检查
- `gui-tauri-check`：GUI Tauri Rust 格式、Clippy、测试与绑定文件检查
- `build`：Release 构建并产出二进制 artifacts（`secaudit`）
- `release`：仅在 Tag 触发，打包 `securagent-<tag>.tar.gz` 作为发布产物

GUI 前端检查使用 `GUI_BUN_IMAGE` 指定的预装 Bun 镜像，避免在流水线中通过外部安装脚本临时下载 Bun；如果 Runner 不能访问 Docker Hub，将该变量指向内网镜像即可。

## 数据集准备（scripts）

```bash
cd scripts
uv run prepare_dataset.py --output-dir ../datasets
```

生成文件：

- `datasets/code_vulnerability_labeled.json`
- `datasets/owasp_benchmark.json`
- `datasets/coverage_report.json`

其中扩展字段包括：

- `cwe_source`：CWE 来源（如 `raw_field` / `text_extract` / `mapped_label` / `owasp_csv`）
- `mapping_version`：HF 标签映射版本（仅 HF 记录）

`coverage_report.json` 用于记录映射覆盖率、漏洞分布与未覆盖标签，便于迭代三的"评估-优化-再评估"闭环追踪。
