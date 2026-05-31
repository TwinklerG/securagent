<script setup lang="ts">
import { CheckCircle2, LoaderCircle, ShieldAlert, Terminal, XCircle } from "lucide-vue-next";
import { computed } from "vue";
import type { CommandApprovalRequest } from "../types";

const props = defineProps<{
  approval: CommandApprovalRequest | null;
  busy: boolean;
  errorText: string | null;
  leftInsetPx: number;
  rightInsetPx: number;
}>();

const emit = defineEmits<{
  resolve: [approved: boolean];
}>();

const DIALOG_EDGE_GAP_PX = 16;

const dialogStyle = computed(() => ({
  left: `${props.leftInsetPx + DIALOG_EDGE_GAP_PX}px`,
  right: `${props.rightInsetPx + DIALOG_EDGE_GAP_PX}px`,
}));

function resolve(approved: boolean) {
  if (!props.approval || props.busy) {
    return;
  }
  emit("resolve", approved);
}
</script>

<template>
  <Teleport to="body">
    <div
      v-if="approval"
      class="pointer-events-none fixed bottom-4 z-50 flex justify-center"
      data-testid="command-approval-dialog"
      role="region"
      aria-labelledby="command-approval-title"
      :style="dialogStyle"
    >
      <section
        class="pointer-events-auto grid w-full max-w-[760px] grid-cols-[minmax(0,1fr)_auto] gap-3 rounded-lg border border-[#d7b261] bg-[#fffaf0] p-4 shadow-[0_20px_70px_rgba(16,23,18,0.24)]"
      >
        <header class="flex min-w-0 items-start gap-3">
          <span class="grid size-10 shrink-0 place-items-center rounded-lg bg-[#fff1d5] text-[#805100]">
            <ShieldAlert :size="22" />
          </span>
          <div class="grid min-w-0 gap-1">
            <h2 id="command-approval-title" class="text-sm font-black text-[#221a0e]">
              命令执行确认
            </h2>
            <p class="text-xs leading-5 text-[#62440f]">
              本轮审计正在等待你的决定；拒绝后 Agent 会收到工具失败结果。
            </p>
          </div>
        </header>

        <div class="col-start-1 rounded-lg border border-[#e1c27c] bg-[#fff6df] p-2.5">
          <div class="mb-1.5 flex items-center gap-2 text-xs font-black text-[#62440f]">
            <Terminal :size="15" />
            <span>待执行内容</span>
          </div>
          <pre
            class="max-h-28 overflow-auto whitespace-pre-wrap text-xs leading-5 text-[#2a2114] [overflow-wrap:anywhere]"
            data-testid="command-approval-prompt"
          >{{ approval.prompt }}</pre>
        </div>

        <p v-if="errorText" class="col-start-1 rounded-lg bg-[#fff1ee] px-3 py-2 text-sm font-bold text-[#9b2d25]">
          {{ errorText }}
        </p>

        <footer class="col-start-2 row-span-3 row-start-1 flex min-w-[120px] flex-col justify-end gap-2">
          <button
            type="button"
            class="inline-flex min-h-[38px] items-center justify-center gap-1.5 rounded-lg border border-[#2f765e] bg-[#245f4b] px-4 text-sm font-black text-[#fffaf0] hover:bg-[#1f4f41] disabled:cursor-not-allowed disabled:opacity-60"
            :disabled="busy"
            data-testid="command-approval-approve"
            @click="resolve(true)"
          >
            <CheckCircle2 v-if="!busy" :size="16" />
            <LoaderCircle v-else :size="16" class="animate-spin" />
            <span>允许</span>
          </button>
          <button
            type="button"
            class="inline-flex min-h-[38px] items-center justify-center gap-1.5 rounded-lg border border-[#d8847b] bg-[#fff1ee] px-4 text-sm font-black text-[#9b2d25] hover:bg-[#ffe6e1] disabled:cursor-not-allowed disabled:opacity-60"
            :disabled="busy"
            data-testid="command-approval-deny"
            @click="resolve(false)"
          >
            <XCircle v-if="!busy" :size="16" />
            <LoaderCircle v-else :size="16" class="animate-spin" />
            <span>拒绝</span>
          </button>
        </footer>
      </section>
    </div>
  </Teleport>
</template>
