<script setup lang="ts">
import { Archive, MessageSquarePlus, RefreshCw, ShieldCheck } from "lucide-vue-next";
import { computed, ref } from "vue";
import { SESSION_ID_PREVIEW_LENGTH } from "../constants";
import type { SessionListItem } from "../types";
import { formatSessionAbsoluteTimeLabel, formatSessionTimeLabel } from "../utils/time";

const props = defineProps<{
  sessions: Array<SessionListItem>;
  activeSessionId: string | null;
  disabled: boolean;
}>();

defineEmits<{
  newSession: [];
  refresh: [];
  switchSession: [id: string];
  archiveSession: [id: string];
}>();

type SessionFilterId = "all" | "active" | "archived";

const activeSessionFilter = ref<SessionFilterId>("all");
const SESSION_FILTERS: Array<{
  id: SessionFilterId;
  label: string;
  status: string | null;
}> = [
  { id: "all", label: "全部", status: null },
  { id: "active", label: "活跃", status: "active" },
  { id: "archived", label: "归档", status: "archived" },
];

const sessionFilters = computed(() =>
  SESSION_FILTERS.map((filter) => ({
    ...filter,
    count: filter.status
      ? props.sessions.filter((item) => item.status === filter.status).length
      : props.sessions.length,
  })),
);

const filteredSessions = computed(() => {
  const filter = SESSION_FILTERS.find((item) => item.id === activeSessionFilter.value);
  if (!filter?.status) {
    return props.sessions;
  }
  return props.sessions.filter((item) => item.status === filter.status);
});

const activeSessionFilterLabel = computed(
  () => sessionFilters.value.find((filter) => filter.id === activeSessionFilter.value)?.label ?? "全部",
);

function selectSessionFilter(id: SessionFilterId) {
  activeSessionFilter.value = id;
}

function sessionTitle(item: SessionListItem): string {
  return item.title || item.id.slice(0, SESSION_ID_PREVIEW_LENGTH);
}

function sessionStatusLabel(status: string): string {
  const labels: Record<string, string> = {
    active: "活跃",
    archived: "归档",
  };
  return labels[status] ?? status;
}

function sessionStatusClass(status: string): string {
  const base = "rounded-full px-1.5 py-0.5 text-[10px] font-black";
  if (status === "active") {
    return `${base} bg-[#e3efe6] text-[#255744]`;
  }
  if (status === "archived") {
    return `${base} bg-[#eee7dc] text-[#5a5f58]`;
  }
  return `${base} bg-[#fff1d5] text-[#7a4b00]`;
}

function sessionUpdatedLabel(item: SessionListItem): string {
  return formatSessionTimeLabel(item.updatedAt);
}

function sessionUpdatedTitle(item: SessionListItem): string {
  return formatSessionAbsoluteTimeLabel(item.updatedAt);
}
</script>

<template>
  <aside
    class="flex min-h-0 flex-col border-r border-[rgba(39,48,40,0.18)] bg-[rgba(247,244,236,0.94)] p-5"
  >
    <div class="flex items-center gap-3 text-[#173b32]">
      <ShieldCheck :size="24" />
      <div>
        <p class="m-0 text-[17px] font-black tracking-normal">SecAudit Agent</p>
        <p class="m-0 mt-0.5 text-xs text-[#667166]">代码安全审计工作台</p>
      </div>
    </div>

    <button
      type="button"
      class="mt-[22px] inline-flex w-full items-center justify-center gap-2 rounded-lg border-0 bg-[#173b32] px-3.5 py-[11px] font-extrabold text-[#fffaf0]"
      :disabled="disabled"
      @click="$emit('newSession')"
    >
      <MessageSquarePlus :size="18" />
      <span>新建审计会话</span>
    </button>

    <section class="mt-6 flex min-h-0 flex-1 flex-col">
      <div class="flex items-center justify-between text-[13px] font-black text-[#334235]">
        <span>会话历史</span>
        <button
          type="button"
          class="inline-flex size-7 items-center justify-center rounded-[7px] border-0 bg-[#e9e2d4] text-[#364638]"
          aria-label="刷新工作台"
          :disabled="disabled"
          @click="$emit('refresh')"
        >
          <RefreshCw :size="15" />
        </button>
      </div>

      <div class="mt-3 grid grid-cols-3 gap-1.5" data-testid="session-filters">
        <button
          v-for="filter in sessionFilters"
          :key="filter.id"
          type="button"
          class="grid min-w-0 justify-items-center rounded-lg border px-1.5 py-1.5 text-[11px] font-black"
          :class="
            filter.id === activeSessionFilter
              ? 'border-[#2f765e] bg-[#e3efe6] text-[#255744]'
              : 'border-[rgba(39,48,40,0.13)] bg-[rgba(255,252,244,0.7)] text-[#4c584d]'
          "
          :aria-pressed="filter.id === activeSessionFilter"
          :data-testid="`session-filter-${filter.id}`"
          @click="selectSessionFilter(filter.id)"
        >
          <span>{{ filter.label }}</span>
          <span class="mt-0.5 font-black tabular-nums">{{ filter.count }}</span>
        </button>
      </div>

      <div class="mt-3 flex min-h-0 flex-1 flex-col gap-2 overflow-auto" data-testid="session-list">
        <article
          v-for="item in filteredSessions"
          :key="item.id"
          class="grid grid-cols-[minmax(0,1fr)_auto] gap-2 rounded-lg border p-[11px]"
          :class="
            item.id === activeSessionId
              ? 'border-[#2f765e] bg-[#e7f1ea]'
              : 'border-[rgba(39,48,40,0.13)] bg-[rgba(255,252,244,0.7)]'
          "
          :aria-current="item.id === activeSessionId"
          data-testid="session-item"
        >
          <button
            type="button"
            class="grid min-w-0 gap-[5px] text-left"
            :disabled="disabled"
            data-testid="session-switch"
            @click="$emit('switchSession', item.id)"
          >
            <span class="flex min-w-0 items-center gap-1.5">
              <span
                class="min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap text-[13px] font-black text-[#20291f]"
              >
                {{ sessionTitle(item) }}
              </span>
              <span :class="sessionStatusClass(item.status)">{{ sessionStatusLabel(item.status) }}</span>
            </span>
            <span
              class="overflow-hidden text-xs leading-[1.45] text-[#4c584d] [display:-webkit-box] [-webkit-box-orient:vertical] [-webkit-line-clamp:2]"
            >
              {{ item.preview }}
            </span>
            <span class="text-xs text-[#667166]" :title="sessionUpdatedTitle(item)">
              {{ item.messageCount }} 条 · {{ sessionUpdatedLabel(item) }}
            </span>
          </button>
          <button
            v-if="item.canArchive"
            type="button"
            class="inline-flex size-7 items-center justify-center rounded-[7px] border border-[rgba(39,48,40,0.12)] bg-[#eee7dc] text-[#5a4631]"
            :disabled="disabled"
            :aria-label="`归档会话 ${sessionTitle(item)}`"
            data-testid="session-archive-button"
            @click="$emit('archiveSession', item.id)"
          >
            <Archive :size="14" />
          </button>
        </article>
        <p v-if="sessions.length === 0" class="text-xs text-[#667166]">暂无审计会话</p>
        <p v-else-if="filteredSessions.length === 0" class="text-xs text-[#667166]">
          {{ activeSessionFilterLabel }} 暂无会话
        </p>
      </div>
    </section>
  </aside>
</template>
