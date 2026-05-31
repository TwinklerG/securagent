<script setup lang="ts">
import {
  ChevronDown,
  ChevronRight,
  ClipboardCopy,
  FileSearch,
  History,
  ListFilter,
  ShieldAlert,
  Wrench,
} from "lucide-vue-next";
import { computed, nextTick, ref, watch } from "vue";
import { OPS_RAIL_MAX_WIDTH, OPS_RAIL_MIN_WIDTH } from "../constants";
import type { FindingPreview, ToolCapability, ToolParameter, TraceEvent } from "../types";
import { formatTraceTimeLabel } from "../utils/time";

const props = defineProps<{
  trace: Array<TraceEvent>;
  tools: Array<ToolCapability>;
  findings: Array<FindingPreview>;
  width: number;
}>();

const emit = defineEmits<{
  updateWidth: [width: number];
}>();

const traceViewport = ref<HTMLElement | null>(null);
const activeTab = ref<OpsTabId>("trace");
const activeTraceFilter = ref<TraceFilterId>("all");
const expandedTraceIds = ref<Set<number>>(new Set());
const resizing = ref(false);
let resizeStartX = 0;
let resizeStartWidth = 0;

type OpsTabId = "trace" | "confirm" | "tools" | "findings";
type TraceFilterId = "all" | "state" | "tool" | "error";
type ToolRiskSummary = {
  label: string;
  count: number;
};
type ToolGroup = {
  category: string;
  count: number;
  risks: Array<ToolRiskSummary>;
  tools: Array<ToolCapability>;
};
type ToolConfirmStatusId = "pending" | "approved" | "denied" | "timeout" | "record";
type TraceDetailField = {
  key: string;
  label: string;
  value: string;
};

const TOOL_RISK_BADGE_BASE =
  "inline-flex items-center rounded-full border px-1.5 py-0.5 text-[10px] font-black";
const TOOL_RISK_CLASS_BY_LABEL: Record<string, string> = {
  只读: "border-[#b8cec1] bg-[#e5efe7] text-[#2f765e]",
  网络: "border-[#b8c8d5] bg-[#e4ecf2] text-[#365d78]",
  需确认: "border-[#e1c27c] bg-[#fff1d5] text-[#805100]",
};
const DEFAULT_TOOL_RISK_CLASS = "border-[#cfc6b8] bg-[#eee7dc] text-[#4c584d]";
const TOOL_PARAMETER_TYPE_LABEL_BY_NAME: Record<string, string> = {
  string: "文本",
  integer: "整数",
  number: "数字",
  boolean: "布尔",
  unknown: "未知",
};
const FINDING_BADGE_BASE =
  "inline-flex items-center rounded-full border px-2 py-1 text-[11px] font-black";
const FINDING_SEVERITY_CLASS_BY_VALUE: Record<FindingPreview["severity"], string> = {
  pending: "border-[#e1c27c] bg-[#fff1d5] text-[#805100]",
  low: "border-[#b8cec1] bg-[#e5efe7] text-[#2f765e]",
  medium: "border-[#d8c174] bg-[#fff6d6] text-[#79610e]",
  high: "border-[#dd9c75] bg-[#fff0e4] text-[#944315]",
  critical: "border-[#d8847b] bg-[#fff1ee] text-[#9b2d25]",
};
const FINDING_STATUS_CLASS_BY_VALUE: Record<FindingPreview["status"], string> = {
  candidate: "border-[#cfc6b8] bg-[#eee7dc] text-[#4c584d]",
  confirmed: "border-[#d8847b] bg-[#fff1ee] text-[#9b2d25]",
  dismissed: "border-[#b8c8d5] bg-[#e4ecf2] text-[#365d78]",
};
const TRACE_DETAIL_PREVIEW_LENGTH = 180;
const TRACE_LOW_VALUE_DETAILS = new Set(["Agent 状态已更新"]);
const TRACE_KIND_LABEL_BY_VALUE: Record<TraceEvent["kind"], string> = {
  state: "状态",
  think: "思考",
  token: "流式",
  tool_call: "调用",
  tool_confirm: "确认",
  tool_result: "结果",
  error: "错误",
};
const TRACE_KIND_BADGE_BASE =
  "inline-flex min-w-[38px] items-center justify-center rounded-full border px-2 py-0.5 text-[11px] font-black";
const TRACE_KIND_BADGE_CLASS_BY_VALUE: Record<TraceEvent["kind"], string> = {
  state: "border-[#cfc6b8] bg-[#eee7dc] text-[#4c584d]",
  think: "border-[#cfc6b8] bg-[#eee7dc] text-[#4c584d]",
  token: "border-[#cfc6b8] bg-[#eee7dc] text-[#4c584d]",
  tool_call: "border-[#b8cec1] bg-[#e5efe7] text-[#2f765e]",
  tool_confirm: "border-[#e1c27c] bg-[#fff1d5] text-[#805100]",
  tool_result: "border-[#b8c8d5] bg-[#e4ecf2] text-[#365d78]",
  error: "border-[#d8847b] bg-[#fff1ee] text-[#9b2d25]",
};
const TOOL_CONFIRM_STATUS_LABEL_BY_ID: Record<ToolConfirmStatusId, string> = {
  pending: "待确认",
  approved: "已允许",
  denied: "已拒绝",
  timeout: "超时拒绝",
  record: "记录",
};
const TOOL_CONFIRM_STATUS_CLASS_BY_ID: Record<ToolConfirmStatusId, string> = {
  pending: "border-[#e1c27c] bg-[#fff1d5] text-[#805100]",
  approved: "border-[#b8cec1] bg-[#e5efe7] text-[#2f765e]",
  denied: "border-[#d8847b] bg-[#fff1ee] text-[#9b2d25]",
  timeout: "border-[#dd9c75] bg-[#fff0e4] text-[#944315]",
  record: "border-[#cfc6b8] bg-[#eee7dc] text-[#4c584d]",
};
const TOOL_CONFIRM_STATUS_BADGE_BASE =
  "inline-flex shrink-0 items-center rounded-full border px-2 py-0.5 text-[11px] font-black";
const TOOL_CONFIRM_WAITING_DETAILS = [
  "等待你在主确认弹窗中批准或拒绝。右侧确认页会保留历史记录。",
  "等待用户确认。",
  "用户选择：已允许。",
  "用户选择：已拒绝。",
  "批准请求超时，已按拒绝处理。",
  "预览确认结果：已允许。",
  "预览确认结果：已拒绝。",
];

const TRACE_FILTERS: Array<{
  id: TraceFilterId;
  label: string;
  kinds: Array<TraceEvent["kind"]>;
}> = [
  {
    id: "all",
    label: "全部",
    kinds: ["state", "tool_call", "tool_confirm", "tool_result", "error"],
  },
  {
    id: "state",
    label: "状态",
    kinds: ["state"],
  },
  {
    id: "tool",
    label: "工具",
    kinds: ["tool_call", "tool_confirm", "tool_result"],
  },
  {
    id: "error",
    label: "错误",
    kinds: ["error"],
  },
];

const traceFilters = computed(() =>
  TRACE_FILTERS.map((filter) => ({
    ...filter,
    count: countTraceByKinds(filter.kinds),
  })),
);

const filteredTrace = computed(() => {
  const filter = TRACE_FILTERS.find((item) => item.id === activeTraceFilter.value);
  if (!filter) {
    return props.trace;
  }
  return props.trace.filter((item) => filter.kinds.includes(item.kind));
});

const toolGroups = computed<Array<ToolGroup>>(() => groupToolsByCategory(props.tools));
const toolConfirmEvents = computed(() => mergeToolConfirmEvents(props.trace));
const tabItems = computed(() => [
  { id: "trace" as const, label: "轨迹", count: props.trace.length, icon: History },
  { id: "confirm" as const, label: "确认", count: toolConfirmEvents.value.length, icon: ShieldAlert },
  { id: "tools" as const, label: "工具", count: props.tools.length, icon: Wrench },
  { id: "findings" as const, label: "发现", count: props.findings.length, icon: FileSearch },
]);

const activeTraceFilterLabel = computed(
  () => traceFilters.value.find((filter) => filter.id === activeTraceFilter.value)?.label ?? "全部",
);
const filteredTraceSummaryLabel = computed(
  () => `${activeTraceFilterLabel.value} ${filteredTrace.value.length} / ${props.trace.length}`,
);
const traceSummary = computed(() => ({
  total: props.trace.length,
  tool: countTraceByKinds(["tool_call", "tool_confirm", "tool_result"]),
  error: countTraceByKinds(["error"]),
}));

watch(
  () => filteredTrace.value[0]?.id,
  () => {
    void scrollTraceToTop();
  },
);

async function scrollTraceToTop() {
  await nextTick();
  if (!traceViewport.value) {
    return;
  }
  traceViewport.value.scrollTop = 0;
}

function countTraceByKinds(kinds: Array<TraceEvent["kind"]>): number {
  return props.trace.filter((item) => kinds.includes(item.kind)).length;
}

function selectTraceFilter(id: TraceFilterId) {
  activeTraceFilter.value = id;
}

function selectTab(id: OpsTabId) {
  activeTab.value = id;
}

function beginResize(event: PointerEvent) {
  resizing.value = true;
  resizeStartX = event.clientX;
  resizeStartWidth = props.width;
  window.addEventListener("pointermove", handleResizeMove);
  window.addEventListener("pointerup", finishResize, { once: true });
}

function handleResizeMove(event: PointerEvent) {
  if (!resizing.value) {
    return;
  }
  emit("updateWidth", clampRailWidth(resizeStartWidth + resizeStartX - event.clientX));
}

function finishResize() {
  resizing.value = false;
  window.removeEventListener("pointermove", handleResizeMove);
}

function clampRailWidth(width: number): number {
  return Math.min(Math.max(width, OPS_RAIL_MIN_WIDTH), OPS_RAIL_MAX_WIDTH);
}

function groupToolsByCategory(tools: Array<ToolCapability>): Array<ToolGroup> {
  const groups = new Map<string, Array<ToolCapability>>();

  for (const tool of tools) {
    const category = tool.category || "工具";
    const groupTools = groups.get(category);
    if (groupTools) {
      groupTools.push(tool);
      continue;
    }
    groups.set(category, [tool]);
  }

  return Array.from(groups.entries()).map(([category, groupTools]) => ({
    category,
    count: groupTools.length,
    risks: summarizeToolRisks(groupTools),
    tools: groupTools,
  }));
}

function summarizeToolRisks(tools: Array<ToolCapability>): Array<ToolRiskSummary> {
  const counts = new Map<string, number>();
  for (const tool of tools) {
    const risk = tool.risk || "按需";
    counts.set(risk, (counts.get(risk) ?? 0) + 1);
  }
  return Array.from(counts.entries()).map(([label, count]) => ({ label, count }));
}

function toolGroupTestId(category: string): string {
  return `tool-group-${category}`;
}

function toolRiskTestId(risk: string): string {
  return `tool-risk-${risk}`;
}

function toolRiskBadgeClass(risk: string): string {
  return `${TOOL_RISK_BADGE_BASE} ${TOOL_RISK_CLASS_BY_LABEL[risk] ?? DEFAULT_TOOL_RISK_CLASS}`;
}

function toolParameterTypeLabel(parameter: ToolParameter): string {
  return TOOL_PARAMETER_TYPE_LABEL_BY_NAME[parameter.typeName] ?? parameter.typeName;
}

function traceItemClass(item: TraceEvent): string {
  const base = "rounded-lg border border-[rgba(39,48,40,0.12)] border-l-[3px] bg-[rgba(255,252,244,0.76)] p-2.5";
  const kindClass = {
    state: "border-l-[#c4b9a6]",
    think: "border-l-[#c4b9a6]",
    token: "border-l-[#c4b9a6]",
    tool_call: "border-l-[#2f765e]",
    tool_confirm: "border-l-[#c58a1a]",
    tool_result: "border-l-[#5d6d7d]",
    error: "border-l-[#b04435]",
  } satisfies Record<TraceEvent["kind"], string>;

  return `${base} ${kindClass[item.kind]}`;
}

function traceKindBadgeClass(kind: TraceEvent["kind"]): string {
  return `${TRACE_KIND_BADGE_BASE} ${TRACE_KIND_BADGE_CLASS_BY_VALUE[kind]}`;
}

function traceKindLabel(kind: TraceEvent["kind"]): string {
  return TRACE_KIND_LABEL_BY_VALUE[kind];
}

function mergeToolConfirmEvents(trace: Array<TraceEvent>): Array<TraceEvent> {
  const merged = new Map<string, TraceEvent>();
  for (const event of trace) {
    if (event.kind !== "tool_confirm") {
      continue;
    }

    const key = toolConfirmMergeKey(event);
    const previous = merged.get(key);
    if (!previous || toolConfirmStatusRank(event) > toolConfirmStatusRank(previous)) {
      merged.set(key, event);
    }
  }
  return Array.from(merged.values());
}

function toolConfirmMergeKey(event: TraceEvent): string {
  return toolConfirmDetail(event) || event.detail.trim() || `${event.title}-${event.id}`;
}

function toolConfirmStatusRank(event: TraceEvent): number {
  const rank: Record<ToolConfirmStatusId, number> = {
    record: 0,
    pending: 1,
    timeout: 2,
    denied: 3,
    approved: 3,
  };
  return rank[toolConfirmStatusId(event)];
}

function toolConfirmStatusId(event: TraceEvent): ToolConfirmStatusId {
  if (event.detail.includes("已超时拒绝") || event.title.includes("超时")) {
    return "timeout";
  }
  if (event.detail.includes("已拒绝")) {
    return "denied";
  }
  if (event.detail.includes("已允许")) {
    return "approved";
  }
  if (event.title.includes("请求")) {
    return "pending";
  }
  return "record";
}

function toolConfirmStatusClass(event: TraceEvent): string {
  return `${TOOL_CONFIRM_STATUS_BADGE_BASE} ${TOOL_CONFIRM_STATUS_CLASS_BY_ID[toolConfirmStatusId(event)]}`;
}

function toolConfirmStatusLabel(event: TraceEvent): string {
  return TOOL_CONFIRM_STATUS_LABEL_BY_ID[toolConfirmStatusId(event)];
}

function toolConfirmStatusTestId(event: TraceEvent): string {
  return `tool-confirm-status-${toolConfirmStatusId(event)}`;
}

function toolConfirmDetail(event: TraceEvent): string {
  let detail = event.detail.trim();
  for (const waitingDetail of TOOL_CONFIRM_WAITING_DETAILS) {
    detail = detail.replaceAll(waitingDetail, " ");
  }
  return detail.replace(/\s+/g, " ").trim();
}

function traceDetail(item: TraceEvent): string {
  const detail = displayTraceDetail(item);
  if (isTraceExpanded(item.id) || !isLongTraceDetail(item)) {
    return detail;
  }
  return `${detail.slice(0, TRACE_DETAIL_PREVIEW_LENGTH).trimEnd()}...`;
}

function displayTraceDetail(item: TraceEvent): string {
  const detail = item.detail.trim();
  if (item.kind === "state" && TRACE_LOW_VALUE_DETAILS.has(detail)) {
    return "";
  }
  return detail;
}

function isLongTraceDetail(item: TraceEvent): boolean {
  const detail = displayTraceDetail(item);
  return detail.length > TRACE_DETAIL_PREVIEW_LENGTH || detail.includes("\n");
}

function hasTraceDetail(item: TraceEvent): boolean {
  return displayTraceDetail(item).length > 0;
}

function hasPlainTraceDetail(item: TraceEvent): boolean {
  return hasTraceDetail(item) && structuredTraceFields(item).length === 0;
}

function structuredTraceFields(item: TraceEvent): Array<TraceDetailField> {
  if (item.kind !== "tool_call") {
    return [];
  }

  const payload = parseTraceJsonObject(displayTraceDetail(item));
  if (!payload) {
    return [];
  }

  const tool = props.tools.find((candidate) => candidate.name === item.title);
  const parameters = tool?.parameters ?? [];
  const knownFields = parameters
    .filter((parameter) => Object.hasOwn(payload, parameter.name))
    .map((parameter) => traceDetailFieldFromParameter(parameter, payload[parameter.name]));
  const knownNames = new Set(parameters.map((parameter) => parameter.name));
  const extraFields = Object.entries(payload)
    .filter(([key]) => !knownNames.has(key))
    .map(([key, value]) => traceDetailField(key, fallbackTraceFieldLabel(key), value));

  return [...knownFields, ...extraFields].filter((field) => field.value.length > 0);
}

function traceDetailFieldFromParameter(
  parameter: ToolParameter,
  value: unknown,
): TraceDetailField {
  return traceDetailField(parameter.name, parameter.label, value);
}

function traceDetailField(key: string, label: string, value: unknown): TraceDetailField {
  return {
    key,
    label,
    value: traceFieldValue(value),
  };
}

function fallbackTraceFieldLabel(key: string): string {
  return key
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .replaceAll("_", " ")
    .replace(/\s+/g, " ")
    .trim();
}

function parseTraceJsonObject(value: string): Record<string, unknown> | null {
  if (!value.startsWith("{")) {
    return null;
  }

  try {
    const parsed = JSON.parse(value) as unknown;
    if (isPlainTraceObject(parsed)) {
      return parsed;
    }
  } catch {
    return null;
  }
  return null;
}

function isPlainTraceObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function traceFieldValue(value: unknown): string {
  if (value === null || value === undefined) {
    return "";
  }
  if (typeof value === "string") {
    return value.trim();
  }
  if (typeof value === "number" || typeof value === "boolean") {
    return String(value);
  }
  return JSON.stringify(value) ?? String(value);
}

function isTraceExpanded(id: number): boolean {
  return expandedTraceIds.value.has(id);
}

function toggleTraceItem(id: number) {
  const nextIds = new Set(expandedTraceIds.value);
  if (nextIds.has(id)) {
    nextIds.delete(id);
  } else {
    nextIds.add(id);
  }
  expandedTraceIds.value = nextIds;
}

function copyTraceDetail(item: TraceEvent) {
  if (!navigator.clipboard) {
    return;
  }
  void navigator.clipboard.writeText(`${traceKindLabel(item.kind)} ${item.title}\n${item.detail}`);
}

function traceTimeLabel(item: TraceEvent): string {
  return formatTraceTimeLabel(item.occurredAt);
}

function findingSeverityClass(severity: FindingPreview["severity"]): string {
  return `${FINDING_BADGE_BASE} ${FINDING_SEVERITY_CLASS_BY_VALUE[severity]}`;
}

function findingStatusClass(status: FindingPreview["status"]): string {
  return `${FINDING_BADGE_BASE} ${FINDING_STATUS_CLASS_BY_VALUE[status]}`;
}
</script>

<template>
  <aside
    class="relative flex min-h-0 flex-col border-l border-[rgba(39,48,40,0.18)] bg-[rgba(247,244,236,0.94)]"
    data-testid="ops-rail"
  >
    <button
      type="button"
      class="absolute top-0 bottom-0 left-0 z-10 w-2 cursor-col-resize border-0 bg-transparent p-0 hover:bg-[rgba(47,118,94,0.16)]"
      aria-label="调整右侧栏宽度"
      data-testid="ops-resizer"
      @pointerdown="beginResize"
    />

    <nav class="grid grid-cols-4 gap-1.5 border-b border-[rgba(39,48,40,0.14)] p-[14px] pb-3">
      <button
        v-for="tab in tabItems"
        :key="tab.id"
        type="button"
        class="grid min-w-0 justify-items-center rounded-lg border px-1.5 py-2 text-[11px] font-black"
        :class="
          tab.id === activeTab
            ? 'border-[#2f765e] bg-[#e3efe6] text-[#255744]'
            : 'border-[rgba(39,48,40,0.13)] bg-[rgba(255,252,244,0.72)] text-[#4c584d] hover:border-[#8da795]'
        "
        :aria-pressed="tab.id === activeTab"
        :data-testid="`ops-tab-${tab.id}`"
        @click="selectTab(tab.id)"
      >
        <component :is="tab.icon" :size="15" />
        <span class="mt-1">{{ tab.label }}</span>
        <span class="mt-0.5 font-black tabular-nums">{{ tab.count }}</span>
      </button>
    </nav>

    <section v-if="activeTab === 'trace'" class="flex min-h-0 flex-1 flex-col p-[18px] pt-4">
      <div class="mb-2.5 flex items-center justify-between gap-2">
        <div class="flex min-w-0 items-center gap-2 text-[13px] font-black text-[#334235]">
          <History :size="17" />
          <span>运行轨迹</span>
        </div>
        <span
          class="inline-flex items-center gap-1 rounded-full border border-[rgba(39,48,40,0.13)] bg-[rgba(255,252,244,0.72)] px-2 py-1 text-[11px] font-black text-[#4c584d]"
          data-testid="trace-filter-summary"
        >
          <ListFilter :size="12" />
          {{ filteredTraceSummaryLabel }}
        </span>
      </div>
      <div
        class="mb-2.5 grid grid-cols-3 gap-1.5 rounded-lg border border-[rgba(39,48,40,0.12)] bg-[rgba(255,252,244,0.66)] p-2 text-center"
        data-testid="trace-summary"
      >
        <span class="grid gap-0.5 text-[11px] font-bold text-[#667166]">
          <strong class="text-sm font-black text-[#20291f] tabular-nums">{{ traceSummary.total }}</strong>
          全部
        </span>
        <span class="grid gap-0.5 text-[11px] font-bold text-[#667166]">
          <strong class="text-sm font-black text-[#2f765e] tabular-nums">{{ traceSummary.tool }}</strong>
          工具
        </span>
        <span class="grid gap-0.5 text-[11px] font-bold text-[#667166]">
          <strong class="text-sm font-black text-[#9b2d25] tabular-nums">{{ traceSummary.error }}</strong>
          错误
        </span>
      </div>
      <div class="mb-2.5 grid grid-cols-4 gap-1.5" data-testid="trace-filters">
        <button
          v-for="filter in traceFilters"
          :key="filter.id"
          type="button"
          class="grid min-w-0 justify-items-center rounded-lg border px-1.5 py-1.5 text-[11px] font-black"
          :class="
            filter.id === activeTraceFilter
              ? 'border-[#2f765e] bg-[#e3efe6] text-[#255744]'
              : 'border-[rgba(39,48,40,0.13)] bg-[rgba(255,252,244,0.72)] text-[#4c584d] hover:border-[#8da795]'
          "
          :aria-pressed="filter.id === activeTraceFilter"
          :data-testid="`trace-filter-${filter.id}`"
          @click="selectTraceFilter(filter.id)"
        >
          <span>{{ filter.label }}</span>
          <span class="mt-0.5 font-black tabular-nums">{{ filter.count }}</span>
        </button>
      </div>
      <div
        ref="traceViewport"
        class="flex min-h-0 flex-1 flex-col gap-2.5 overflow-auto"
        data-testid="trace-list"
      >
        <article
          v-for="item in filteredTrace"
          :key="item.id"
          :class="traceItemClass(item)"
          data-testid="trace-item"
        >
          <div class="flex items-start gap-2.5">
            <div class="grid shrink-0 justify-items-start gap-1">
              <span :class="traceKindBadgeClass(item.kind)" data-testid="trace-kind">
                {{ traceKindLabel(item.kind) }}
              </span>
              <time class="pl-0.5 text-[11px] font-bold text-[#667166]" :title="item.occurredAt">
                {{ traceTimeLabel(item) }}
              </time>
            </div>
            <strong
              class="min-w-0 flex-1 pt-0.5 text-[13px] font-black leading-[1.42] text-[#20291f] [overflow-wrap:anywhere]"
            >
              {{ item.title }}
            </strong>
          </div>
          <p
            v-if="hasPlainTraceDetail(item)"
            class="mt-2 whitespace-pre-wrap rounded-md border border-[rgba(39,48,40,0.08)] bg-[rgba(255,253,247,0.64)] px-2.5 py-2 text-xs leading-[1.55] text-[#4c584d] [overflow-wrap:anywhere]"
            :class="isTraceExpanded(item.id) ? 'max-h-72 overflow-auto' : 'max-h-28 overflow-hidden'"
            data-testid="trace-detail"
          >
            {{ traceDetail(item) }}
          </p>
          <div
            v-else-if="structuredTraceFields(item).length > 0"
            class="mt-2 grid gap-1.5 rounded-md border border-[rgba(39,48,40,0.08)] bg-[rgba(255,253,247,0.64)] p-2"
            data-testid="trace-detail-fields"
          >
            <div
              v-for="field in structuredTraceFields(item)"
              :key="`${item.id}-${field.key}`"
              class="grid grid-cols-[54px_minmax(0,1fr)] gap-2 text-xs leading-[1.45]"
              data-testid="trace-detail-field"
            >
              <span class="font-black text-[#667166]">{{ field.label }}</span>
              <span class="min-w-0 text-[#26382d] [overflow-wrap:anywhere]">
                {{ field.value }}
              </span>
            </div>
          </div>
          <div
            v-if="hasPlainTraceDetail(item) && isLongTraceDetail(item)"
            class="mt-2 flex items-center justify-between gap-2"
            data-testid="trace-actions"
          >
            <button
              type="button"
              class="inline-flex items-center gap-1 rounded-md border border-[rgba(39,48,40,0.12)] bg-[rgba(255,252,244,0.7)] px-2 py-1 text-[11px] font-black text-[#4c584d]"
              data-testid="trace-toggle"
              @click="toggleTraceItem(item.id)"
            >
              <ChevronDown v-if="isTraceExpanded(item.id)" :size="13" />
              <ChevronRight v-else :size="13" />
              <span>{{ isTraceExpanded(item.id) ? "收起详情" : "展开详情" }}</span>
            </button>
            <button
              type="button"
              class="inline-flex items-center gap-1 rounded-md border border-[rgba(39,48,40,0.12)] bg-[rgba(255,252,244,0.7)] px-2 py-1 text-[11px] font-black text-[#4c584d]"
              title="复制轨迹详情"
              data-testid="trace-copy"
              @click="copyTraceDetail(item)"
            >
              <ClipboardCopy :size="13" />
              <span>复制</span>
            </button>
          </div>
        </article>
        <p v-if="trace.length === 0" class="text-xs text-[#667166]">暂无 Agent 事件</p>
        <p v-else-if="filteredTrace.length === 0" class="text-xs text-[#667166]">
          {{ activeTraceFilterLabel }} 暂无事件
        </p>
      </div>
    </section>

    <section v-else-if="activeTab === 'confirm'" class="flex min-h-0 flex-1 flex-col p-[18px] pt-4">
      <div class="mb-2.5 flex items-center justify-between gap-2">
        <div class="flex min-w-0 items-center gap-2 text-[13px] font-black text-[#62440f]">
          <ShieldAlert :size="17" />
          <span>确认历史</span>
        </div>
        <span
          class="rounded-full border border-[#e1c27c] bg-[#fff1d5] px-2 py-1 text-[11px] font-black text-[#805100]"
          data-testid="tool-confirm-count"
        >
          {{ toolConfirmEvents.length }} 条
        </span>
      </div>
      <div class="grid min-h-0 gap-2 overflow-auto pr-1" data-testid="tool-confirm-list">
        <article
          v-for="event in toolConfirmEvents"
          :key="event.id"
          class="rounded-lg border border-[#e1c27c] bg-[#fff8e6] p-2.5"
          data-testid="tool-confirm-item"
        >
          <div class="flex items-start gap-1.5">
            <strong class="min-w-0 flex-1 text-xs font-black text-[#3b2b10]">
              {{ event.title }}
            </strong>
            <span :class="toolConfirmStatusClass(event)" :data-testid="toolConfirmStatusTestId(event)">
              {{ toolConfirmStatusLabel(event) }}
            </span>
            <time class="text-[11px] text-[#805100]">{{ event.occurredAt }}</time>
          </div>
          <p
            v-if="toolConfirmDetail(event).length > 0"
            class="mt-2 max-h-32 overflow-auto whitespace-pre-wrap text-xs leading-[1.45] text-[#62440f] [overflow-wrap:anywhere]"
            data-testid="tool-confirm-detail"
          >
            {{ toolConfirmDetail(event) }}
          </p>
        </article>
        <p v-if="toolConfirmEvents.length === 0" class="text-xs text-[#667166]">暂无确认记录</p>
      </div>
    </section>

    <section v-else-if="activeTab === 'tools'" class="flex min-h-0 flex-1 flex-col p-[18px] pt-4">
      <div class="mb-2.5 flex items-center justify-between gap-2">
        <div class="flex min-w-0 items-center gap-2 text-[13px] font-black text-[#334235]">
          <Wrench :size="17" />
          <span>工具能力</span>
        </div>
        <span
          class="rounded-full border border-[rgba(39,48,40,0.13)] bg-[rgba(255,252,244,0.7)] px-2 py-1 text-[11px] font-black text-[#4c584d]"
          data-testid="tool-count"
        >
          {{ tools.length }} 项
        </span>
      </div>
      <div class="grid min-h-0 gap-2 overflow-auto pr-1" data-testid="tool-groups">
        <article
          v-for="group in toolGroups"
          :key="group.category"
          class="rounded-lg border border-[rgba(39,48,40,0.13)] bg-[rgba(255,252,244,0.72)] p-2.5"
          :data-testid="toolGroupTestId(group.category)"
        >
          <div class="flex items-start justify-between gap-2">
            <div class="min-w-0">
              <strong class="block text-xs font-black text-[#20291f]">{{ group.category }}</strong>
              <span class="mt-0.5 block text-[11px] font-bold text-[#667166]">
                {{ group.count }} 个能力
              </span>
            </div>
            <div class="flex flex-wrap justify-end gap-1">
              <span
                v-for="risk in group.risks"
                :key="risk.label"
                :class="toolRiskBadgeClass(risk.label)"
                :data-testid="toolRiskTestId(risk.label)"
              >
                {{ risk.label }} {{ risk.count }}
              </span>
            </div>
          </div>
          <div class="mt-2 grid gap-2">
            <div
              v-for="tool in group.tools"
              :key="tool.name"
              class="border-t border-[rgba(39,48,40,0.11)] pt-2 first:border-t-0 first:pt-0"
              data-testid="tool-item"
            >
              <strong class="block text-xs font-black text-[#20291f]">{{ tool.name }}</strong>
              <p class="mt-1 text-xs leading-[1.45] text-[#4c584d] [overflow-wrap:anywhere]">
                {{ tool.description }}
              </p>
              <div
                v-if="tool.parameters.length > 0"
                class="mt-2 grid gap-1.5"
                data-testid="tool-parameters"
              >
                <div
                  v-for="parameter in tool.parameters"
                  :key="`${tool.name}-${parameter.name}`"
                  class="grid grid-cols-[minmax(0,1fr)_auto] items-start gap-2 rounded-md border border-[rgba(39,48,40,0.08)] bg-[rgba(255,253,247,0.56)] px-2 py-1.5"
                  data-testid="tool-parameter"
                >
                  <div class="min-w-0">
                    <span class="text-[11px] font-black text-[#26382d]">
                      {{ parameter.label }}
                    </span>
                    <span
                      v-if="parameter.required"
                      class="ml-1 inline-flex rounded-full border border-[#e1c27c] bg-[#fff8e6] px-1.5 py-0.5 text-[10px] font-black text-[#805100]"
                    >
                      必填
                    </span>
                    <p
                      v-if="parameter.description.length > 0"
                      class="mt-0.5 text-[11px] leading-[1.35] text-[#667166] [overflow-wrap:anywhere]"
                    >
                      {{ parameter.description }}
                    </p>
                  </div>
                  <span
                    class="rounded-full border border-[rgba(39,48,40,0.1)] bg-[rgba(233,226,212,0.5)] px-1.5 py-0.5 text-[10px] font-black text-[#667166]"
                  >
                    {{ toolParameterTypeLabel(parameter) }}
                  </span>
                </div>
              </div>
            </div>
          </div>
        </article>
        <p v-if="tools.length === 0" class="text-xs text-[#667166]">暂无可用工具</p>
      </div>
    </section>

    <section v-else class="flex min-h-0 flex-1 flex-col p-[18px] pt-4">
      <div class="mb-2.5 flex items-center justify-between gap-2">
        <div class="flex min-w-0 items-center gap-2 text-[13px] font-black text-[#334235]">
          <FileSearch :size="17" />
          <span>候选发现</span>
        </div>
        <span
          class="rounded-full border border-[rgba(39,48,40,0.13)] bg-[rgba(255,252,244,0.7)] px-2 py-1 text-[11px] font-black text-[#4c584d]"
          data-testid="finding-count"
        >
          {{ findings.length }} 项
        </span>
      </div>
      <div class="grid min-h-0 gap-2 overflow-auto pr-1" data-testid="finding-list">
        <article
          v-for="finding in findings"
          :key="finding.id"
          class="rounded-lg border border-[rgba(39,48,40,0.13)] bg-[rgba(255,252,244,0.72)] p-2.5"
          data-testid="finding-item"
        >
          <div class="mb-2 flex flex-wrap gap-1.5">
            <span :class="findingStatusClass(finding.status)" data-testid="finding-status">
              {{ finding.statusLabel }}
            </span>
            <span :class="findingSeverityClass(finding.severity)" data-testid="finding-severity">
              {{ finding.severityLabel }}
            </span>
            <span
              class="inline-flex items-center rounded-full border border-[#b8c8d5] bg-[#e4ecf2] px-2 py-1 text-[11px] font-black text-[#365d78]"
              data-testid="finding-confidence"
            >
              {{ finding.confidenceLabel }}
            </span>
          </div>
          <strong class="block text-xs font-black text-[#20291f]" data-testid="finding-title">
            {{ finding.title }}
          </strong>
          <p class="mt-1.5 text-xs leading-[1.45] text-[#4c584d] [overflow-wrap:anywhere]">
            {{ finding.location }}
          </p>
          <p class="mt-2 text-xs leading-[1.45] text-[#26382d] [overflow-wrap:anywhere]">
            {{ finding.summary }}
          </p>

          <div
            class="mt-2 grid grid-cols-2 gap-1.5 text-[11px] font-bold text-[#4c584d]"
            data-testid="finding-meta"
          >
            <span class="rounded-md bg-[rgba(233,226,212,0.62)] px-2 py-1">
              位置 {{ finding.location }}
            </span>
            <span class="rounded-md bg-[rgba(233,226,212,0.62)] px-2 py-1">
              类型 {{ finding.taxonomy ?? "待归因" }}
            </span>
          </div>

          <div class="mt-2 grid gap-1.5" data-testid="finding-evidence-list">
            <div
              v-for="evidence in finding.evidence"
              :key="`${finding.id}-${evidence.label}-${evidence.source}`"
              class="rounded-md border border-[rgba(39,48,40,0.1)] bg-[rgba(255,253,247,0.72)] p-2"
              data-testid="finding-evidence-item"
            >
              <div class="flex items-center justify-between gap-2">
                <strong class="text-[11px] font-black text-[#20291f]">
                  {{ evidence.label }}
                </strong>
                <span class="text-[11px] font-bold text-[#667166]">{{ evidence.source }}</span>
              </div>
              <p class="mt-1 text-xs leading-[1.4] text-[#4c584d] [overflow-wrap:anywhere]">
                {{ evidence.detail }}
              </p>
            </div>
          </div>

          <div
            class="mt-2 rounded-md border border-[#e1c27c] bg-[#fff8e6] p-2 text-xs leading-[1.45] text-[#62440f] [overflow-wrap:anywhere]"
            data-testid="finding-next-action"
          >
            {{ finding.nextAction }}
          </div>
        </article>
        <p v-if="findings.length === 0" class="text-xs text-[#667166]">暂无候选发现</p>
      </div>
    </section>
  </aside>
</template>
