import type {
  AgentEvent,
  AgentWorkbench,
  GuiMessage,
  ToolCapability,
  ToolParameter,
  TraceEvent,
} from "./types";
import { formatTimeLabel } from "./utils/time";

export const PREVIEW_AGENT_EVENT_DELAY_MS = 320;

const PREVIEW_WORK_DIR = "D:\\Project\\securagent";
const PREVIEW_SESSION_ID = "preview-session";
const PREVIEW_ARCHIVABLE_SESSION_ID = "preview-archivable-session";
const PREVIEW_ARCHIVED_SESSION_ID = "preview-archived-session";
const PREVIEW_STORAGE_ROOT = "~/.secaudit";
const PREVIEW_CONFIG_ERROR =
  "浏览器预览模式：真实 Agent 运行时读取 ~/.secaudit/config.json 或 SECAUDIT_API_KEY；可选配置为 SECAUDIT_API_BASE_URL、SECAUDIT_MODEL、SECAUDIT_MAX_ITERATIONS、SECAUDIT_STRATEGY。";
const PREVIEW_DEFAULT_REQUEST = "审计当前仓库中可能的高风险代码路径";
const PREVIEW_READ_FILE_ARGS = {
  path: "src/views/PromptApp.vue",
  offset: 1,
  limit: 120,
};
const PREVIEW_SEMGREP_ARGS = {
  project_path: PREVIEW_WORK_DIR,
  ruleset: "p/owasp-top-ten",
};
const PREVIEW_RESPONSE = [
  "## 预览审计计划",
  "",
  "我会先识别入口、权限边界和外部输入，再调用只读工具收集证据。",
  "",
  "- 检查命令执行路径",
  "- 汇总依赖与配置风险",
  "",
  "| 阶段 | 关注点 |",
  "| --- | --- |",
  "| 侦查 | 入口与配置 |",
  "| 验证 | 命令执行路径 |",
  "",
  "```rust",
  "fn audit_target(path: &str) -> bool {",
  "    !path.trim().is_empty()",
  "}",
  "```",
  "",
  "`browser preview` 不会访问真实 API Key。",
].join("\n");
let nextPreviewTraceId = 100;

const PREVIEW_TOOLS: Array<ToolCapability> = [
  {
    name: "read_file",
    category: "文件",
    risk: "只读",
    description: "读取工作区内文件内容，作为审计证据。",
    parameters: [
      createPreviewToolParameter(
        "path",
        "路径",
        "文件路径（相对于工作目录或绝对路径）",
        "string",
        true,
      ),
      createPreviewToolParameter(
        "offset",
        "起始行",
        "起始行号（从 1 开始，默认 1）",
        "integer",
        false,
      ),
      createPreviewToolParameter("limit", "行数", "读取行数（默认 2000）", "integer", false),
    ],
  },
  {
    name: "semgrep_scanner",
    category: "安全扫描",
    risk: "只读",
    description: "运行静态规则扫描，定位常见漏洞模式。",
    parameters: [
      createPreviewToolParameter(
        "project_path",
        "项目路径",
        "待扫描的项目路径或文件路径",
        "string",
        true,
      ),
      createPreviewToolParameter(
        "ruleset",
        "规则集",
        "Semgrep 规则集（如 p/owasp-top-ten、p/python、p/javascript）",
        "string",
        false,
      ),
    ],
  },
  {
    name: "dependency_checker",
    category: "依赖",
    risk: "只读",
    description: "识别依赖清单和潜在供应链风险。",
    parameters: [
      createPreviewToolParameter("project_path", "项目路径", "项目根目录路径", "string", true),
    ],
  },
  {
    name: "nvd_lookup",
    category: "情报",
    risk: "网络",
    description: "查询 CVE/NVD 信息，补充漏洞影响面和修复版本。",
    parameters: [
      createPreviewToolParameter(
        "query",
        "查询",
        "搜索关键词（如 SQL injection python）",
        "string",
        false,
      ),
      createPreviewToolParameter("cwe_id", "CWE", "CWE 编号（如 CWE-89）", "string", false),
    ],
  },
  {
    name: "execute_command",
    category: "命令",
    risk: "需确认",
    description: "执行受控命令收集证据，涉及副作用前需要用户确认。",
    parameters: [
      createPreviewToolParameter("command", "命令", "要执行的命令", "string", true),
      createPreviewToolParameter(
        "timeout_secs",
        "超时秒数",
        "超时秒数（默认 30）",
        "integer",
        false,
      ),
    ],
  },
];

export function createPreviewWorkbench(request?: string): AgentWorkbench {
  const traceNow = createTimeLabel();
  const sessionNow = new Date().toISOString();
  const userMessage = createUserMessage(request ?? PREVIEW_DEFAULT_REQUEST);
  const assistantMessage = createAssistantMessage();
  const messages = [userMessage, assistantMessage];

  return {
    project: {
      workDir: PREVIEW_WORK_DIR,
      storageRoot: PREVIEW_STORAGE_ROOT,
      configReady: false,
      configError: PREVIEW_CONFIG_ERROR,
    },
    conversation: {
      activeSessionId: PREVIEW_SESSION_ID,
      messageCount: messages.length,
      messages,
      sessions: createPreviewSessions(sessionNow, messages.length),
    },
    run: {
      phase: "ready",
      label: "预览",
      statusDetail: "浏览器预览使用模拟工作台；真实 Agent 状态由 Tauri 后端提供。",
      busy: false,
      canSend: true,
      canCancel: false,
      primaryActionLabel: "发送审计请求",
      pendingLabel: "预览运行中",
      pendingDetail: "浏览器预览正在生成模拟响应。",
    },
    tools: PREVIEW_TOOLS,
    trace: createPreviewTrace(traceNow),
    findings: createPreviewFindings(),
  };
}

export function createPreviewWorkbenchForWorkDir(workDir: string): AgentWorkbench {
  const snapshot = createPreviewWorkbench();
  return {
    ...snapshot,
    project: { ...snapshot.project, workDir },
  };
}

export function createPreviewSessionSwitch(sessionId: string): AgentWorkbench {
  return createPreviewWorkbench(`切换到会话 ${sessionId.slice(0, 8)}`);
}

export function createPreviewArchiveSession(sessionId: string): AgentWorkbench {
  const snapshot = createPreviewWorkbench();
  return {
    ...snapshot,
    conversation: {
      ...snapshot.conversation,
      sessions: snapshot.conversation.sessions.map((session) =>
        session.id === sessionId
          ? {
              ...session,
              status: "archived",
              canArchive: false,
              preview: "Agent: 已归档，保留历史审计上下文。",
            }
          : session,
      ),
    },
    trace: [
      createTraceEvent(90, "state", "归档会话", `已归档会话 ${sessionId.slice(0, 8)}。`),
      ...snapshot.trace,
    ],
  };
}

export function createPreviewAgentEvents(request: string): Array<AgentEvent> {
  return [
    createPreviewAgentEvent("state", "预览运行", `接收审计请求：${request}`),
    createPreviewAgentEvent("tool_call", "read_file", JSON.stringify(PREVIEW_READ_FILE_ARGS)),
    createPreviewAgentEvent("tool_call", "semgrep_scanner", JSON.stringify(PREVIEW_SEMGREP_ARGS)),
    createPreviewAgentEvent("token", "流式输出", "我会先识别入口、"),
    createPreviewAgentEvent("token", "流式输出", "权限边界和外部输入，"),
    createPreviewAgentEvent("tool_result", "read_file", "已返回预览模式下的模拟文件摘要。"),
    createPreviewAgentEvent(
      "tool_confirm",
      "工具确认请求",
      "即将执行未知命令：npm audit --json，是否允许？",
    ),
    createPreviewAgentEvent("token", "流式输出", "再调用只读工具收集证据。"),
  ];
}

function createPreviewToolParameter(
  key: ToolParameter["key"],
  label: string,
  description: string,
  typeName: string,
  required: boolean,
): ToolParameter {
  return {
    key,
    name: key,
    label,
    description,
    typeName,
    required,
  };
}

function createUserMessage(content: string): GuiMessage {
  return { role: "user", content };
}

function createAssistantMessage(): GuiMessage {
  return { role: "assistant", content: PREVIEW_RESPONSE };
}

function createPreviewSessions(
  now: string,
  activeMessageCount: number,
): AgentWorkbench["conversation"]["sessions"] {
  return [
    {
      id: PREVIEW_SESSION_ID,
      title: "预览审计会话",
      status: "active",
      updatedAt: now,
      messageCount: activeMessageCount,
      preview: "助手: 我会先识别入口、权限边界和外部输入。",
      canArchive: false,
    },
    {
      id: PREVIEW_ARCHIVABLE_SESSION_ID,
      title: "待归档命令审计",
      status: "active",
      updatedAt: now,
      messageCount: 4,
      preview: "用户: 审计命令执行和路径处理风险。",
      canArchive: true,
    },
    {
      id: PREVIEW_ARCHIVED_SESSION_ID,
      title: "历史依赖审计",
      status: "archived",
      updatedAt: now,
      messageCount: 6,
      preview: "Agent: 已归档的供应链风险分析记录。",
      canArchive: false,
    },
  ];
}

function createPreviewTrace(now: string): Array<TraceEvent> {
  return [
    {
      id: 1,
      kind: "state",
      title: "工作台已加载",
      detail: "等待用户输入审计目标。",
      occurredAt: now,
    },
    {
      id: 2,
      kind: "tool_call",
      title: "工具能力",
      detail: "read_file、search_content、semgrep_scanner 已准备。",
      occurredAt: now,
    },
  ];
}

function createPreviewFindings(): AgentWorkbench["findings"] {
  return [
    {
      id: "preview-finding",
      status: "candidate",
      statusLabel: "候选",
      severity: "pending",
      severityLabel: "待确认",
      confidenceLabel: "等待证据",
      title: "候选发现会在 Agent 收集证据后出现",
      location: "当前无真实扫描结果",
      taxonomy: null,
      summary: "浏览器预览不会执行真实扫描，只展示后端发现契约的结构化占位。",
      evidence: [
        {
          label: "证据来源",
          source: "运行轨迹",
          detail: "等待工具调用、扫描输出或文件片段进入轨迹。",
        },
        {
          label: "归因信息",
          source: "Agent 输出",
          detail: "等待模型给出 CWE、风险原因和影响范围。",
        },
      ],
      remediation: "确认证据后再生成具体修复建议。",
      nextAction: "发送审计请求，让 Agent 收集证据并更新发现详情。",
    },
  ];
}

function createPreviewAgentEvent(
  kind: TraceEvent["kind"],
  title: string,
  detail: string,
): AgentEvent {
  const trace = createTraceEvent(nextPreviewTraceId, kind, title, detail);
  nextPreviewTraceId += 1;
  return { trace };
}

function createTraceEvent(
  id: number,
  kind: TraceEvent["kind"],
  title: string,
  detail: string,
): TraceEvent {
  return {
    id,
    kind,
    title,
    detail,
    occurredAt: createTimeLabel(),
  };
}

function createTimeLabel(): string {
  return formatTimeLabel();
}
