import * as vscode from 'vscode';
import * as cp from 'child_process';
import * as fs from 'fs';
import * as path from 'path';
import * as crypto from 'crypto';
import type { ChatMessage, HeadlessResponse, WebviewMessage, SecurAgentConfig } from '../types';
import { getChatHtml } from './chatHtml';

interface CommandSpec {
  command: string;
  prefixArgs: string[];
  cwd: string | undefined;
}

export class ChatViewProvider implements vscode.WebviewViewProvider {
  public static readonly viewType = 'securagent.chatView';

  private webviewView: vscode.WebviewView | undefined;
  private extensionUri: vscode.Uri;
  private outputChannel: vscode.OutputChannel;

  constructor(extensionUri: vscode.Uri, outputChannel: vscode.OutputChannel) {
    this.extensionUri = extensionUri;
    this.outputChannel = outputChannel;
  }

  public resolveWebviewView(
    webviewView: vscode.WebviewView,
    _context: vscode.WebviewViewResolveContext,
    _token: vscode.CancellationToken
  ): void {
    this.webviewView = webviewView;

    webviewView.webview.options = {
      enableScripts: true,
      localResourceRoots: [],
    };

    this.updateHtml();

    webviewView.webview.onDidReceiveMessage(async (data: WebviewMessage) => {
      if (data.type === 'sendMessage') {
        await this.handleUserMessage(data.text);
      } else if (data.type === 'clearChat') {
        // nothing else to do — webview already cleared its state
      }
    });
  }

  private updateHtml(): void {
    if (!this.webviewView) { return; }
    const nonce = crypto.randomBytes(16).toString('hex');
    this.webviewView.webview.html = getChatHtml(nonce);
  }

  private async handleUserMessage(text: string): Promise<void> {
    const config = getConfig();
    const commandSpec = resolveCommandSpec(config, this.outputChannel);
    if (!commandSpec) { return; }

    const workspaceFolder = vscode.workspace.workspaceFolders?.[0];
    const cwd = workspaceFolder ? workspaceFolder.uri.fsPath : undefined;

    const args = [
      '--mode', 'chat',
      '--message', text,
      '--confirm-mode', config.confirmMode,
      '--output-format', 'json',
    ];

    this.outputChannel.appendLine(`[Chat] $ ${commandSpec.command} ${[...commandSpec.prefixArgs, ...args].join(' ')}`);
    this.outputChannel.appendLine(`[Chat] cwd: ${commandSpec.cwd || cwd || '(default)'}`);

    try {
      const stdout = await spawnAndCollect(commandSpec, args, config, cwd);
      const response = parseResponse(stdout, this.outputChannel);
      this.postMessage({ type: 'addMessage', message: response });
    } catch (err: unknown) {
      const errorMsg = err instanceof Error ? err.message : String(err);
      this.outputChannel.appendLine(`[Chat] Error: ${errorMsg}`);
      this.postMessage({
        type: 'addMessage',
        message: { role: 'assistant', content: `Error: ${errorMsg}` },
      });
    }
  }

  private postMessage(msg: { type: string; message?: ChatMessage; loading?: boolean }): void {
    this.webviewView?.webview.postMessage(msg);
  }
}

function spawnAndCollect(
  commandSpec: CommandSpec,
  args: string[],
  config: SecurAgentConfig,
  cwd: string | undefined
): Promise<string> {
  return new Promise((resolve, reject) => {
    const child = cp.spawn(
      commandSpec.command,
      [...commandSpec.prefixArgs, ...args],
      {
        cwd: commandSpec.cwd || cwd,
        env: buildEnv(config),
      }
    );

    let stdout = '';
    let stderr = '';

    child.stdout.on('data', (chunk: Buffer) => { stdout += chunk.toString(); });
    child.stderr.on('data', (chunk: Buffer) => { stderr += chunk.toString(); });

    child.on('error', (error: Error) => { reject(error); });
    child.on('close', (code: number | null) => {
      if (code === 0) {
        resolve(stdout);
      } else {
        const detail = firstUsefulLine(stderr) || firstUsefulLine(stdout) || `exit code ${code}`;
        reject(new Error(detail));
      }
    });
  });
}

function parseResponse(stdout: string, outputChannel: vscode.OutputChannel): ChatMessage {
  let parsed: HeadlessResponse;
  try {
    parsed = JSON.parse(stdout);
  } catch {
    outputChannel.appendLine('[Chat] Warning: failed to parse JSON, returning raw text.');
    return { role: 'assistant', content: stdout };
  }

  if (parsed.status === 'error') {
    return { role: 'assistant', content: `Audit failed: ${parsed.error || 'Unknown error'}` };
  }

  const parts: string[] = [];
  parts.push(parsed.final_message || 'No response.');

  if (parsed.metrics?.token_usage) {
    const t = parsed.metrics.token_usage;
    parts.push(`\nTokens: ${t.prompt_tokens} prompt + ${t.completion_tokens} completion = ${t.total_tokens} total`);
  }

  const toolCount = parsed.trace?.tool_calls?.length || 0;
  if (toolCount > 0) {
    parts.push(`Tool calls: ${toolCount}`);
  }

  return { role: 'assistant', content: parts.join('\n\n') };
}

// --- Helpers (mirror extension.ts) ---

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
  if (config.apiKey) { env.SECAUDIT_API_KEY = config.apiKey; }
  if (config.apiBaseUrl) { env.SECAUDIT_API_BASE_URL = config.apiBaseUrl; }
  if (config.model) { env.SECAUDIT_MODEL = config.model; }
  return env;
}

function resolveCommandSpec(
  config: SecurAgentConfig,
  outputChannel: vscode.OutputChannel
): CommandSpec | undefined {
  if (config.executablePath) {
    return { command: config.executablePath, prefixArgs: [], cwd: undefined };
  }

  const bundled = resolveBundledBinary();
  if (bundled) {
    return { command: bundled, prefixArgs: [], cwd: undefined };
  }

  const cargoPath = resolveCargoPath(config.cargoPath);
  if (!cargoPath) {
    outputChannel.appendLine('[Chat] No secaudit binary found. Set securagent.executablePath.');
    return undefined;
  }

  const repositoryPath = resolveRepositoryPath(config.repositoryPath);
  if (!repositoryPath) {
    outputChannel.appendLine('[Chat] No repository found. Set securagent.executablePath or securagent.repositoryPath.');
    return undefined;
  }

  return {
    command: cargoPath,
    prefixArgs: ['run', '--manifest-path', path.join(repositoryPath, 'Cargo.toml'), '-p', 'secaudit', '--'],
    cwd: undefined,
  };
}

function resolveBundledBinary(): string | undefined {
  const binaryName = process.platform === 'win32' ? 'secaudit.exe' : 'secaudit';
  const bundledPath = path.join(__dirname, '..', 'bin', binaryName);
  if (fs.existsSync(bundledPath)) { return bundledPath; }
  const repoTarget = path.join(__dirname, '..', '..', 'target', 'release', binaryName);
  if (fs.existsSync(repoTarget)) { return repoTarget; }
  return undefined;
}

function resolveCargoPath(configuredPath: string): string {
  if (configuredPath) { return configuredPath; }
  const cargoExecutable = process.platform === 'win32' ? 'cargo.exe' : 'cargo';
  const candidates = [
    process.env.CARGO,
    process.env.USERPROFILE ? path.join(process.env.USERPROFILE, '.cargo', 'bin', cargoExecutable) : undefined,
    process.env.HOME ? path.join(process.env.HOME, '.cargo', 'bin', cargoExecutable) : undefined,
  ].filter(Boolean) as string[];
  return candidates.find((c) => fs.existsSync(c)) || 'cargo';
}

function resolveRepositoryPath(configuredPath: string): string | undefined {
  const candidates = [
    configuredPath,
    path.resolve(__dirname, '..'),
    ...(vscode.workspace.workspaceFolders || []).map((f) => f.uri.fsPath),
  ].filter(Boolean);
  return candidates.find((c) =>
    fs.existsSync(path.join(c, 'Cargo.toml')) &&
    fs.existsSync(path.join(c, 'secaudit', 'Cargo.toml'))
  );
}

function firstUsefulLine(text: string): string | undefined {
  return text.split(/\r?\n/).map((l) => l.trim()).find(Boolean);
}
