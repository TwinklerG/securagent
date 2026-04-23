# securagent — 安全代码审计 LLM Agent

基于 ReAct/Reflexion 推理框架的安全代码审计 Agent，支持 CLI 交互 / 单文件审计 / Web 会话三种模式。

## 架构概览

```
securagent/
├── secaudit/              # Agent 主程序
│   ├── src/agent/         # Agent 核心：状态机、执行器、推理策略
│   │   ├── state.rs       #   状态枚举与流转
│   │   ├── executor.rs    #   ReAct 单步执行器
│   │   └── strategy/      #   推理策略（react / reflexion）
│   ├── src/tools/         # 工具集
│   │   ├── shared.rs      #   共享模块（沙箱路径校验、二进制检测）
│   │   ├── read_file.rs   #   文件读取（带行号）
│   │   ├── list_directory.rs   # 目录列表
│   │   ├── search_content.rs   # 正则内容搜索（异步 IO）
│   │   ├── find_files.rs       # glob 文件查找
│   │   ├── write_file.rs       # 文件写入（需确认）
│   │   ├── execute_command.rs  # 命令执行（需确认 + 黑名单）
│   │   ├── semgrep_scanner.rs  # Semgrep 静态分析
│   │   ├── dependency_checker.rs  # 依赖漏洞审计
│   │   └── nvd_lookup.rs      # NVD CVE 查询
│   ├── src/llm.rs         # LLM 模块（重导出 llm-common 统一类型）
│   ├── src/server.rs      # Web 会话 API 服务器（Axum + SSE）
│   ├── src/session.rs     # 会话管理（UUID + 序列化）
│   ├── src/config.rs      # 应用配置（环境变量 + Default）
│   ├── src/prompt.rs      # Prompt 模板
│   └── src/trajectory.rs  # 对话 → MultiTurnSample 转换
├── ragrs/                 # Rust 版 Ragas 评估框架
├── ragrs-bridge/          # CLI 桥接器（stdin JSON → ragrs 评分）
└── crates/llm-common/     # 通用 LLM 客户端与对话类型
                           #   供 secaudit 和 ragrs-bridge 共用
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

# 导出 trajectory（供评估平台使用）
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

### Web 模式

```bash
# 启动会话式 API 服务器（默认端口 8080）
just run-web

# 指定端口
just run-web 3001
```

Web 模式提供会话式 API：

| 端点 | 方法 | 说明 |
|------|------|------|
| `/api/sessions` | POST | 创建会话，指定 `work_dir` |
| `/api/sessions/:id/messages` | POST | 异步发送消息（结果通过 SSE 推送） |
| `/api/sessions/:id/chat` | POST | **同步聊天**（等待完成后一次性返回） |
| `/api/sessions/:id/events` | GET | SSE 事件流（状态/思考/工具调用/结果） |
| `/api/sessions/:id/history` | GET | 获取会话完整历史 |
| `/api/sessions/:id/confirm` | POST | 回复确认请求 |
| `/api/health` | GET | 健康检查 |

`/chat` 端点适合 AI Agent 或脚本调用，无需解析 SSE 流，返回完整结构化响应：

```json
{
  "message": "Agent 最终回复",
  "tool_calls": [{ "name": "...", "args": "...", "result": "..." }],
  "state_history": ["执行中", "分析中", "完成"],
  "duration_ms": 12345
}
```

## 工具集

Agent 根据运行模式加载不同工具集：

**交互 / Web 模式**（9 个工具）：

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

所有文件操作工具共享沙箱路径校验逻辑（`tools/shared.rs`），确保路径不逃逸出工作目录。

## 推理策略

- **ReAct**：标准 Observe-Reason-Act 循环，适合快速审计
- **Reflexion**：在 ReAct 基础上累积反思记忆，多轮深入审计

## LLM 客户端架构

统一的 LLM 类型和客户端定义在 `crates/llm-common` 中：

- **`HttpLlmClient`**：基于 `async-openai`，支持两种调用模式
  - `generate(prompt)` — 单轮文本生成（ragrs LLM-as-Judge 评估）
  - `chat(messages, tools)` — 多轮对话 + 工具调用（Agent 交互）
- **核心类型**：`ChatMessage`、`Role`、`ToolCallResponse`、`FunctionCall`、`ToolDefinition`

`secaudit/src/llm.rs` 为薄重导出层，提供 `create_client(config)` 工厂函数。

## 构建与测试

```bash
just build       # Release 构建
just check       # Clippy + 格式检查
just test        # 运行测试
```

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

### 一键导入到评估平台

```bash
cd scripts

# 导入到任务样本（/api/samples）
uv run import_prepared_dataset.py \
	--input-file ../datasets/code_vulnerability_labeled.json \
	--input-file ../datasets/owasp_benchmark.json \
	--task-id 1

# 导入到指定数据集（/api/datasets/:id/samples）
uv run import_prepared_dataset.py \
	--input-file ../datasets/owasp_benchmark.json \
	--task-id 1 --dataset-id 2
```

支持 `--dry-run` 先做结构检查，不发送请求。
