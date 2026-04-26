# Agent 评估输入契约（securagent）

本文档定义 `securagent` 与评估平台（如 `ragaslens`）之间的数据接口。

## 1. 目标

- 统一 Agent 输出格式，避免平台联调反复改字段
- 支持迭代二要求的结果导向 + 过程导向评估
- 支持显式指标（含 token、时延、工具调用）与后续模糊指标扩展

## 2. 产出入口

### 2.1 Chat 模式（推荐）

命令：

```bash
just run-chat --message "审计当前项目的高风险漏洞" --output-format json
```

输出为 `HeadlessResponse` JSON（`status=success|error`）。

### 2.2 单文件模式 trajectory

命令：

```bash
just run path/to/file.java -f json -o trajectory.json
```

输出为 `MultiTurnSample` JSON，可直接作为样本轨迹。

## 3. 数据结构（Chat 输出）

### 3.1 顶层结构

```json
{
  "status": "success",
  "final_message": "...",
  "turns": [],
  "trace": {},
  "session": {},
  "metrics": {},
  "duration_ms": 0,
  "work_dir": "...",
  "confirm_mode": "deny"
}
```

失败场景：`status=error`，并包含 `error` 字段。

### 3.2 字段说明

- `status`: 执行状态，`success` 或 `error`
- `final_message`: 最终回复（仅 success）
- `error`: 失败原因（仅 error）
- `turns`: 每轮记录
  - `turn_index`
  - `user_message`
  - `assistant_message`
  - `error`
  - `duration_ms`
- `trace`: 过程轨迹
  - `state_history`: 状态流转序列
  - `think_events`: 思考文本序列
  - `tool_calls`: 工具调用序列
    - `name`
    - `args`
    - `result`
  - `confirm_events`: 高风险操作确认记录
- `session`: 会话快照
  - `id`
  - `created_at`
  - `messages`（完整对话消息，含 tool role）
- `metrics`: 统计信息
  - `token_usage`
    - `prompt_tokens`
    - `completion_tokens`
    - `total_tokens`
- `duration_ms`: 本次请求总耗时
- `work_dir`: 工作目录
- `confirm_mode`: `deny|allow|ask`

## 4. 数据结构（trajectory 输出）

`trajectory.json` 使用 `MultiTurnSample`：

```json
{
  "user_input": [],
  "reference": "...",
  "reference_tool_calls": null,
  "metadata": {
    "token_usage": {
      "prompt_tokens": 0,
      "completion_tokens": 0,
      "total_tokens": 0
    }
  }
}
```

说明：
- `metadata.token_usage` 为可选；有可统计 usage 时输出
- `reference` 当前默认由 findings 序列化生成

## 5. 评估平台最低消费要求

平台至少应读取以下字段：

- 结果导向：`final_message`、`duration_ms`、`metrics.token_usage`
- 过程导向：`trace.state_history`、`trace.tool_calls`、`session.messages`

## 6. 兼容性约定

- 新增字段：允许（向后兼容）
- 删除字段：禁止（需版本升级）
- 字段重命名：禁止（需版本升级）

版本建议：
- 当前契约版本：`v1`
- 若结构不兼容变化，升为 `v2`
