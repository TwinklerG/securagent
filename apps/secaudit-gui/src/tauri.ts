import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import {
  createPreviewAgentEvents,
  createPreviewArchiveSession,
  createPreviewSessionSwitch,
  createPreviewWorkbench,
  createPreviewWorkbenchForWorkDir,
  PREVIEW_AGENT_EVENT_DELAY_MS,
  PREVIEW_APPROVAL_PROMPT,
} from "./preview";
import type { AgentEvent, AgentWorkbench } from "./types";

const previewAgentEventHandlers = new Set<(event: AgentEvent) => void>();
const isTauriRuntime = () => typeof window !== "undefined" && Boolean(window.__TAURI_INTERNALS__);

export async function initWorkbench(): Promise<AgentWorkbench> {
  if (!isTauriRuntime()) {
    return createPreviewWorkbench();
  }
  return invoke<AgentWorkbench>("init_workbench");
}

export async function sendAuditMessage(message: string): Promise<AgentWorkbench> {
  if (!isTauriRuntime()) {
    await emitPreviewAgentEvents(message);
    return createPreviewWorkbench(message);
  }
  return invoke<AgentWorkbench>("send_audit_message", { message });
}

export async function createSession(): Promise<AgentWorkbench> {
  if (!isTauriRuntime()) {
    return createPreviewWorkbench();
  }
  return invoke<AgentWorkbench>("new_session");
}

export async function switchSession(sessionId: string): Promise<AgentWorkbench> {
  if (!isTauriRuntime()) {
    return createPreviewSessionSwitch(sessionId);
  }
  return invoke<AgentWorkbench>("switch_session", { sessionId });
}

export async function archiveSession(sessionId: string): Promise<AgentWorkbench> {
  if (!isTauriRuntime()) {
    return createPreviewArchiveSession(sessionId);
  }
  return invoke<AgentWorkbench>("archive_session", { sessionId });
}

export async function setWorkDir(workDir: string): Promise<AgentWorkbench> {
  if (!isTauriRuntime()) {
    return createPreviewWorkbenchForWorkDir(workDir);
  }
  return invoke<AgentWorkbench>("set_work_dir", { workDir });
}

export async function resolveCommandApproval(id: number, approved: boolean): Promise<void> {
  if (!isTauriRuntime()) {
    emitPreviewApprovalResolution(id, approved);
    return;
  }
  await invoke("resolve_command_approval", { id, approved });
}

export async function selectWorkDir(currentWorkDir: string): Promise<string | null> {
  if (!isTauriRuntime()) {
    return null;
  }
  return open({
    title: "选择审计工作区",
    directory: true,
    multiple: false,
    defaultPath: currentWorkDir || undefined,
  });
}

export async function listenAgentEvents(handler: (event: AgentEvent) => void) {
  if (!isTauriRuntime()) {
    previewAgentEventHandlers.add(handler);
    return () => {
      previewAgentEventHandlers.delete(handler);
    };
  }
  return listen<AgentEvent>("agent-event", (event) => handler(event.payload));
}

function emitPreviewApprovalResolution(id: number, approved: boolean) {
  const statusLabel = approved ? "已允许" : "已拒绝";
  const event: AgentEvent = {
    trace: {
      id,
      kind: "tool_confirm",
      title: "工具确认结果",
      detail: `${PREVIEW_APPROVAL_PROMPT}\n\n预览确认结果：${statusLabel}。`,
      occurredAt: new Date().toLocaleTimeString("zh-CN", { hour12: false }),
    },
    approvalRequest: null,
    approvalResolution: {
      id,
      approved,
      statusLabel,
    },
    tokenUsage: null,
  };
  for (const handler of previewAgentEventHandlers) {
    handler(event);
  }
}

async function emitPreviewAgentEvents(message: string) {
  for (const event of createPreviewAgentEvents(message)) {
    await sleep(PREVIEW_AGENT_EVENT_DELAY_MS);
    for (const handler of previewAgentEventHandlers) {
      handler(event);
    }
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => {
    window.setTimeout(resolve, ms);
  });
}
