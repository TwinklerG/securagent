# SecurAgent VS Code 插件

SecurAgent 是一个基于大语言模型的代码安全审计工具，可以直接在 VS Code 中扫描源文件，自动发现潜在漏洞并给出修复建议。

## 安装

从项目的 Release 页面下载最新的 `.vsix` 文件（内置 `secaudit` 二进制），然后通过以下任一方式安装：

- 打开 VS Code，按 `Ctrl+Shift+P`，输入 `Extensions: Install from VSIX...`，选择下载的 `.vsix` 文件。
- 或在命令行执行：`code --install-extension securagent-vscode-x.x.x.vsix`



## 快速开始

1. **配置 API Key**：安装插件后，点击左下角状态栏的 `SecurAgent`，或按 `Ctrl+Shift+P` 运行 `SecurAgent: Configure`，在菜单中设置 API Key、Base URL 和模型名称。
   - 也可以在 VS Code 设置中直接填写 `securagent.apiKey` 等字段。
   - 或者通过系统环境变量 `SECAUDIT_API_KEY`、`SECAUDIT_API_BASE_URL`、`SECAUDIT_MODEL` 配置。

2. **审计当前文件**：在编辑器中打开要审计的文件，按 `Ctrl+Shift+P` 运行 `SecurAgent: Audit Current File`，或右键文件选择 `SecurAgent: Audit Current File`。审计结果会以 Markdown 文档的形式在新标签页中展示。

3. **审计整个工作区**：按 `Ctrl+Shift+P` 运行 `SecurAgent: Audit Workspace`，插件会对当前打开的工作区进行全面的安全审计。结果以结构化形式展示（包含 Token 用量、工具调用等），并可展开查看完整 JSON 输出。

## 使用方式

### 状态栏入口

点击 VS Code 左下角的 `$(shield) SecurAgent` 按钮，弹出统一菜单，可以选择审计当前文件、审计工作区、配置或查看输出。

### 右键菜单

- **编辑器中右键**：`SecurAgent: Audit Current File`
- **资源管理器中右键文件**：`SecurAgent: Audit Current File`

### 命令面板

按 `Ctrl+Shift+P`，输入 `SecurAgent` 可以看到所有可用命令：

| 命令 | 说明 |
|------|------|
| `SecurAgent: Start Audit` | 打开统一入口，选择要执行的操作 |
| `SecurAgent: Audit Current File` | 审计当前编辑器中的文件 |
| `SecurAgent: Audit Workspace` | 审计当前 VS Code 工作区 |
| `SecurAgent: Configure` | 配置 API Key、模型、可执行文件路径等 |
| `SecurAgent: Open Output` | 打开 SecurAgent 输出面板，查看详细日志 |

## 插件设置

在 VS Code 设置中搜索 `securagent`，可以配置以下选项：

| 设置项 | 说明 | 默认值 |
|--------|------|--------|
| `securagent.apiKey` | 大模型 API Key，传给 `SECAUDIT_API_KEY` | 空 |
| `securagent.apiBaseUrl` | API 接口地址，传给 `SECAUDIT_API_BASE_URL` | 空 |
| `securagent.model` | 模型名称，传给 `SECAUDIT_MODEL` | 空 |
| `securagent.strategy` | 单文件审计策略，可选 `react` 或 `reflexion` | `react` |
| `securagent.confirmMode` | 工作区审计的工具确认策略，可选 `deny`（更安全）或 `allow` | `deny` |
| `securagent.workspacePrompt` | 工作区审计使用的提示词 | （预设值） |
| `securagent.executablePath` | 自定义 `secaudit` 可执行文件路径，一般不需要设置 | 空 |
| `securagent.repositoryPath` | securagent 仓库路径（仅开发者使用） | 空 |
| `securagent.cargoPath` | cargo 路径（仅开发者使用） | 空 |
