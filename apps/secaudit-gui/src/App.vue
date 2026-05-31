<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import CommandApprovalDialog from "./components/CommandApprovalDialog.vue";
import ConversationPane from "./components/ConversationPane.vue";
import OpsRail from "./components/OpsRail.vue";
import SessionRail from "./components/SessionRail.vue";
import {
  OPS_RAIL_DEFAULT_WIDTH,
  SESSION_ID_PREVIEW_LENGTH,
  SESSION_RAIL_WIDTH,
  TRACE_LIMIT,
} from "./constants";
import {
  archiveSession,
  createSession,
  initWorkbench,
  listenAgentEvents,
  resolveCommandApproval,
  selectWorkDir,
  sendAuditMessage,
  setWorkDir,
  switchSession,
} from "./tauri";
import type {
  AgentEvent,
  AgentWorkbench,
  CommandApprovalRequest,
  GuiMessage,
  TraceEvent,
} from "./types";

const LIVE_ERROR_FALLBACK = "本轮运行失败，详情见运行轨迹和错误提示。";
const LIVE_PENDING_FALLBACK = "Agent 正在分析当前审计请求。";
const LIVE_STATE_PREFIX = "Agent 状态";
const LIVE_TOOL_CALL_PREFIX = "正在调用工具";
const LIVE_TOOL_CONFIRM_PREFIX = "工具请求确认";
const LIVE_TOOL_RESULT_PREFIX = "已收到工具结果";

const workbench = ref<AgentWorkbench | null>(null);
const requestPending = ref(false);
const liveAssistantContent = ref("");
const liveAssistantSynthetic = ref(false);
const switchingWorkDir = ref(false);
const errorText = ref<string | null>(null);
const opsRailWidth = ref(OPS_RAIL_DEFAULT_WIDTH);
const pendingApproval = ref<CommandApprovalRequest | null>(null);
const approvalPending = ref(false);
const approvalErrorText = ref<string | null>(null);

const backendMessages = computed<Array<GuiMessage>>(
  () => workbench.value?.conversation.messages ?? [],
);
const messages = computed<Array<GuiMessage>>(() => {
  if (!liveAssistantContent.value || !requestPending.value) {
    return backendMessages.value;
  }
  return [
    ...backendMessages.value,
    {
      role: "assistant",
      content: liveAssistantContent.value,
    },
  ];
});
const trace = computed<Array<TraceEvent>>(() => workbench.value?.trace ?? []);
const sessions = computed(() => workbench.value?.conversation.sessions ?? []);
const tools = computed(() => workbench.value?.tools ?? []);
const findings = computed(() => workbench.value?.findings ?? []);
const project = computed(() => workbench.value?.project ?? null);
const run = computed(() => workbench.value?.run ?? null);
const runBusy = computed(() => requestPending.value || Boolean(run.value?.busy));
const activeSessionId = computed(() => workbench.value?.conversation.activeSessionId ?? null);

const activeSessionLabel = computed(() => {
  const id = activeSessionId.value ?? "";
  return id ? id.slice(0, SESSION_ID_PREVIEW_LENGTH) : "未创建";
});
const shellStyle = computed(() => ({
  gridTemplateColumns: `${SESSION_RAIL_WIDTH}px minmax(0, 1fr) ${opsRailWidth.value}px`,
}));

onMounted(async () => {
  await listenAgentEvents(handleAgentEvent);
  await refreshWorkbench();
});

async function refreshWorkbench() {
  try {
    workbench.value = await initWorkbench();
    errorText.value = null;
  } catch (error) {
    setError(error);
  }
}

async function handleSubmit(message: string) {
  if (!run.value?.canSend || runBusy.value) {
    return;
  }

  requestPending.value = true;
  errorText.value = null;
  resetLiveAssistant();
  showLiveStatus(run.value?.pendingDetail || LIVE_PENDING_FALLBACK);
  appendLocalMessage({ role: "user", content: message });

  try {
    const nextWorkbench = await sendAuditMessage(message);
    resetLiveAssistant();
    workbench.value = nextWorkbench;
  } catch (error) {
    resetLiveAssistant();
    setError(error);
  } finally {
    requestPending.value = false;
  }
}

async function handleNewSession() {
  if (runBusy.value) {
    return;
  }

  try {
    errorText.value = null;
    workbench.value = await createSession();
  } catch (error) {
    setError(error);
  }
}

async function handleSwitchSession(id: string) {
  if (runBusy.value || id === activeSessionId.value) {
    return;
  }

  try {
    errorText.value = null;
    workbench.value = await switchSession(id);
  } catch (error) {
    setError(error);
  }
}

async function handleArchiveSession(id: string) {
  if (runBusy.value || id === activeSessionId.value) {
    return;
  }

  try {
    errorText.value = null;
    workbench.value = await archiveSession(id);
  } catch (error) {
    setError(error);
  }
}

async function handleApplyWorkDir(nextDir: string) {
  if (runBusy.value || switchingWorkDir.value || nextDir === project.value?.workDir) {
    return;
  }

  switchingWorkDir.value = true;
  errorText.value = null;
  try {
    workbench.value = await setWorkDir(nextDir);
  } catch (error) {
    setError(error);
  } finally {
    switchingWorkDir.value = false;
  }
}

async function handleBrowseWorkDir(currentWorkDir: string) {
  if (runBusy.value) {
    return;
  }

  try {
    errorText.value = null;
    const selected = await selectWorkDir(currentWorkDir);
    if (selected && selected !== project.value?.workDir) {
      await handleApplyWorkDir(selected);
    }
  } catch (error) {
    setError(error);
  }
}

async function handleResolveApproval(approved: boolean) {
  const approval = pendingApproval.value;
  if (!approval || approvalPending.value) {
    return;
  }

  approvalPending.value = true;
  approvalErrorText.value = null;
  try {
    await resolveCommandApproval(approval.id, approved);
    pendingApproval.value = null;
  } catch (error) {
    approvalErrorText.value = error instanceof Error ? error.message : String(error);
  } finally {
    approvalPending.value = false;
  }
}

function handleAgentEvent(event: AgentEvent) {
  if (event.approvalRequest) {
    pendingApproval.value = event.approvalRequest;
    approvalErrorText.value = null;
  }
  if (
    event.approvalResolution &&
    pendingApproval.value?.id === event.approvalResolution.id
  ) {
    pendingApproval.value = null;
    approvalErrorText.value = null;
  }

  if (!workbench.value) {
    return;
  }

  if (isPersistentTraceEvent(event.trace)) {
    workbench.value = {
      ...workbench.value,
      trace: [event.trace, ...workbench.value.trace].slice(0, TRACE_LIMIT),
    };
  }
  applyLiveAssistantEvent(event.trace);
}

function handleOpsRailWidth(width: number) {
  opsRailWidth.value = width;
}

function appendLocalMessage(message: GuiMessage) {
  if (!workbench.value) {
    return;
  }

  workbench.value = {
    ...workbench.value,
    conversation: {
      ...workbench.value.conversation,
      messageCount: workbench.value.conversation.messageCount + 1,
      messages: [...workbench.value.conversation.messages, message],
    },
  };
}

function applyLiveAssistantEvent(event: TraceEvent) {
  if (!requestPending.value) {
    return;
  }

  switch (event.kind) {
    case "token":
      appendLiveToken(event.detail);
      break;
    case "think":
      replaceLiveAssistant(event.detail);
      break;
    case "tool_call":
      showLiveStatus(`${LIVE_TOOL_CALL_PREFIX}：${event.title}`);
      break;
    case "tool_confirm":
      showLiveStatus(`${LIVE_TOOL_CONFIRM_PREFIX}：${event.title}`);
      break;
    case "tool_result":
      showLiveStatus(`${LIVE_TOOL_RESULT_PREFIX}：${event.title}`);
      break;
    case "state":
      showLiveStatus(`${LIVE_STATE_PREFIX}：${event.title}`);
      break;
    case "error":
      replaceLiveAssistant(event.detail || LIVE_ERROR_FALLBACK);
      break;
  }
}

function appendLiveToken(delta: string) {
  if (!delta) {
    return;
  }
  liveAssistantContent.value = liveAssistantSynthetic.value
    ? delta
    : `${liveAssistantContent.value}${delta}`;
  liveAssistantSynthetic.value = false;
}

function replaceLiveAssistant(content: string) {
  if (!content) {
    return;
  }
  liveAssistantContent.value = content;
  liveAssistantSynthetic.value = false;
}

function showLiveStatus(content: string) {
  if (!content || (liveAssistantContent.value && !liveAssistantSynthetic.value)) {
    return;
  }
  liveAssistantContent.value = content;
  liveAssistantSynthetic.value = true;
}

function resetLiveAssistant() {
  liveAssistantContent.value = "";
  liveAssistantSynthetic.value = false;
}

function setError(error: unknown) {
  errorText.value = error instanceof Error ? error.message : String(error);
}

function isPersistentTraceEvent(event: TraceEvent): boolean {
  return event.kind !== "token" && event.kind !== "think";
}
</script>

<template>
  <main
    class="grid h-screen bg-[#f4f1ea] bg-[linear-gradient(90deg,rgba(39,87,71,0.08)_0_1px,transparent_1px_100%),linear-gradient(180deg,rgba(39,87,71,0.06)_0_1px,transparent_1px_100%)] [background-size:34px_34px]"
    :style="shellStyle"
    data-testid="app-shell"
  >
    <SessionRail
      :sessions="sessions"
      :active-session-id="activeSessionId"
      :disabled="runBusy"
      @new-session="handleNewSession"
      @refresh="refreshWorkbench"
      @switch-session="handleSwitchSession"
      @archive-session="handleArchiveSession"
    />

    <ConversationPane
      :messages="messages"
      :project="project"
      :run="run"
      :busy="runBusy"
      :switching-work-dir="switchingWorkDir"
      :error-text="errorText"
      :active-session-label="activeSessionLabel"
      @submit-message="handleSubmit"
      @apply-work-dir="handleApplyWorkDir"
      @browse-work-dir="handleBrowseWorkDir"
    />

    <OpsRail
      :trace="trace"
      :tools="tools"
      :findings="findings"
      :width="opsRailWidth"
      @update-width="handleOpsRailWidth"
    />

    <CommandApprovalDialog
      :approval="pendingApproval"
      :busy="approvalPending"
      :error-text="approvalErrorText"
      :left-inset-px="SESSION_RAIL_WIDTH"
      :right-inset-px="opsRailWidth"
      @resolve="handleResolveApproval"
    />
  </main>
</template>
