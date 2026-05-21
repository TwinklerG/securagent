# 会话持久化与历史对话规范

本文档定义 securagent 迭代三的本地持久化目录、会话服务边界和历史对话裁剪策略。

## 目标

- 用户可以保存、列出、恢复、归档 chat/headless 会话。
- Memory、Tool 动态配置、Skill 动态配置复用同一项目键和目录布局。
- TUI、headless CLI、未来 Web GUI 只做前端适配，不能各自维护一套历史读写逻辑。
- headless JSON 既有字段保持兼容，新增字段只能作为可忽略扩展。

## 架构边界

共享会话能力位于 `crates/secaudit-conversation`：

```text
TUI / headless CLI / future Web GUI
        |
        v
secaudit-conversation
        |
        +-- ConversationService
        +-- StorageLayout
        +-- SlidingWindowPolicy
        |
        v
secaudit-agent
```

职责划分：

- `secaudit-conversation`：会话创建、恢复、保存、归档、列表投影、项目键、滑动窗口。
- `secaudit-agent`：推理循环、工具调用、内存中的 `Session` 和 `ChatMessage`。
- `secaudit`：CLI/TUI 参数解析、输出渲染、调用 `ConversationService`。
- 未来 Web GUI：调用同一个 service 或薄 HTTP adapter，不重新解析会话文件。

## 运行时目录

默认用户级运行时目录：

```text
~/.secaudit
└── projects
    └── <path-readable-project-key>
        ├── memory
        ├── sessions
        │   ├── active
        │   │   └── <session-id>.json
        │   ├── archived
        │   │   └── <session-id>.json
        │   └── index.jsonl
        ├── skills
        ├── tool-config
        └── project.json
```

项目内 `.secaudit/` 仅用于可提交模板、说明或示例配置，默认不写入用户私有会话、记忆或动态配置。

项目键来自规范化工作目录的可读编码，不使用不可复原 hash 作为主标识。例如：

```text
/workspace/team/Sample Project
=> -workspace-team-Sample-Project
```

如果两个不同路径编码后发生冲突，会在可读项目键后追加短后缀，例如 `-workspace-a-b--12ab34cd`。`project.json` 保存 canonical path，因此仍能人工复原项目来源。

## 文件格式

### `project.json`

```json
{
  "schema_version": 1,
  "project_key": "-workspace-team-Sample-Project",
  "canonical_path": "/workspace/team/Sample Project",
  "created_at": "2026-05-16T00:00:00Z",
  "updated_at": "2026-05-16T00:00:00Z"
}
```

### 会话文件

```json
{
  "schema_version": 1,
  "id": "uuid",
  "project_key": "-workspace-team-Sample-Project",
  "created_at": "2026-05-16T00:00:00Z",
  "updated_at": "2026-05-16T00:05:00Z",
  "status": "active",
  "title": "未命名会话",
  "work_dir": "/workspace/team/Sample Project",
  "messages": [],
  "summary": null
}
```

`messages` 使用 `secaudit-llm::ChatMessage` 的现有序列化格式。`summary` 为摘要压缩预留，MVP 不填充。

### `sessions/index.jsonl`

每行是一个 `SessionMetadata` 投影：

```json
{
  "schema_version": 1,
  "session_id": "uuid",
  "status": "active",
  "title": "未命名会话",
  "created_at": "2026-05-16T00:00:00Z",
  "updated_at": "2026-05-16T00:05:00Z",
  "message_count": 12,
  "file": "active/uuid.json"
}
```

索引是 append-friendly JSONL。读取时按 `session_id` 归并，以最后一条为准。

写入侧会做机会性 compaction：普通小索引只追加；索引文件超过阈值且重复状态记录明显多于当前会话数时，`secaudit-conversation` 会在索引锁保护下重放 JSONL，并原子改写为每个 session 最新一条 `SessionMetadata`。这保留了 append-friendly 的写入路径，同时避免长期使用后 `list_sessions` 每次都扫描大量过期投影。

## 服务 API

入口层应通过 `ConversationService` 访问历史：

- `start_session(work_dir)`：创建新的托管会话；空会话不落盘。
- `load_session(work_dir, session_id)`：加载 active 或 archived 会话。
- `list_sessions(work_dir)`：返回 `SessionMetadata` 列表。
- `list_sessions_with_preview(work_dir)`：返回带最近用户/助手消息预览的会话列表投影。
- `archive_session(work_dir, session_id)`：把 active 会话移动到 archived。
- `chat(agent, managed_session, user_message)`：构造裁剪上下文、调用 Agent、保存完整历史。

CLI/TUI/Web GUI 不应直接读写 `sessions/*.json` 或 `index.jsonl`。

空会话不保留：`start_session` 只分配会话 ID 与项目上下文；只有出现非空用户/助手消息后，`chat` 或显式 `save_session` 才会写入会话文件与索引。`list_sessions` 也会过滤历史遗留的 0 消息索引项，避免 UI 展示不可继续的空会话。

## 滑动窗口

当前使用 `SlidingWindowPolicy` 的消息条数裁剪，不是 token 预算裁剪，也不会像 Claude Code / Codex 接近上下文上限时自动生成摘要压缩：

- system prompt 始终保留。
- 默认保留最新 24 条非 system 消息。
- 裁剪只作用于送给 LLM 的上下文视图。
- 磁盘上的完整历史不被裁剪。
- 裁剪后若出现孤立的 leading tool result，会丢弃该 tool result，避免向 OpenAI-compatible 接口发送无对应 tool call 的非法消息序列。

因此当前策略能限制发送给模型的消息数量，但被裁掉的旧内容不会以摘要形式回流到上下文里；长会话里早期事实可能被遗忘。摘要压缩后续应复用同一边界：先从 `summary` 注入压缩历史，再拼接滑动窗口保留的最新消息。

## Headless CLI

当前 chat/headless 支持：

```bash
just run-chat --message "审计当前项目"
just run-chat --session <id> --message "继续上一轮"
just run-chat --list-sessions
just run-chat --archive-session <id>
```

普通 chat 默认创建并保存新会话。`--session <id>` 恢复已有会话继续；如果恢复的是 archived 会话，继续聊天成功后会重新保存为 active。

`--list-sessions` 和 `--archive-session` 不调用 LLM，也不需要 `SECAUDIT_API_KEY`。

headless JSON 在原有字段外追加可选字段：

```json
{
  "session_management": {
    "project_key": "...",
    "status": "active",
    "session_path": "...",
    "storage_root": "..."
  }
}
```

消费者必须可以忽略该字段。

## TUI 会话管理

TUI 同样只通过 `ConversationService` 访问历史，不直接读取 `sessions/*.json` 或 `index.jsonl`。

当前支持：

```text
/sessions          列出当前项目会话，包含序号、短 ID、消息数、更新时间和最近消息预览
/session <id>      切换到指定会话
/session <序号>    按 /sessions 显示的序号切换会话
/new           新建会话并清空当前视图
/clear         /new 的兼容别名
```

切换会话时，TUI 从 `ManagedSession.session().messages()` 重建主对话区，只恢复用户与助手消息。system prompt 和 tool result 不直接渲染到主对话区，避免历史恢复后破坏 conversation-first 展示；当前运行期间的工具调用仍通过事件面板展示。

会话列表预览由 `ConversationService::list_sessions_with_preview` 生成，TUI 只消费该投影，不直接读取会话 JSON 文件。

## 后续接入

- Web GUI：复用同一个 service，或提供薄 HTTP/WebSocket adapter 包装 service。
- Memory：通过 `StorageLayout::memory_dir(project_key)` 访问 `~/.secaudit/projects/<project-key>/memory/`。
- Tool/Skill 动态配置：通过 `StorageLayout::tool_config_dir(project_key)` 与 `StorageLayout::skills_dir(project_key)` 访问用户私有配置目录；项目内 `.secaudit/` 只放模板和示例。
