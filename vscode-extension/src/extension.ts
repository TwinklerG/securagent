import * as vscode from 'vscode';
import * as cp from 'child_process';
import * as crypto from 'crypto';
import * as fs from 'fs';
import * as path from 'path';
import type { AuditHistoryItem, HeadlessResponse, SecurAgentConfig } from './types';
import { ResultPanel } from './webview/ResultPanel';
import { ChatViewProvider } from './webview/ChatViewProvider';
import { AuditHistoryProvider } from './treeView/AuditHistoryProvider';

interface CommandSpec {
  command: string;
  prefixArgs: string[];
  cwd: string | undefined;
}

interface AuditOptions {
  title: string;
  args: string[];
  cwd: string;
  mode: 'file' | 'workspace';
  targetName: string;
}

interface ConfigureQuickPickItem extends vscode.QuickPickItem {
  run: () => Promise<void>;
}

interface StartAuditQuickPickItem extends vscode.QuickPickItem {
  command: string;
}

let outputChannel: vscode.OutputChannel;
let statusBarItem: vscode.StatusBarItem;
let activeAudit: string | undefined;
let extensionUri: vscode.Uri;
let auditHistoryProvider: AuditHistoryProvider;

export function activate(context: vscode.ExtensionContext): void {
  extensionUri = context.extensionUri;

  outputChannel = vscode.window.createOutputChannel('SecurAgent');

  auditHistoryProvider = new AuditHistoryProvider(context);

  statusBarItem = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 100);
  statusBarItem.command = 'securagent.startAudit';
  statusBarItem.text = '$(shield) SecurAgent';
  statusBarItem.tooltip = 'Run a SecurAgent security audit';
  statusBarItem.show();

  context.subscriptions.push(
    vscode.commands.registerCommand('securagent.startAudit', startAudit),
    vscode.commands.registerCommand('securagent.auditCurrentFile', auditCurrentFile),
    vscode.commands.registerCommand('securagent.auditWorkspace', auditWorkspace),
    vscode.commands.registerCommand('securagent.configure', configureSecurAgent),
    vscode.commands.registerCommand('securagent.openOutput', () => outputChannel.show(true)),
    vscode.commands.registerCommand('securagent.showAuditResult', (item: AuditHistoryItem) => {
      ResultPanel.createOrShow(item, extensionUri);
    }),
    vscode.commands.registerCommand('securagent.goToLocation', async (location: string) => {
      const match = location.match(/^(.*?)(?::(\d+))?(?::(\d+))?$/);
      const filePath = match?.[1] ?? location;
      const line = match?.[2] ? parseInt(match[2], 10) - 1 : 0;
      const col = match?.[3] ? parseInt(match[3], 10) - 1 : 0;
      try {
        const doc = await vscode.workspace.openTextDocument(filePath);
        const editor = await vscode.window.showTextDocument(doc);
        const pos = new vscode.Position(line, col);
        editor.selection = new vscode.Selection(pos, pos);
        editor.revealRange(new vscode.Range(pos, pos), vscode.TextEditorRevealType.InCenter);
      } catch {
        vscode.window.showWarningMessage(`Cannot open: ${filePath}`);
      }
    }),
    vscode.window.registerWebviewViewProvider(
      ChatViewProvider.viewType,
      new ChatViewProvider(context.extensionUri, outputChannel),
      { webviewOptions: { retainContextWhenHidden: true } }
    ),
    vscode.window.registerTreeDataProvider('securagent.auditHistory', auditHistoryProvider),
    statusBarItem,
    outputChannel
  );
}

export function deactivate(): void {}

// --- Audit commands ---

async function auditCurrentFile(resource?: vscode.Uri): Promise<void> {
  if (activeAudit) {
    vscode.window.showWarningMessage('SecurAgent is already running an audit.');
    return;
  }

  const target = await resolveTargetFile(resource);
  if (!target) {
    return;
  }

  const workspaceFolder = vscode.workspace.getWorkspaceFolder(vscode.Uri.file(target));
  const cwd = workspaceFolder ? workspaceFolder.uri.fsPath : path.dirname(target);
  const config = getConfig();
  const args = [target, '--format', 'markdown', '--strategy', config.strategy];

  await runAudit({
    title: `Auditing ${path.basename(target)}`,
    args,
    cwd,
    mode: 'file',
    targetName: path.basename(target),
  });
}

async function auditWorkspace(): Promise<void> {
  if (activeAudit) {
    vscode.window.showWarningMessage('SecurAgent is already running an audit.');
    return;
  }

  const workspaceFolder = await resolveWorkspaceFolder();
  if (!workspaceFolder) {
    return;
  }

  const config = getConfig();
  const args = [
    '--mode',
    'chat',
    '--message',
    config.workspacePrompt,
    '--confirm-mode',
    config.confirmMode,
    '--output-format',
    'json',
  ];

  await runAudit({
    title: `Auditing ${workspaceFolder.name}`,
    args,
    cwd: workspaceFolder.uri.fsPath,
    mode: 'workspace',
    targetName: workspaceFolder.name,
  });
}

async function startAudit(resource?: vscode.Uri): Promise<void> {
  const picks: StartAuditQuickPickItem[] = [
    {
      label: '$(file-code) Audit Current File',
      description: 'Scan the active editor or selected file',
      command: 'securagent.auditCurrentFile',
    },
    {
      label: '$(folder-library) Audit Workspace',
      description: 'Run a broader agent audit in the current workspace',
      command: 'securagent.auditWorkspace',
    },
    {
      label: '$(settings-gear) Configure SecurAgent',
      description: 'Set API key, model, executable path, and defaults',
      command: 'securagent.configure',
    },
    {
      label: '$(output) Open Output',
      description: 'Show the SecurAgent output channel',
      command: 'securagent.openOutput',
    },
  ];

  const selected = await vscode.window.showQuickPick(picks, {
    title: 'SecurAgent',
    placeHolder: 'Choose what to do',
  });
  if (selected) {
    await vscode.commands.executeCommand(selected.command, resource);
  }
}

// --- Configure ---

async function configureSecurAgent(): Promise<void> {
  const config = getConfig();
  const settings = vscode.workspace.getConfiguration('securagent');
  const target = vscode.ConfigurationTarget.Global;

  const action = await vscode.window.showQuickPick<ConfigureQuickPickItem>(
    [
      {
        label: '$(key) Set API Key',
        description: config.apiKey
          ? `Configured (${maskApiKey(config.apiKey)})`
          : 'Not set — required unless set in your environment',
        run: async () => {
          const apiKey = await vscode.window.showInputBox({
            title: 'SecurAgent API Key',
            prompt: 'Stored in VS Code User Settings as securagent.apiKey.',
            password: true,
            ignoreFocusOut: true,
            value: config.apiKey,
          });
          if (apiKey !== undefined) {
            await settings.update('apiKey', apiKey.trim(), target);
          }
        },
      },
      {
        label: '$(server-environment) Set API Base URL',
        description: config.apiBaseUrl || 'Default',
        run: async () => {
          const apiBaseUrl = await vscode.window.showInputBox({
            title: 'SecurAgent API Base URL',
            prompt: 'For OpenAI-compatible providers, for example https://api.openai.com/v1.',
            ignoreFocusOut: true,
            value: config.apiBaseUrl,
          });
          if (apiBaseUrl !== undefined) {
            await settings.update('apiBaseUrl', apiBaseUrl.trim(), target);
          }
        },
      },
      {
        label: '$(symbol-method) Set Model',
        description: config.model || 'Default',
        run: async () => {
          const model = await vscode.window.showInputBox({
            title: 'SecurAgent Model',
            prompt: 'Model name passed as SECAUDIT_MODEL.',
            ignoreFocusOut: true,
            value: config.model,
          });
          if (model !== undefined) {
            await settings.update('model', model.trim(), target);
          }
        },
      },
      {
        label: '$(run) Strategy',
        description: `Current: ${config.strategy}`,
        run: async () => {
          const choice = await vscode.window.showQuickPick(['react', 'reflexion'], {
            title: 'SecurAgent Strategy',
            placeHolder: 'Choose the reasoning strategy',
          });
          if (choice) {
            await settings.update('strategy', choice, target);
          }
        },
      },
      {
        label: '$(terminal) Set Executable Path',
        description: config.executablePath || 'Use bundled binary',
        run: async () => {
          const selected = await vscode.window.showOpenDialog({
            title: 'Choose secaudit executable',
            canSelectFiles: true,
            canSelectFolders: false,
            canSelectMany: false,
            filters: process.platform === 'win32' ? { Executable: ['exe'] } : undefined,
          });
          if (selected && selected[0]) {
            await settings.update('executablePath', selected[0].fsPath, target);
          }
        },
      },
      {
        label: '$(json) Open Settings',
        description: 'Edit all SecurAgent settings directly',
        run: async () =>
          vscode.commands.executeCommand(
            'workbench.action.openSettings',
            '@ext:securagent.securagent-vscode'
          ),
      },
    ],
    {
      title: 'Configure SecurAgent',
      placeHolder: 'Choose a setting to update',
    }
  );

  if (action) {
    await action.run();
    updateStatusBar();
  }
}

// --- Core audit execution ---

async function runAudit(options: AuditOptions): Promise<void> {
  const config = getConfig();
  const commandSpec = resolveCommandSpec(config);
  if (!commandSpec) {
    return;
  }

  outputChannel.clear();
  outputChannel.show(true);
  outputChannel.appendLine(
    `$ ${commandSpec.command} ${[...commandSpec.prefixArgs, ...options.args].join(' ')}`
  );
  outputChannel.appendLine(`cwd: ${options.cwd}`);
  outputChannel.appendLine(`model: ${config.model || '(default)'}`);
  outputChannel.appendLine(`strategy: ${config.strategy}`);
  outputChannel.appendLine('');

  await vscode.window.withProgress(
    {
      location: vscode.ProgressLocation.Window,
      title: `SecurAgent: ${options.title}`,
      cancellable: true,
    },
    (progress, token) =>
      new Promise<void>((resolve) => {
        activeAudit = options.title;
        updateStatusBar(`$(sync~spin) ${options.title}`);
        progress.report({ message: 'Running secaudit...' });

        const child = cp.spawn(
          commandSpec!.command,
          [...commandSpec!.prefixArgs, ...options.args],
          {
            cwd: commandSpec!.cwd || options.cwd,
            env: buildEnv(config),
          }
        );

        let stdout = '';
        let stderr = '';

        token.onCancellationRequested(() => {
          child.kill();
          outputChannel.appendLine('');
          outputChannel.appendLine('Audit cancelled.');
        });

        child.stdout.on('data', (chunk: Buffer) => {
          const text = chunk.toString();
          stdout += text;
          outputChannel.append(text);
        });

        child.stderr.on('data', (chunk: Buffer) => {
          const text = chunk.toString();
          stderr += text;
          outputChannel.append(text);
        });

        child.on('error', async (error: Error) => {
          outputChannel.appendLine(`Failed to start secaudit: ${error.message}`);
          await vscode.window.showErrorMessage(`SecurAgent failed to start: ${error.message}`);
          activeAudit = undefined;
          updateStatusBar();
          resolve();
        });

        child.on('close', async (code: number | null) => {
          outputChannel.appendLine('');
          outputChannel.appendLine(`secaudit exited with code ${code}`);
          activeAudit = undefined;

          if (code === 0) {
            const item = buildAuditItem(stdout, options);
            auditHistoryProvider.addItem(item);
            ResultPanel.createOrShow(item, extensionUri);
            updateStatusBar();
            vscode.window.showInformationMessage('SecurAgent audit completed.');
          } else {
            const message =
              firstUsefulLine(stderr) || firstUsefulLine(stdout) || `secaudit exited with code ${code}`;
            updateStatusBar();
            const action = await vscode.window.showErrorMessage(
              `SecurAgent audit failed: ${message}`,
              'Show Output',
              'Configure'
            );
            if (action === 'Show Output') {
              outputChannel.show(true);
            } else if (action === 'Configure') {
              await configureSecurAgent();
            }
          }
          resolve();
        });
      })
  );
}

function buildAuditItem(stdout: string, options: AuditOptions): AuditHistoryItem {
  const base: AuditHistoryItem = {
    id: crypto.randomUUID(),
    title: options.title,
    timestamp: new Date(),
    target: options.targetName,
    findings: [],
    summary: stdout,
    mode: options.mode,
    rawOutput: stdout,
  };

  if (options.mode === 'workspace') {
    return parseHeadlessResponse(stdout, base);
  }

  return base;
}

function parseHeadlessResponse(stdout: string, base: AuditHistoryItem): AuditHistoryItem {
  let parsed: HeadlessResponse;
  try {
    parsed = JSON.parse(stdout);
  } catch {
    outputChannel.appendLine('Warning: failed to parse headless JSON output, displaying raw text.');
    return { ...base, summary: stdout };
  }

  if (parsed.status === 'error') {
    return {
      ...base,
      summary: `**Audit Error**\n\n${parsed.error || 'Unknown error'}`,
      rawOutput: JSON.stringify(parsed, null, 2),
      durationMs: parsed.duration_ms,
    };
  }

  const summaryParts: string[] = [];
  summaryParts.push(parsed.final_message || 'No summary returned.');

  if (parsed.metrics?.token_usage) {
    const t = parsed.metrics.token_usage;
    summaryParts.push(
      `\n---\n**Token Usage** — prompt: ${t.prompt_tokens}, completion: ${t.completion_tokens}, total: ${t.total_tokens}`
    );
  }

  const toolCalls = parsed.trace?.tool_calls || [];
  if (toolCalls.length > 0) {
    summaryParts.push(`\n**Tool Calls** — ${toolCalls.length} total`);
  }

  if (parsed.turns?.length) {
    summaryParts.push(`**Conversation Turns** — ${parsed.turns.length}`);
  }

  return {
    ...base,
    summary: summaryParts.join('\n\n'),
    rawOutput: JSON.stringify(parsed, null, 2),
    durationMs: parsed.duration_ms,
    toolCalls,
    tokenUsage: parsed.metrics?.token_usage,
  };
}

// --- Status bar ---

function updateStatusBar(text?: string): void {
  if (!statusBarItem) {
    return;
  }
  statusBarItem.text = text || '$(shield) SecurAgent';
  statusBarItem.tooltip = activeAudit
    ? `SecurAgent is running: ${activeAudit}`
    : 'Run a SecurAgent security audit';
}

// --- Helpers ---

async function resolveTargetFile(resource?: vscode.Uri): Promise<string | undefined> {
  if (resource && resource.fsPath) {
    return resource.fsPath;
  }

  const editor = vscode.window.activeTextEditor;
  if (!editor || editor.document.uri.scheme !== 'file') {
    vscode.window.showWarningMessage('Open a file first, then run SecurAgent: Audit Current File.');
    return undefined;
  }

  if (editor.document.isDirty) {
    const choice = await vscode.window.showWarningMessage(
      'The current file has unsaved changes. Save before auditing?',
      'Save and Audit',
      'Audit Saved Version'
    );
    if (choice === 'Save and Audit') {
      await editor.document.save();
    } else if (!choice) {
      return undefined;
    }
  }

  return editor.document.uri.fsPath;
}

async function resolveWorkspaceFolder(): Promise<vscode.WorkspaceFolder | undefined> {
  const folders = vscode.workspace.workspaceFolders || [];
  if (folders.length === 0) {
    vscode.window.showWarningMessage(
      'Open a workspace folder before running a SecurAgent workspace audit.'
    );
    return undefined;
  }
  if (folders.length === 1) {
    return folders[0];
  }
  return vscode.window.showWorkspaceFolderPick({ placeHolder: 'Choose a workspace to audit' });
}

function resolveCommandSpec(config: SecurAgentConfig): CommandSpec | undefined {
  if (config.executablePath) {
    return { command: config.executablePath, prefixArgs: [], cwd: undefined };
  }

  const bundled = resolveBundledBinary();
  if (bundled) {
    return { command: bundled, prefixArgs: [], cwd: undefined };
  }

  const cargoPath = resolveCargoPath(config.cargoPath);
  if (!cargoPath) {
    vscode.window
      .showErrorMessage(
        'SecurAgent: no bundled binary found and cargo is not installed. Set securagent.executablePath to a prebuilt secaudit binary, or install Rust.',
        'Configure'
      )
      .then((action) => {
        if (action === 'Configure') {
          configureSecurAgent();
        }
      });
    return undefined;
  }

  const repositoryPath = resolveRepositoryPath(config.repositoryPath);
  if (!repositoryPath) {
    vscode.window
      .showErrorMessage(
        'SecurAgent: no bundled binary found and the repository was not found. Set securagent.executablePath or securagent.repositoryPath.',
        'Configure'
      )
      .then((action) => {
        if (action === 'Configure') {
          configureSecurAgent();
        }
      });
    return undefined;
  }

  return {
    command: cargoPath,
    prefixArgs: [
      'run',
      '--manifest-path',
      path.join(repositoryPath, 'Cargo.toml'),
      '-p',
      'secaudit',
      '--',
    ],
    cwd: undefined,
  };
}

function resolveBundledBinary(): string | undefined {
  const binaryName = process.platform === 'win32' ? 'secaudit.exe' : 'secaudit';

  const bundledPath = path.join(__dirname, '..', 'bin', binaryName);
  if (fs.existsSync(bundledPath)) {
    return bundledPath;
  }

  const repoTarget = path.join(__dirname, '..', '..', 'target', 'release', binaryName);
  if (fs.existsSync(repoTarget)) {
    return repoTarget;
  }

  return undefined;
}

function resolveCargoPath(configuredPath: string): string {
  if (configuredPath) {
    return configuredPath;
  }

  const cargoExecutable = process.platform === 'win32' ? 'cargo.exe' : 'cargo';
  const candidates = [
    process.env.CARGO,
    process.env.USERPROFILE
      ? path.join(process.env.USERPROFILE, '.cargo', 'bin', cargoExecutable)
      : undefined,
    process.env.HOME ? path.join(process.env.HOME, '.cargo', 'bin', cargoExecutable) : undefined,
  ].filter(Boolean) as string[];

  const installedCargo = candidates.find((candidate) => fs.existsSync(candidate));
  return installedCargo || 'cargo';
}

function resolveRepositoryPath(configuredPath: string): string | undefined {
  const candidates = [
    configuredPath,
    path.resolve(__dirname, '..'),
    ...(vscode.workspace.workspaceFolders || []).map((folder) => folder.uri.fsPath),
  ].filter(Boolean);

  return candidates.find((candidate) => {
    return (
      fs.existsSync(path.join(candidate, 'Cargo.toml')) &&
      fs.existsSync(path.join(candidate, 'secaudit', 'Cargo.toml'))
    );
  });
}

function getConfig(): SecurAgentConfig {
  const config = vscode.workspace.getConfiguration('securagent');
  return {
    executablePath: config.get('executablePath', '').trim(),
    repositoryPath: config.get('repositoryPath', '').trim(),
    cargoPath: config.get('cargoPath', '').trim(),
    apiKey: config.get('apiKey', '').trim(),
    apiBaseUrl: config.get('apiBaseUrl', '').trim(),
    model: config.get('model', '').trim(),
    strategy: config.get('strategy', 'react'),
    confirmMode: config.get('confirmMode', 'deny'),
    workspacePrompt: config.get('workspacePrompt', ''),
  };
}

function buildEnv(config: SecurAgentConfig): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = { ...process.env };
  if (config.apiKey) {
    env.SECAUDIT_API_KEY = config.apiKey;
  }
  if (config.apiBaseUrl) {
    env.SECAUDIT_API_BASE_URL = config.apiBaseUrl;
  }
  if (config.model) {
    env.SECAUDIT_MODEL = config.model;
  }
  return env;
}

function firstUsefulLine(text: string): string | undefined {
  return text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .find(Boolean);
}

function maskApiKey(key: string): string {
  if (key.length <= 8) {
    return '****' + key.slice(-2);
  }
  return '****' + key.slice(-4);
}
