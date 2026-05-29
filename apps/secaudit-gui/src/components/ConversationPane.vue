<script setup lang="ts">
import {
  AlertCircle,
  Bot,
  CheckCircle2,
  FolderOpen,
  LoaderCircle,
  Send,
  Terminal,
} from "lucide-vue-next";
import { computed, nextTick, ref, watch } from "vue";
import { TEXTAREA_MAX_HEIGHT, TEXTAREA_MIN_HEIGHT } from "../constants";
import type { GuiMessage, ProjectPanel, RunPanel } from "../types";
import { renderMarkdown } from "../utils/markdown";

const props = defineProps<{
  messages: Array<GuiMessage>;
  project: ProjectPanel | null;
  run: RunPanel | null;
  busy: boolean;
  switchingWorkDir: boolean;
  errorText: string | null;
  activeSessionLabel: string;
}>();

const emit = defineEmits<{
  submitMessage: [message: string];
  applyWorkDir: [workDir: string];
  browseWorkDir: [currentWorkDir: string];
}>();

const requestText = ref("");
const workDirInput = ref("");
const chatViewport = ref<HTMLElement | null>(null);
const inputArea = ref<HTMLTextAreaElement | null>(null);

const isRunning = computed(() => props.busy || Boolean(props.run?.busy));
const canSend = computed(() => {
  return Boolean(props.run?.canSend && requestText.value.trim().length > 0 && !isRunning.value);
});

const canApplyWorkDir = computed(() => {
  const nextDir = workDirInput.value.trim();
  return Boolean(
    nextDir.length > 0 &&
      nextDir !== props.project?.workDir &&
      !isRunning.value &&
      !props.switchingWorkDir,
  );
});
const canBrowseWorkDir = computed(() => !isRunning.value && !props.switchingWorkDir);

const composerDisabled = computed(() => !props.run || isRunning.value);
const composerPlaceholder = computed(() =>
  props.run?.canSend
    ? "例如：审计当前仓库的命令执行、路径穿越和依赖风险"
    : "可先写下审计目标，配置完成后即可发送",
);
const runStatusLabel = computed(() =>
  isRunning.value ? props.run?.pendingLabel || "运行中" : props.run?.label || "初始化",
);
const runStatusDetail = computed(() =>
  isRunning.value ? props.run?.pendingDetail || "" : props.run?.statusDetail || "",
);
const sendButtonLabel = computed(() => props.run?.primaryActionLabel || "发送审计请求");

watch(
  () => props.project?.workDir,
  (workDir) => {
    workDirInput.value = workDir ?? "";
  },
  { immediate: true },
);

watch(
  [
    () => props.messages.length,
    () => lastMessageContent(),
    () => props.errorText,
    () => props.project?.configError,
  ],
  () => {
    void scrollChatToBottom();
  },
);

function submitMessage() {
  const message = requestText.value.trim();
  if (!canSend.value) {
    return;
  }

  requestText.value = "";
  resizeInput();
  emit("submitMessage", message);
}

function applyWorkDir() {
  const nextDir = workDirInput.value.trim();
  if (!canApplyWorkDir.value) {
    return;
  }
  emit("applyWorkDir", nextDir);
}

function browseWorkDir() {
  if (!canBrowseWorkDir.value) {
    return;
  }
  emit("browseWorkDir", workDirInput.value.trim() || props.project?.workDir || "");
}

function resizeInput() {
  if (!inputArea.value) {
    return;
  }
  inputArea.value.style.height = `${TEXTAREA_MIN_HEIGHT}px`;
  inputArea.value.style.height = `${Math.min(inputArea.value.scrollHeight, TEXTAREA_MAX_HEIGHT)}px`;
}

function handleInputKeydown(event: KeyboardEvent) {
  if (event.key !== "Enter" || event.shiftKey || event.ctrlKey || event.metaKey) {
    return;
  }
  event.preventDefault();
  submitMessage();
}

function roleLabel(message: GuiMessage) {
  return message.role === "user" ? "你" : "Agent";
}

function assistantMarkdown(content: string): string {
  return renderMarkdown(content);
}

function lastMessageContent(): string {
  const lastIndex = props.messages.length - 1;
  if (lastIndex < 0) {
    return "";
  }
  return props.messages[lastIndex]?.content ?? "";
}

function messageRowClass(message: GuiMessage): string {
  return message.role === "user"
    ? "grid grid-cols-[minmax(0,780px)_34px] justify-end gap-3 mb-5"
    : "grid grid-cols-[34px_minmax(0,780px)] gap-3 mb-5";
}

function avatarClass(message: GuiMessage): string {
  const base = "flex size-[34px] items-center justify-center rounded-lg";
  return message.role === "user"
    ? `${base} col-start-2 row-start-1 bg-[#1f4f41] text-[#fffaf0]`
    : `${base} bg-[#dfd7c5] text-[#284437]`;
}

function messageBodyClass(message: GuiMessage): string {
  const base =
    "rounded-lg border border-[rgba(39,48,40,0.13)] px-[15px] py-3.5 shadow-[0_16px_34px_rgba(44,52,42,0.07)]";
  return message.role === "user"
    ? `${base} col-start-1 row-start-1 bg-[#213f36] text-[#fffaf0]`
    : `${base} bg-[rgba(255,252,244,0.88)]`;
}

function statusPillClass(): string {
  const base = "inline-flex min-h-[30px] items-center gap-1.5 rounded-full px-[11px] text-xs font-black";
  if (isRunning.value) {
    return `${base} bg-[#fff1d5] text-[#7a4b00]`;
  }
  return props.run?.phase === "ready"
    ? `${base} bg-[#e3efe6] text-[#255744]`
    : `${base} bg-[#fff1d5] text-[#7a4b00]`;
}

async function scrollChatToBottom() {
  await nextTick();
  if (!chatViewport.value) {
    return;
  }
  chatViewport.value.scrollTop = chatViewport.value.scrollHeight;
}
</script>

<template>
  <section class="grid min-h-0 min-w-0 grid-rows-[auto_minmax(0,1fr)_auto]">
    <header
      class="flex items-center justify-between gap-5 border-b border-[rgba(39,48,40,0.16)] bg-[rgba(255,252,244,0.8)] px-[22px] py-4"
    >
      <form class="grid min-w-0 flex-1 gap-1.5" @submit.prevent="applyWorkDir">
        <span class="block text-xs font-black text-[#667166]">工作区</span>
        <div class="grid grid-cols-[minmax(0,1fr)_34px_auto] gap-2">
          <input
            v-model="workDirInput"
            :disabled="isRunning || switchingWorkDir"
            type="text"
            spellcheck="false"
            class="h-[34px] min-w-0 rounded-lg border border-[rgba(39,48,40,0.16)] bg-[rgba(255,253,247,0.9)] px-2.5 text-sm text-[#18251b] outline-none focus:border-[#2f765e] focus:shadow-[0_0_0_3px_rgba(47,118,94,0.14)]"
            data-testid="work-dir-input"
          />
          <button
            type="button"
            class="inline-flex h-[34px] items-center justify-center rounded-lg border border-[rgba(39,48,40,0.16)] bg-[rgba(255,253,247,0.9)] text-[#364638] hover:border-[#8da795]"
            :disabled="!canBrowseWorkDir"
            title="选择工作区目录"
            aria-label="选择工作区目录"
            data-testid="work-dir-picker-button"
            @click="browseWorkDir"
          >
            <FolderOpen :size="15" />
          </button>
          <button
            type="submit"
            class="inline-flex h-[34px] items-center justify-center gap-1.5 rounded-lg border-0 bg-[#e9e2d4] px-3 text-xs font-black text-[#26382d]"
            :disabled="!canApplyWorkDir"
            data-testid="work-dir-button"
          >
            <FolderOpen v-if="!switchingWorkDir" :size="15" />
            <LoaderCircle v-else :size="15" class="animate-spin" />
            <span>应用</span>
          </button>
        </div>
      </form>

      <div class="flex min-w-[260px] flex-none items-center justify-end gap-2">
        <div class="min-w-0 text-right">
          <div class="flex justify-end gap-2">
            <span :class="statusPillClass()">
              <LoaderCircle v-if="isRunning" :size="14" class="animate-spin" />
              <CheckCircle2 v-else-if="run?.phase === 'ready'" :size="14" />
              <AlertCircle v-else :size="14" />
              {{ runStatusLabel }}
            </span>
            <span
              class="inline-flex min-h-[30px] items-center gap-1.5 rounded-full bg-[#20291f] px-[11px] text-xs font-black text-[#fffaf0]"
            >
              #{{ activeSessionLabel }}
            </span>
          </div>
          <p
            v-if="runStatusDetail"
            class="mt-1 max-w-[360px] overflow-hidden text-ellipsis whitespace-nowrap text-xs text-[#667166]"
          >
            {{ runStatusDetail }}
          </p>
        </div>
      </div>
    </header>

    <div ref="chatViewport" class="min-h-0 overflow-auto scroll-smooth px-[26px] pt-[26px] pb-[18px]">
      <div
        v-if="project?.configError"
        class="mb-4 flex items-start gap-2.5 rounded-lg border border-[rgba(153,45,38,0.26)] bg-[#fff1ee] px-3.5 py-3 text-[13px] leading-6 text-[#8b2a22]"
      >
        <AlertCircle :size="18" />
        <span>{{ project.configError }}</span>
      </div>

      <div
        v-if="messages.length === 0"
        class="grid min-h-[360px] place-content-center justify-items-center text-[#657066]"
      >
        <Bot :size="40" />
        <p class="mt-3 max-w-[360px] text-center font-extrabold leading-[1.6]">
          输入审计目标，Agent 会规划路径并调用工具收集证据。
        </p>
      </div>

      <article
        v-for="(message, index) in messages"
        :key="`${message.role}-${index}-${message.content.length}`"
        :class="messageRowClass(message)"
      >
        <div :class="avatarClass(message)">
          <Bot v-if="message.role === 'assistant'" :size="18" />
          <Terminal v-else :size="18" />
        </div>
        <div :class="messageBodyClass(message)">
          <div class="mb-2 text-xs font-black text-inherit opacity-75">{{ roleLabel(message) }}</div>
          <div
            v-if="message.role === 'assistant'"
            class="markdown-body"
            data-testid="assistant-markdown"
            v-html="assistantMarkdown(message.content)"
          />
          <p v-else class="m-0 whitespace-pre-wrap leading-[1.65] [overflow-wrap:anywhere]">
            {{ message.content }}
          </p>
        </div>
      </article>

      <div
        v-if="errorText"
        class="mb-4 flex items-start gap-2.5 rounded-lg border border-[rgba(153,45,38,0.26)] bg-[#fff1ee] px-3.5 py-3 text-[13px] leading-6 text-[#8b2a22]"
      >
        <AlertCircle :size="18" />
        <span>{{ errorText }}</span>
      </div>
    </div>

    <footer
      class="grid grid-cols-[minmax(0,1fr)_48px] gap-2.5 border-t border-[rgba(39,48,40,0.16)] bg-[rgba(255,252,244,0.84)] px-[18px] py-[15px]"
    >
      <textarea
        ref="inputArea"
        v-model="requestText"
        :disabled="composerDisabled"
        rows="1"
        :placeholder="composerPlaceholder"
        class="min-h-11 max-h-[148px] resize-none rounded-lg border border-[rgba(39,48,40,0.2)] bg-[#fffdf7] px-3 py-[11px] leading-[1.45] text-[#1c211c] outline-none focus:border-[#2f765e] focus:shadow-[0_0_0_3px_rgba(47,118,94,0.14)]"
        data-testid="composer-input"
        @input="resizeInput"
        @keydown="handleInputKeydown"
      />
      <button
        type="button"
        class="inline-flex size-12 items-center justify-center rounded-lg border-0 bg-[#c6562f] text-[#fffaf0]"
        :disabled="!canSend"
        data-testid="send-button"
        @click="submitMessage"
        :title="sendButtonLabel"
      >
        <Send :size="18" />
      </button>
    </footer>
  </section>
</template>
