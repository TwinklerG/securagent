import { spawn } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import { createServer } from "node:net";
import { basename, join } from "node:path";
import process from "node:process";

const DEV_SERVER_PORT = 1420;
const APP_URL = `http://127.0.0.1:${DEV_SERVER_PORT}`;
const TEST_RESULTS_DIR = join(process.cwd(), "test-results");
const EDGE_PATHS = [
  process.env.SECAUDIT_GUI_BROWSER,
  "C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe",
  "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
  "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
  "C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe",
].filter(Boolean);
const REQUIRED_TEXT = [
  "SecAudit Agent",
  "代码安全审计工作台",
  "工作区",
  "会话历史",
  "轨迹",
  "确认",
  "工具",
  "发现",
];
const VIEWPORT = {
  width: 1280,
  height: 820,
};
const WAIT_TIMEOUT_MS = 20_000;
const POLL_INTERVAL_MS = 250;
const CDP_REQUEST_TIMEOUT_MS = 5_000;
const PROCESS_COMMAND_TIMEOUT_MS = 10_000;
const FINAL_CLEANUP_TIMEOUT_MS = 15_000;
const BROWSER_FLAGS = [
  "--headless=new",
  "--disable-gpu",
  "--no-sandbox",
  "--disable-dev-shm-usage",
  "--disable-extensions",
  "--disable-background-networking",
  "--remote-allow-origins=*",
  "--no-first-run",
  "--no-default-browser-check",
];
const SELECTORS = {
  appShell: '[data-testid="app-shell"]',
  assistantMarkdown: '[data-testid="assistant-markdown"]',
  composerInput: '[data-testid="composer-input"]',
  findingConfidence: '[data-testid="finding-confidence"]',
  findingCount: '[data-testid="finding-count"]',
  findingEvidenceItem: '[data-testid="finding-evidence-item"]',
  findingEvidenceList: '[data-testid="finding-evidence-list"]',
  findingItem: '[data-testid="finding-item"]',
  findingList: '[data-testid="finding-list"]',
  findingNextAction: '[data-testid="finding-next-action"]',
  findingSeverity: '[data-testid="finding-severity"]',
  findingStatus: '[data-testid="finding-status"]',
  findingTitle: '[data-testid="finding-title"]',
  opsResizer: '[data-testid="ops-resizer"]',
  opsTabConfirm: '[data-testid="ops-tab-confirm"]',
  opsTabFindings: '[data-testid="ops-tab-findings"]',
  opsTabTools: '[data-testid="ops-tab-tools"]',
  opsTabTrace: '[data-testid="ops-tab-trace"]',
  sendButton: '[data-testid="send-button"]',
  sessionFilterActive: '[data-testid="session-filter-active"]',
  sessionFilterAll: '[data-testid="session-filter-all"]',
  sessionFilterArchived: '[data-testid="session-filter-archived"]',
  sessionArchiveButton: '[data-testid="session-archive-button"]',
  sessionItem: '[data-testid="session-item"]',
  sessionList: '[data-testid="session-list"]',
  toolConfirmCount: '[data-testid="tool-confirm-count"]',
  toolConfirmItem: '[data-testid="tool-confirm-item"]',
  toolConfirmList: '[data-testid="tool-confirm-list"]',
  toolCount: '[data-testid="tool-count"]',
  toolGroupFile: '[data-testid="tool-group-文件"]',
  toolGroups: '[data-testid="tool-groups"]',
  toolItem: '[data-testid="tool-item"]',
  toolParameter: '[data-testid="tool-parameter"]',
  toolParameters: '[data-testid="tool-parameters"]',
  toolRiskConfirm: '[data-testid="tool-risk-需确认"]',
  toolRiskNetwork: '[data-testid="tool-risk-网络"]',
  toolRiskReadonly: '[data-testid="tool-risk-只读"]',
  traceFilterAll: '[data-testid="trace-filter-all"]',
  traceFilterTool: '[data-testid="trace-filter-tool"]',
  traceDetailField: '[data-testid="trace-detail-field"]',
  traceDetailFields: '[data-testid="trace-detail-fields"]',
  traceKind: '[data-testid="trace-kind"]',
  traceItem: '[data-testid="trace-item"]',
  traceList: '[data-testid="trace-list"]',
  traceSummary: '[data-testid="trace-summary"]',
  workDirInput: '[data-testid="work-dir-input"]',
  workDirButton: '[data-testid="work-dir-button"]',
  workDirPickerButton: '[data-testid="work-dir-picker-button"]',
};

const now = new Date();
const stamp = now.toISOString().replaceAll(":", "-").replaceAll(".", "-");
const resultPath = join(TEST_RESULTS_DIR, `gui-smoke-${stamp}.json`);
const screenshotPath = join(TEST_RESULTS_DIR, `gui-smoke-${stamp}.png`);
const browserProfileDir = join(TEST_RESULTS_DIR, `.browser-profile-smoke-${stamp}`);

const artifacts = {
  appUrl: APP_URL,
  resultPath,
  screenshotPath,
  startedDevServer: false,
  browser: null,
  checks: [],
};
const diagnostics = [];

let devServerProcess = null;
let browserProcess = null;
let cdp = null;
let initialDevServerPids = new Set();
let ownedDevServerPids = new Set();

async function ensureDevServer() {
  if (await canFetch(APP_URL)) {
    return;
  }

  initialDevServerPids = await findListeningPids(DEV_SERVER_PORT);
  devServerProcess = spawn("bun", ["run", "dev"], {
    cwd: process.cwd(),
    shell: false,
    stdio: "ignore",
    windowsHide: true,
  });
  devServerProcess.unref();
  artifacts.startedDevServer = true;

  await waitUntil(async () => canFetch(APP_URL), "Vite dev server 未在 1420 端口就绪");
  ownedDevServerPids = await findNewListeningPids(DEV_SERVER_PORT, initialDevServerPids);
  debug("dev-server", {
    launcherPid: devServerProcess.pid,
    listenerPids: Array.from(ownedDevServerPids),
  });
}

async function findBrowserPath() {
  const { access } = await import("node:fs/promises");
  for (const browserPath of EDGE_PATHS) {
    try {
      await access(browserPath);
      return browserPath;
    } catch {
      // 继续尝试下一个常见安装路径。
    }
  }

  throw new Error("未找到 Edge/Chrome。可通过 SECAUDIT_GUI_BROWSER 指定浏览器路径。");
}

function spawnBrowser(browserPath, port) {
  const child = spawn(
    browserPath,
    [
      ...BROWSER_FLAGS,
      `--remote-debugging-port=${port}`,
      `--user-data-dir=${browserProfileDir}`,
      "about:blank",
    ],
    {
      stdio: "ignore",
      windowsHide: true,
    },
  );
  child.unref();
  return child;
}

async function waitForBrowserDebugger(port) {
  const endpoint = `http://127.0.0.1:${port}/json/version`;
  await waitUntil(async () => canFetch(endpoint), "浏览器 DevTools 端口未就绪");
  const response = await fetch(endpoint);
  const version = await response.json();
  if (!version.webSocketDebuggerUrl) {
    throw new Error("DevTools 端点未返回浏览器级 WebSocket URL");
  }
  return version.webSocketDebuggerUrl;
}

async function waitForPageDebugger(port, targetId) {
  await waitUntil(async () => {
    const target = await findPageTarget(port, targetId);
    return Boolean(target?.webSocketDebuggerUrl);
  }, "页面 DevTools target 未就绪");

  const target = await findPageTarget(port, targetId);
  return target.webSocketDebuggerUrl;
}

async function findPageTarget(port, targetId) {
  const response = await fetch(`http://127.0.0.1:${port}/json/list`);
  const targets = await response.json();
  return targets.find((target) => target.id === targetId && target.type === "page") ?? null;
}

async function checkTextContent() {
  const text = await evaluate("document.body.innerText", "页面文本");
  const missing = REQUIRED_TEXT.filter((item) => !text.includes(item));
  addCheck("核心区域文本", missing.length === 0, {
    missing,
  });
}

async function checkOpsRailTabs() {
  const tabs = await evaluate(
    `(() => ({
      traceActive: document.querySelector('${SELECTORS.opsTabTrace}')?.getAttribute("aria-pressed") === "true",
      confirmVisible: Boolean(document.querySelector('${SELECTORS.opsTabConfirm}')),
      toolsVisible: Boolean(document.querySelector('${SELECTORS.opsTabTools}')),
      findingsVisible: Boolean(document.querySelector('${SELECTORS.opsTabFindings}')),
      resizerVisible: Boolean(document.querySelector('${SELECTORS.opsResizer}')),
    }))()`,
    "右侧栏 tabs 状态",
  );
  addCheck(
    "右侧栏使用 tabs 并可调整宽度",
    tabs.traceActive &&
      tabs.confirmVisible &&
      tabs.toolsVisible &&
      tabs.findingsVisible &&
      tabs.resizerVisible,
    tabs,
  );
}

async function checkNoHorizontalOverflow() {
  const overflow = await evaluate(
    `(() => {
      const root = document.documentElement;
      const body = document.body;
      const rootOverflow = Math.max(root.scrollWidth - root.clientWidth, 0);
      const bodyOverflow = Math.max(body.scrollWidth - window.innerWidth, 0);
      const offenders = Array.from(document.querySelectorAll("body *"))
        .map((node) => {
          const rect = node.getBoundingClientRect();
          return {
            tag: node.tagName.toLowerCase(),
            className: String(node.className || ""),
            left: Math.round(rect.left),
            right: Math.round(rect.right),
          };
        })
        .filter((item) => item.left < -1 || item.right > window.innerWidth + 1)
        .slice(0, 8);
      return { rootOverflow, bodyOverflow, offenders };
    })()`,
    "横向溢出检查",
  );
  addCheck(
    "桌面视口无横向溢出",
    overflow.rootOverflow === 0 && overflow.bodyOverflow === 0,
    overflow,
  );
}

async function checkToolCapabilityPanel() {
  await selectOpsTab(SELECTORS.opsTabTools, SELECTORS.toolGroups, "切换工具 tab");
  const panel = await evaluate(
    `(() => {
      const groups = document.querySelector('${SELECTORS.toolGroups}');
      const text = groups?.innerText ?? "";
      return {
        groupCount: document.querySelectorAll('[data-testid^="tool-group-"]').length,
        itemCount: document.querySelectorAll('${SELECTORS.toolItem}').length,
        countLabel: document.querySelector('${SELECTORS.toolCount}')?.innerText ?? "",
        fileGroupVisible: Boolean(document.querySelector('${SELECTORS.toolGroupFile}')),
        readonlyRiskVisible: Boolean(document.querySelector('${SELECTORS.toolRiskReadonly}')),
        networkRiskVisible: Boolean(document.querySelector('${SELECTORS.toolRiskNetwork}')),
        confirmRiskVisible: Boolean(document.querySelector('${SELECTORS.toolRiskConfirm}')),
        fileToolVisible: text.includes("read_file"),
        networkToolVisible: text.includes("nvd_lookup"),
        confirmToolVisible: text.includes("execute_command"),
        parameterGroupCount: document.querySelectorAll('${SELECTORS.toolParameters}').length,
        parameterCount: document.querySelectorAll('${SELECTORS.toolParameter}').length,
        projectPathParameterVisible: text.includes("项目路径"),
        rulesetParameterVisible: text.includes("规则集"),
        requiredParameterVisible: text.includes("必填"),
      };
    })()`,
    "工具能力面板状态",
  );
  addCheck(
    "工具能力按类别和风险展示",
    panel.groupCount >= 4 &&
      panel.itemCount >= 5 &&
      panel.countLabel.includes("5") &&
      panel.fileGroupVisible &&
      panel.readonlyRiskVisible &&
      panel.networkRiskVisible &&
      panel.confirmRiskVisible &&
      panel.fileToolVisible &&
      panel.networkToolVisible &&
      panel.confirmToolVisible &&
      panel.parameterGroupCount >= 5 &&
      panel.parameterCount >= 10 &&
      panel.projectPathParameterVisible &&
      panel.rulesetParameterVisible &&
      panel.requiredParameterVisible,
    panel,
  );
}

async function checkFindingPanel() {
  await selectOpsTab(SELECTORS.opsTabFindings, SELECTORS.findingList, "切换发现 tab");
  const panel = await evaluate(
    `(() => {
      const list = document.querySelector('${SELECTORS.findingList}');
      const text = list?.innerText ?? "";
      return {
        countLabel: document.querySelector('${SELECTORS.findingCount}')?.innerText ?? "",
        itemCount: document.querySelectorAll('${SELECTORS.findingItem}').length,
        evidenceCount: document.querySelectorAll('${SELECTORS.findingEvidenceItem}').length,
        statusLabel: document.querySelector('${SELECTORS.findingStatus}')?.innerText ?? "",
        severityLabel: document.querySelector('${SELECTORS.findingSeverity}')?.innerText ?? "",
        confidenceLabel: document.querySelector('${SELECTORS.findingConfidence}')?.innerText ?? "",
        title: document.querySelector('${SELECTORS.findingTitle}')?.innerText ?? "",
        evidenceVisible: Boolean(document.querySelector('${SELECTORS.findingEvidenceList}')),
        nextAction: document.querySelector('${SELECTORS.findingNextAction}')?.innerText ?? "",
        summaryVisible: text.includes("结构化占位") || text.includes("待验证的发现槽位"),
      };
    })()`,
    "候选发现详情状态",
  );
  addCheck(
    "候选发现展示结构化详情",
    panel.countLabel.includes("1") &&
      panel.itemCount === 1 &&
      panel.evidenceCount >= 2 &&
      panel.statusLabel.includes("候选") &&
      panel.severityLabel.includes("待确认") &&
      panel.confidenceLabel.includes("等待证据") &&
      panel.title.includes("候选发现") &&
      panel.evidenceVisible &&
      panel.nextAction.includes("发送审计请求") &&
      panel.summaryVisible,
    panel,
  );
}

async function checkMarkdownRendering() {
  const markdown = await evaluate(
    `(() => {
      const root = document.querySelector('${SELECTORS.assistantMarkdown}');
      const table = root?.querySelector("table");
      const tableStyle = table ? getComputedStyle(table) : null;
      const code = root?.querySelector("pre code.hljs");
      const tableWidth = table?.getBoundingClientRect().width ?? 0;
      const rootWidth = root?.getBoundingClientRect().width ?? 0;
      return {
        visible: Boolean(root),
        heading: root?.querySelector("h2")?.innerText ?? "",
        listCount: root?.querySelectorAll("li").length ?? 0,
        tableVisible: Boolean(table),
        tableBorderStyle: tableStyle?.borderTopStyle ?? "",
        tableBorderWidth: tableStyle?.borderTopWidth ?? "",
        tableWidth: Math.round(tableWidth),
        rootWidth: Math.round(rootWidth),
        codeHighlighted: Boolean(code?.classList.contains("language-rust")),
        codeText: code?.innerText ?? "",
      };
    })()`,
    "Markdown 渲染状态",
  );
  addCheck(
    "Agent 输出按 Markdown 渲染并支持代码高亮与表格边框",
    markdown.visible &&
      markdown.heading.includes("预览审计计划") &&
      markdown.listCount >= 2 &&
      markdown.tableVisible &&
      markdown.tableBorderStyle !== "none" &&
      markdown.tableBorderWidth !== "0px" &&
      markdown.tableWidth > 0 &&
      markdown.rootWidth > 0 &&
      markdown.tableWidth < markdown.rootWidth * 0.85 &&
      markdown.codeHighlighted &&
      markdown.codeText.includes("audit_target"),
    markdown,
  );
}

async function checkWorkDirControls() {
  const controls = await evaluate(
    `(() => ({
      pickerVisible: Boolean(document.querySelector('${SELECTORS.workDirPickerButton}')),
      pickerDisabled: document.querySelector('${SELECTORS.workDirPickerButton}')?.disabled ?? true,
      staleOptionCount: document.querySelectorAll('[data-testid="work-dir-option"]').length,
      staleOptionsVisible: Boolean(document.querySelector('[data-testid="work-dir-options"]')),
    }))()`,
    "工作区控件状态",
  );
  addCheck(
    "工作区使用目录选择按钮且不再展示候选目录",
    controls.pickerVisible &&
      !controls.pickerDisabled &&
      controls.staleOptionCount === 0 &&
      !controls.staleOptionsVisible,
    controls,
  );
}

async function checkSessionFilters() {
  const initial = await evaluate(
    `(() => ({
      allActive: document.querySelector('${SELECTORS.sessionFilterAll}')?.getAttribute("aria-pressed") === "true",
      allLabel: document.querySelector('${SELECTORS.sessionFilterAll}')?.innerText ?? "",
      activeLabel: document.querySelector('${SELECTORS.sessionFilterActive}')?.innerText ?? "",
      archivedLabel: document.querySelector('${SELECTORS.sessionFilterArchived}')?.innerText ?? "",
      archiveButtonCount: document.querySelectorAll('${SELECTORS.sessionArchiveButton}').length,
      itemCount: document.querySelectorAll('${SELECTORS.sessionItem}').length,
      activeSessionVisible: document.querySelector('${SELECTORS.sessionList}')?.innerText.includes("预览审计会话") ?? false,
      archivableSessionVisible: document.querySelector('${SELECTORS.sessionList}')?.innerText.includes("待归档命令审计") ?? false,
      archivedSessionVisible: document.querySelector('${SELECTORS.sessionList}')?.innerText.includes("历史依赖审计") ?? false,
    }))()`,
    "会话筛选初始状态",
  );
  addCheck(
    "会话侧栏显示状态筛选",
    initial.allActive &&
      initial.allLabel.includes("3") &&
      initial.activeLabel.includes("2") &&
      initial.archivedLabel.includes("1") &&
      initial.archiveButtonCount === 1 &&
      initial.itemCount === 3 &&
      initial.activeSessionVisible &&
      initial.archivableSessionVisible &&
      initial.archivedSessionVisible,
    initial,
  );

  await evaluate(
    `document.querySelector('${SELECTORS.sessionFilterArchived}').click()`,
    "筛选归档会话",
  );
  await waitForExpression(
    `document.querySelector('${SELECTORS.sessionList}')?.innerText.includes("历史依赖审计")`,
  );
  const archived = await evaluate(
    `(() => ({
      archivedActive: document.querySelector('${SELECTORS.sessionFilterArchived}')?.getAttribute("aria-pressed") === "true",
      itemCount: document.querySelectorAll('${SELECTORS.sessionItem}').length,
      archivedSessionVisible: document.querySelector('${SELECTORS.sessionList}')?.innerText.includes("历史依赖审计") ?? false,
      archivableSessionVisible: document.querySelector('${SELECTORS.sessionList}')?.innerText.includes("待归档命令审计") ?? false,
      activeSessionVisible: document.querySelector('${SELECTORS.sessionList}')?.innerText.includes("预览审计会话") ?? false,
    }))()`,
    "归档会话筛选状态",
  );
  addCheck(
    "会话侧栏可筛选归档会话",
    archived.archivedActive &&
      archived.itemCount === 1 &&
      archived.archivedSessionVisible &&
      !archived.archivableSessionVisible &&
      !archived.activeSessionVisible,
    archived,
  );

  await evaluate(
    `document.querySelector('${SELECTORS.sessionFilterActive}').click()`,
    "筛选活跃会话",
  );
  await waitForExpression(
    `document.querySelector('${SELECTORS.sessionList}')?.innerText.includes("预览审计会话")`,
  );
  const active = await evaluate(
    `(() => ({
      activeActive: document.querySelector('${SELECTORS.sessionFilterActive}')?.getAttribute("aria-pressed") === "true",
      archiveButtonCount: document.querySelectorAll('${SELECTORS.sessionArchiveButton}').length,
      itemCount: document.querySelectorAll('${SELECTORS.sessionItem}').length,
      activeSessionVisible: document.querySelector('${SELECTORS.sessionList}')?.innerText.includes("预览审计会话") ?? false,
      archivableSessionVisible: document.querySelector('${SELECTORS.sessionList}')?.innerText.includes("待归档命令审计") ?? false,
      archivedSessionVisible: document.querySelector('${SELECTORS.sessionList}')?.innerText.includes("历史依赖审计") ?? false,
    }))()`,
    "活跃会话筛选状态",
  );
  addCheck(
    "会话侧栏可筛选活跃会话",
    active.activeActive &&
      active.archiveButtonCount === 1 &&
      active.itemCount === 2 &&
      active.activeSessionVisible &&
      active.archivableSessionVisible &&
      !active.archivedSessionVisible,
    active,
  );

  await evaluate(
    `document.querySelector('${SELECTORS.sessionArchiveButton}').click()`,
    "归档非当前活跃会话",
  );
  await waitForExpression(
    `document.querySelector('${SELECTORS.sessionList}')?.innerText.includes("预览审计会话") && !document.querySelector('${SELECTORS.sessionList}')?.innerText.includes("待归档命令审计")`,
  );
  const afterArchiveActive = await evaluate(
    `(() => ({
      activeLabel: document.querySelector('${SELECTORS.sessionFilterActive}')?.innerText ?? "",
      archivedLabel: document.querySelector('${SELECTORS.sessionFilterArchived}')?.innerText ?? "",
      archiveButtonCount: document.querySelectorAll('${SELECTORS.sessionArchiveButton}').length,
      itemCount: document.querySelectorAll('${SELECTORS.sessionItem}').length,
      activeSessionVisible: document.querySelector('${SELECTORS.sessionList}')?.innerText.includes("预览审计会话") ?? false,
      archivableSessionVisible: document.querySelector('${SELECTORS.sessionList}')?.innerText.includes("待归档命令审计") ?? false,
    }))()`,
    "归档后活跃会话状态",
  );
  addCheck(
    "会话侧栏可归档非当前活跃会话",
    afterArchiveActive.activeLabel.includes("1") &&
      afterArchiveActive.archivedLabel.includes("2") &&
      afterArchiveActive.archiveButtonCount === 0 &&
      afterArchiveActive.itemCount === 1 &&
      afterArchiveActive.activeSessionVisible &&
      !afterArchiveActive.archivableSessionVisible,
    afterArchiveActive,
  );

  await evaluate(
    `document.querySelector('${SELECTORS.sessionFilterArchived}').click()`,
    "查看归档后的会话",
  );
  await waitForExpression(
    `document.querySelector('${SELECTORS.sessionList}')?.innerText.includes("待归档命令审计")`,
  );
  const afterArchiveArchived = await evaluate(
    `(() => ({
      itemCount: document.querySelectorAll('${SELECTORS.sessionItem}').length,
      archivedSessionVisible: document.querySelector('${SELECTORS.sessionList}')?.innerText.includes("历史依赖审计") ?? false,
      archivableSessionVisible: document.querySelector('${SELECTORS.sessionList}')?.innerText.includes("待归档命令审计") ?? false,
    }))()`,
    "归档后归档会话状态",
  );
  addCheck(
    "归档后会话进入归档筛选",
    afterArchiveArchived.itemCount === 2 &&
      afterArchiveArchived.archivedSessionVisible &&
      afterArchiveArchived.archivableSessionVisible,
    afterArchiveArchived,
  );

  await evaluate(
    `document.querySelector('${SELECTORS.sessionFilterAll}').click()`,
    "恢复全部会话筛选",
  );
}

async function checkPreviewInteraction() {
  const initial = await evaluate(
    `(() => ({
      textareaDisabled: document.querySelector('${SELECTORS.composerInput}')?.disabled ?? true,
      sendDisabled: document.querySelector('${SELECTORS.sendButton}')?.disabled ?? true,
      configVisible: document.body.innerText.includes("浏览器预览模式"),
    }))()`,
    "初始交互状态",
  );
  addCheck(
    "预览模式显示配置提示且输入框可用",
    initial.configVisible && !initial.textareaDisabled,
    initial,
  );

  await evaluate(
    `(() => {
      const textarea = document.querySelector('${SELECTORS.composerInput}');
      textarea.value = "检查 preview 模式下的审计请求流";
      textarea.dispatchEvent(new Event("input", { bubbles: true }));
    })()`,
    "输入审计请求",
  );
  await waitForExpression(
    `document.querySelector('${SELECTORS.sendButton}') && !document.querySelector('${SELECTORS.sendButton}').disabled`,
  );
  await evaluate(`document.querySelector('${SELECTORS.sendButton}').click()`, "点击发送");
  await waitForExpression("document.body.innerText.includes('检查 preview 模式下的审计请求流')");
  await waitForExpression(
    `document.body.innerText.includes("我会先识别入口、") && document.querySelector('${SELECTORS.sendButton}')?.disabled === true`,
  );
  const duringRun = await evaluate(
    `(() => ({
      liveDraftVisible: document.body.innerText.includes("我会先识别入口、"),
      sendDisabled: document.querySelector('${SELECTORS.sendButton}')?.disabled ?? false,
      workDirDisabled: document.querySelector('${SELECTORS.workDirInput}')?.disabled ?? false,
    }))()`,
    "运行中临时反馈",
  );
  addCheck(
    "预览模式显示运行中 Agent 草稿",
    duringRun.liveDraftVisible && duringRun.sendDisabled && duringRun.workDirDisabled,
    duringRun,
  );
  await checkTraceFiltersDuringRun();
  await waitForExpression(
    `document.body.innerText.includes("不会访问真实 API Key") && document.querySelector('${SELECTORS.sendButton}')?.disabled === true`,
  );
  const afterSend = await evaluate(
    `(() => ({
      userMessageVisible: document.body.innerText.includes("检查 preview 模式下的审计请求流"),
      assistantMessageVisible: document.body.innerText.includes("我会先识别入口"),
      textareaCleared: document.querySelector('${SELECTORS.composerInput}')?.value === "",
    }))()`,
    "发送后状态",
  );
  addCheck(
    "预览模式可输入并发送审计请求",
    afterSend.userMessageVisible && afterSend.assistantMessageVisible,
    afterSend,
  );

  await evaluate(
    `(() => {
      const input = document.querySelector('${SELECTORS.workDirInput}');
      input.value = "D:\\\\Project\\\\securagent\\\\crates";
      input.dispatchEvent(new Event("input", { bubbles: true }));
    })()`,
    "输入工作区",
  );
  await waitForExpression(
    `document.querySelector('${SELECTORS.workDirButton}') && !document.querySelector('${SELECTORS.workDirButton}').disabled`,
  );
  await evaluate(`document.querySelector('${SELECTORS.workDirButton}').click()`, "应用工作区");
  await waitForExpression(
    `document.querySelector('${SELECTORS.workDirInput}')?.value.includes('crates')`,
  );
  const afterWorkDir = await evaluate(
    `(() => ({
      workDir: document.querySelector('${SELECTORS.workDirInput}')?.value ?? "",
      applyDisabled: document.querySelector('${SELECTORS.workDirButton}')?.disabled ?? true,
    }))()`,
    "工作区切换后状态",
  );
  addCheck("预览模式可切换工作区输入", afterWorkDir.workDir.endsWith("crates"), afterWorkDir);
}

async function checkTraceFiltersDuringRun() {
  await selectOpsTab(SELECTORS.opsTabTrace, SELECTORS.traceList, "切换轨迹 tab");
  await evaluate(`document.querySelector('${SELECTORS.traceFilterTool}').click()`, "筛选工具事件");
  await waitForExpression(
    `document.querySelector('${SELECTORS.traceList}')?.innerText.includes("p/owasp-top-ten")`,
  );
  const toolFilter = await evaluate(
    `(() => ({
      toolActive: document.querySelector('${SELECTORS.traceFilterTool}')?.getAttribute("aria-pressed") === "true",
      itemCount: document.querySelectorAll('${SELECTORS.traceItem}').length,
      summaryVisible: Boolean(document.querySelector('${SELECTORS.traceSummary}')),
      kindLabels: Array.from(document.querySelectorAll('${SELECTORS.traceKind}')).map((node) => node.innerText),
      structuredFieldsVisible: Boolean(document.querySelector('${SELECTORS.traceDetailFields}')),
      structuredFieldCount: document.querySelectorAll('${SELECTORS.traceDetailField}').length,
      pathFieldVisible: document.querySelector('${SELECTORS.traceList}')?.innerText.includes("路径") ?? false,
      projectPathFieldVisible: document.querySelector('${SELECTORS.traceList}')?.innerText.includes("项目路径") ?? false,
      rulesetFieldVisible: document.querySelector('${SELECTORS.traceList}')?.innerText.includes("规则集") ?? false,
      toolEventVisible: document.querySelector('${SELECTORS.traceList}')?.innerText.includes("src/views/PromptApp.vue") ?? false,
      semgrepEventVisible: document.querySelector('${SELECTORS.traceList}')?.innerText.includes("D:\\\\Project\\\\securagent") ?? false,
      rulesetValueVisible: document.querySelector('${SELECTORS.traceList}')?.innerText.includes("p/owasp-top-ten") ?? false,
      rawJsonVisible: document.querySelector('${SELECTORS.traceList}')?.innerText.includes('{"path"') ?? false,
      rawProjectPathJsonVisible: document.querySelector('${SELECTORS.traceList}')?.innerText.includes('{"project_path"') ?? false,
      tokenEventVisible: document.querySelector('${SELECTORS.traceList}')?.innerText.includes("权限边界和外部输入，") ?? false,
    }))()`,
    "工具轨迹筛选状态",
  );
  addCheck(
    "运行轨迹可筛选工具事件",
    toolFilter.toolActive &&
      toolFilter.itemCount > 0 &&
      toolFilter.summaryVisible &&
      toolFilter.kindLabels.includes("调用") &&
      toolFilter.structuredFieldsVisible &&
      toolFilter.structuredFieldCount > 0 &&
      toolFilter.pathFieldVisible &&
      toolFilter.projectPathFieldVisible &&
      toolFilter.rulesetFieldVisible &&
      toolFilter.toolEventVisible &&
      toolFilter.semgrepEventVisible &&
      toolFilter.rulesetValueVisible &&
      !toolFilter.rawJsonVisible &&
      !toolFilter.rawProjectPathJsonVisible &&
      !toolFilter.tokenEventVisible,
    toolFilter,
  );

  await selectOpsTab(SELECTORS.opsTabConfirm, SELECTORS.toolConfirmList, "切换确认 tab");
  await waitForExpression(
    `document.querySelector('${SELECTORS.toolConfirmList}')?.innerText.includes('npm audit --json')`,
  );
  const confirmation = await evaluate(
    `(() => ({
      countLabel: document.querySelector('${SELECTORS.toolConfirmCount}')?.innerText ?? "",
      itemCount: document.querySelectorAll('${SELECTORS.toolConfirmItem}').length,
      promptVisible: document.querySelector('${SELECTORS.toolConfirmList}')?.innerText.includes("npm audit --json") ?? false,
      conservativePolicyVisible: document.querySelector('${SELECTORS.toolConfirmList}')?.innerText.includes("已按保守策略拒绝") ?? false,
    }))()`,
    "工具确认请求提示状态",
  );
  addCheck(
    "工具确认请求有明确提示",
    confirmation.countLabel.includes("1") &&
      confirmation.itemCount > 0 &&
      confirmation.promptVisible &&
      confirmation.conservativePolicyVisible,
    confirmation,
  );

  await selectOpsTab(SELECTORS.opsTabTrace, SELECTORS.traceList, "恢复轨迹 tab");
  await evaluate(
    `document.querySelector('${SELECTORS.traceFilterAll}').click()`,
    "恢复全部事件筛选",
  );
  const traceNoise = await evaluate(
    `(() => ({
      allActive: document.querySelector('${SELECTORS.traceFilterAll}')?.getAttribute("aria-pressed") === "true",
      itemCount: document.querySelectorAll('${SELECTORS.traceItem}').length,
      tokenEventVisible: document.querySelector('${SELECTORS.traceList}')?.innerText.includes("权限边界和外部输入，") ?? false,
      toolEventVisible: document.querySelector('${SELECTORS.traceList}')?.innerText.includes("src/views/PromptApp.vue") ?? false,
      confirmEventVisible: document.querySelector('${SELECTORS.traceList}')?.innerText.includes("工具确认请求") ?? false,
    }))()`,
    "轨迹降噪状态",
  );
  addCheck(
    "运行轨迹不展示流式 token 噪音",
    traceNoise.allActive &&
      traceNoise.itemCount > 0 &&
      !traceNoise.tokenEventVisible &&
      traceNoise.toolEventVisible &&
      traceNoise.confirmEventVisible,
    traceNoise,
  );
}

async function captureScreenshot() {
  const screenshot = await cdp.send("Page.captureScreenshot", {
    format: "png",
    fromSurface: true,
  });
  await writeFile(screenshotPath, screenshot.data, "base64");
}

async function selectOpsTab(tabSelector, visibleSelector, label) {
  await evaluate(`document.querySelector('${tabSelector}').click()`, label);
  await waitForExpression(
    `document.querySelector('${tabSelector}')?.getAttribute("aria-pressed") === "true" && Boolean(document.querySelector('${visibleSelector}'))`,
  );
}

function addCheck(name, passed, details = {}) {
  artifacts.checks.push({
    name,
    passed,
    details,
  });
  if (!passed) {
    throw new Error(`GUI smoke check failed: ${name}`);
  }
}

async function evaluate(expression, label) {
  const response = await cdp.send("Runtime.evaluate", {
    expression,
    awaitPromise: true,
    returnByValue: true,
  });
  if (response.exceptionDetails) {
    throw new Error(`${label} 执行失败`);
  }
  return response.result.value;
}

async function waitForExpression(expression) {
  await waitUntil(async () => {
    const value = await evaluate(expression, expression);
    return Boolean(value);
  }, `等待条件超时：${expression}`);
}

async function canFetch(url) {
  try {
    const response = await fetch(url);
    return response.ok;
  } catch {
    return false;
  }
}

async function findNewListeningPids(port, initialPids) {
  const currentPids = await findListeningPids(port);
  return new Set(Array.from(currentPids).filter((pid) => !initialPids.has(pid)));
}

async function findListeningPids(port) {
  if (process.platform !== "win32") {
    return new Set();
  }

  const output = await runCommand("netstat", ["-ano", "-p", "tcp"], { allowFailure: true });
  const pids = new Set();
  for (const line of output.stdout.split(/\r?\n/)) {
    const match = line.match(/^\s*TCP\s+(\S+)\s+\S+\s+LISTENING\s+(\d+)\s*$/);
    if (!match) {
      continue;
    }
    const [, address, pid] = match;
    if (address.endsWith(`:${port}`)) {
      pids.add(Number(pid));
    }
  }
  return pids;
}

async function cleanupDevServer() {
  if (!artifacts.startedDevServer) {
    return;
  }

  const currentPids = await findNewListeningPids(DEV_SERVER_PORT, initialDevServerPids);
  const cleanupPids = new Set([devServerProcess?.pid, ...ownedDevServerPids, ...currentPids]);
  cleanupPids.delete(undefined);

  for (const pid of cleanupPids) {
    await killProcessTree(pid);
  }
  debug("dev-server-cleanup", {
    killedPids: Array.from(cleanupPids),
  });
}

async function killProcessTree(pid) {
  if (!pid) {
    return;
  }

  if (process.platform === "win32") {
    await runCommand("taskkill", ["/PID", String(pid), "/T", "/F"], { allowFailure: true });
    return;
  }

  try {
    process.kill(pid, "SIGTERM");
  } catch {
    // 进程可能已经自行退出。
  }
}

function runCommand(command, args, options = {}) {
  const { allowFailure = false, timeoutMs = PROCESS_COMMAND_TIMEOUT_MS } = options;
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      stdio: ["ignore", "pipe", "pipe"],
      windowsHide: true,
    });
    let stdout = "";
    let stderr = "";
    let settled = false;
    const settle = (callback, payload) => {
      if (settled) {
        return;
      }
      settled = true;
      clearTimeout(timer);
      callback(payload);
    };
    const timer = setTimeout(() => {
      const message = `${command} timed out after ${timeoutMs}ms`;
      child.kill("SIGKILL");
      if (allowFailure) {
        settle(resolve, { stdout, stderr: `${stderr}${message}` });
        return;
      }
      settle(reject, new Error(message));
    }, timeoutMs);

    child.stdout.on("data", (chunk) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
    });
    child.on("error", (error) => {
      if (allowFailure) {
        settle(resolve, { stdout, stderr: `${stderr}${error.message}` });
        return;
      }
      settle(reject, error);
    });
    child.on("close", (code) => {
      if (code === 0 || allowFailure) {
        settle(resolve, { stdout, stderr });
        return;
      }
      settle(reject, new Error(`${command} exited with code ${code}: ${stderr}`));
    });
  });
}

async function waitUntil(predicate, errorMessage) {
  const startedAt = Date.now();
  while (Date.now() - startedAt < WAIT_TIMEOUT_MS) {
    if (await predicate()) {
      return;
    }
    await sleep(POLL_INTERVAL_MS);
  }
  throw new Error(errorMessage);
}

function sleep(ms) {
  return new Promise((resolve) => {
    setTimeout(resolve, ms);
  });
}

function findFreePort() {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      server.close(() => {
        if (address && typeof address === "object") {
          resolve(address.port);
          return;
        }
        reject(new Error("无法获取可用端口"));
      });
    });
    server.on("error", reject);
  });
}

async function writeResult(status) {
  const payload = {
    status,
    createdAt: new Date().toISOString(),
    ...artifacts,
    ...(status === "passed" ? {} : { diagnostics }),
  };
  await writeFile(resultPath, `${JSON.stringify(payload, null, 2)}\n`, "utf8");
}

class CdpClient {
  static connect(url) {
    return new Promise((resolve, reject) => {
      const ws = new WebSocket(url);
      const client = new CdpClient(ws);
      ws.addEventListener("open", () => resolve(client), { once: true });
      ws.addEventListener("error", () => reject(new Error("CDP WebSocket 连接失败")), {
        once: true,
      });
    });
  }

  constructor(ws) {
    this.ws = ws;
    this.nextId = 1;
    this.pending = new Map();
    this.ws.addEventListener("message", (event) => {
      void this.handleMessage(event.data);
    });
    this.ws.addEventListener("close", () => {
      this.rejectPending("CDP WebSocket 已关闭");
    });
  }

  send(method, params = {}) {
    const id = this.nextId;
    this.nextId += 1;
    const payload = JSON.stringify({ id, method, params });
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`CDP 请求超时：${method}`));
      }, CDP_REQUEST_TIMEOUT_MS);
      this.pending.set(id, { resolve, reject, timeout });
      this.ws.send(payload);
    });
  }

  async handleMessage(data) {
    const text = await readWebSocketText(data);
    const message = JSON.parse(text);
    if (!message.id) {
      return;
    }
    const pending = this.pending.get(message.id);
    if (!pending) {
      return;
    }
    this.pending.delete(message.id);
    clearTimeout(pending.timeout);
    if (message.error) {
      pending.reject(new Error(message.error.message));
      return;
    }
    pending.resolve(message.result ?? {});
  }

  rejectPending(message) {
    for (const [id, pending] of this.pending) {
      clearTimeout(pending.timeout);
      pending.reject(new Error(message));
      this.pending.delete(id);
    }
  }

  close() {
    this.ws.close();
  }
}

async function readWebSocketText(data) {
  if (typeof data === "string") {
    return data;
  }
  if (data instanceof ArrayBuffer) {
    return Buffer.from(data).toString("utf8");
  }
  if (ArrayBuffer.isView(data)) {
    return Buffer.from(data.buffer, data.byteOffset, data.byteLength).toString("utf8");
  }
  if (typeof data.text === "function") {
    return data.text();
  }
  return String(data);
}

async function main() {
  try {
    await mkdir(TEST_RESULTS_DIR, { recursive: true });
    await ensureDevServer();
    const browserPath = await findBrowserPath();
    artifacts.browser = basename(browserPath);

    const cdpPort = await findFreePort();
    browserProcess = spawnBrowser(browserPath, cdpPort);
    const browserDebuggerUrl = await waitForBrowserDebugger(cdpPort);
    debug("cdp-browser", { cdpPort, webSocketDebuggerUrl: browserDebuggerUrl });
    const browserCdp = await CdpClient.connect(browserDebuggerUrl);
    const target = await browserCdp.send("Target.createTarget", { url: "about:blank" });
    const pageDebuggerUrl = await waitForPageDebugger(cdpPort, target.targetId);
    debug("cdp-page", { targetId: target.targetId, webSocketDebuggerUrl: pageDebuggerUrl });
    browserCdp.close();
    cdp = await CdpClient.connect(pageDebuggerUrl);

    await cdp.send("Runtime.enable");
    await cdp.send("Page.enable");
    await cdp.send("Emulation.setDeviceMetricsOverride", {
      ...VIEWPORT,
      deviceScaleFactor: 1,
      mobile: false,
    });
    await cdp.send("Page.navigate", { url: APP_URL });
    await waitForExpression(`Boolean(document.querySelector('${SELECTORS.appShell}'))`);

    await checkTextContent();
    await checkNoHorizontalOverflow();
    await checkOpsRailTabs();
    await checkMarkdownRendering();
    await checkWorkDirControls();
    await checkSessionFilters();
    await checkToolCapabilityPanel();
    await checkFindingPanel();
    await checkPreviewInteraction();
    await captureScreenshot();
    await writeResult("passed");
    process.exitCode = 0;
  } catch (error) {
    artifacts.error = error instanceof Error ? error.message : String(error);
    await writeResult("failed");
    process.exitCode = 1;
  } finally {
    await runFinalCleanup();
    process.exit(process.exitCode ?? 0);
  }
}

await main();

function debug(name, details = {}) {
  diagnostics.push({ name, details });
}

async function runFinalCleanup() {
  try {
    await Promise.race([
      cleanupProcesses(),
      sleep(FINAL_CLEANUP_TIMEOUT_MS).then(() => {
        throw new Error(`最终清理超时：${FINAL_CLEANUP_TIMEOUT_MS}ms`);
      }),
    ]);
  } catch (error) {
    debug("final-cleanup-error", {
      message: error instanceof Error ? error.message : String(error),
    });
  }
}

async function cleanupProcesses() {
  if (cdp) {
    cdp.close();
  }
  if (browserProcess) {
    await killProcessTree(browserProcess.pid);
  }
  await cleanupDevServer();
}
