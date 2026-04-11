# securagent — 安全代码审计 LLM Agent

基于 ReAct/Reflexion 推理框架的安全代码审计 Agent，支持 CLI 和 Web 两种运行模式。

## 架构概览

```
securagent/
├── secaudit/          # Agent 主程序（CLI + Web SSE 服务器）
│   ├── src/agent/     # Agent 核心：状态机、执行器、推理策略
│   ├── src/tools/     # 工具集：代码解析、模式扫描、Semgrep、NVD 查询等
│   ├── src/llm.rs     # LLM 客户端（基于 async-openai）
│   ├── src/server.rs  # Web SSE 服务器（Axum）
│   └── src/prompt.rs  # Prompt 模板
├── ragrs/             # Rust 版 Ragas 评估框架
├── ragrs-bridge/      # CLI 桥接器（stdin JSON → ragrs 评分）
└── crates/llm-common/ # 通用 LLM 客户端（供 ragrs-bridge 使用）
```

## 快速开始

### 环境变量

```bash
export SECAUDIT_API_KEY="your-api-key"
export SECAUDIT_API_BASE_URL="https://api.openai.com/v1"  # 可选，默认 OpenAI
export SECAUDIT_MODEL="gpt-4o"                             # 可选
export SECAUDIT_STRATEGY="react"                            # react / reflexion
export SECAUDIT_MAX_ITERATIONS="10"                         # 可选
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

### Web 模式

```bash
# 启动 SSE 服务器（默认端口 8080）
just run-web

# 指定端口
just run-web 3001
```

Web 模式提供以下 API：
- `POST /api/audit` — 提交审计任务
- `GET /api/events` — SSE 事件流（状态/思考/工具调用/报告）
- `GET /api/health` — 健康检查

## 工具集

| 工具 | 描述 | 外部交互 |
|------|------|---------|
| `code_parser` | 解析代码结构（函数/输入点/敏感调用） | 否 |
| `pattern_scanner` | 可配置规则的漏洞模式匹配 | 否 |
| `cwe_knowledge_base` | CWE 漏洞知识库查询 | 否 |
| `dependency_checker` | 依赖漏洞审计（cargo/npm/pip） | 是（CLI） |
| `semgrep_scanner` | Semgrep 静态分析扫描 | 是（CLI） |
| `nvd_lookup` | NVD 漏洞数据库 CVE 查询 | 是（API） |

## 推理策略

- **ReAct**：标准 Observe-Reason-Act 循环，适合快速审计
- **Reflexion**：在 ReAct 基础上累积反思记忆，多轮深入审计

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

`coverage_report.json` 用于记录映射覆盖率、漏洞分布与未覆盖标签，便于迭代三的“评估-优化-再评估”闭环追踪。

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
