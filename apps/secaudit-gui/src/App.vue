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
  GuiTokenUsage,
  TraceEvent,
} from "./types";

const LIVE_ERROR_FALLBACK = "本轮运行失败，详情见运行轨迹和错误提示。";

const workbench = ref<AgentWorkbench | null>(null);
const requestPending = ref(false);
const liveAssistantContent = ref("");
const liveAssistantMessages = ref<Array<GuiMessage>>([]);
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
  if (!requestPending.value) {
    return backendMessages.value;
  }
  const liveMessages = [...backendMessages.value, ...liveAssistantMessages.value];
  if (!liveAssistantContent.value) {
    return liveMessages;
  }
  return [...liveMessages, createAssistantMessage(liveAssistantContent.value, true, false)];
});
const trace = computed<Array<TraceEvent>>(() => workbench.value?.trace ?? []);
const sessions = computed(() => workbench.value?.conversation.sessions ?? []);
const skills = computed(() => workbench.value?.skills ?? []);
const tools = computed(() => workbench.value?.tools ?? []);
const findings = computed(() => workbench.value?.findings ?? []);
const project = computed(() => workbench.value?.project ?? null);
const run = computed(() => workbench.value?.run ?? null);
const status = computed(() => workbench.value?.status ?? null);
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
    resetLiveAssistant();
    resetLiveAssistantMessages();
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
  resetLiveAssistantMessages();
  appendLocalMessage({
    role: "user",
    content: message,
    streaming: false,
    continuesWithTool: false,
  });

  try {
    const nextWorkbench = await sendAuditMessage(message);
    resetLiveAssistant();
    workbench.value = nextWorkbench;
  } catch (error) {
    resetLiveAssistant();
    addLiveAssistantMessage(error instanceof Error ? error.message : String(error), false);
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
    resetLiveAssistant();
    resetLiveAssistantMessages();
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
    resetLiveAssistant();
    resetLiveAssistantMessages();
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
  resetLiveAssistant();
  resetLiveAssistantMessages();
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
    resetLiveAssistant();
    resetLiveAssistantMessages();
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

  applyLiveTokenUsage(event.tokenUsage);
  if (isPersistentTraceEvent(event.trace)) {
    workbench.value = {
      ...workbench.value,
      trace: [event.trace, ...workbench.value.trace].slice(0, TRACE_LIMIT),
    };
  }
  applyLiveAssistantEvent(event.trace);
}

function applyLiveTokenUsage(usage: GuiTokenUsage | null) {
  if (!usage || !requestPending.value || !workbench.value) {
    return;
  }

  const tokenUsage = workbench.value.status.tokenUsage;
  workbench.value = {
    ...workbench.value,
    status: {
      ...workbench.value.status,
      tokenUsage: {
        prompt: tokenUsage.prompt + usage.prompt,
        completion: tokenUsage.completion + usage.completion,
        total: tokenUsage.total + usage.total,
      },
    },
  };
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
      applyLiveAssistantText(event.detail);
      break;
    case "tool_call":
      commitLiveContentAsToolBlock();
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
  liveAssistantContent.value = `${liveAssistantContent.value}${delta}`;
}

function replaceLiveAssistant(content: string) {
  if (!content) {
    return;
  }
  liveAssistantContent.value = content;
}

function applyLiveAssistantText(content: string) {
  const normalized = content.trim();
  if (liveAssistantContent.value.trim()) {
    return;
  }
  addLiveAssistantMessage(normalized, false);
}

function commitLiveContentAsToolBlock() {
  addLiveAssistantMessage(liveAssistantContent.value, true);
  liveAssistantContent.value = "";
}

function addLiveAssistantMessage(content: string, continuesWithTool: boolean) {
  const normalized = content.trim();
  if (!normalized) {
    return;
  }
  const last = liveAssistantMessages.value.at(-1);
  if (last?.content === normalized) {
    return;
  }
  liveAssistantMessages.value = [
    ...liveAssistantMessages.value,
    createAssistantMessage(normalized, false, continuesWithTool),
  ];
}

function resetLiveAssistantMessages() {
  liveAssistantMessages.value = [];
}

function resetLiveAssistant() {
  liveAssistantContent.value = "";
}

function createAssistantMessage(
  content: string,
  streaming: boolean,
  continuesWithTool: boolean,
): GuiMessage {
  return {
    role: "assistant",
    content,
    streaming,
    continuesWithTool,
  };
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
      :status="status"
      :skills="skills"
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
