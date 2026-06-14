export interface Finding {
  cwe_id: string | null;
  severity: 'Critical' | 'High' | 'Medium' | 'Low' | 'Info';
  description: string;
  location: string | null;
  remediation: string | null;
}

/** Single-file audit output (secaudit -f json <file>) */
export interface AuditReport {
  target: string;
  language: string;
  findings: Finding[];
  summary: string;
  iterations_used: number;
}

/** Workspace audit output (secaudit --mode chat --output-format json) */
export interface HeadlessResponse {
  status: 'success' | 'error';
  final_message: string;
  turns: TurnRecord[];
  trace: TraceSnapshot;
  metrics: SessionMetrics;
  duration_ms: number;
  error?: string;
}

export interface TurnRecord {
  turn_index: number;
  user_message: string;
  assistant_message: string;
  error: string | null;
  duration_ms: number;
}

export interface TraceSnapshot {
  tool_calls: ToolCallRecord[];
  state_history: string[];
  think_events: string[];
  confirm_events: ConfirmEvent[];
}

export interface ToolCallRecord {
  name: string;
  args: string;
  result: string;
}

export interface ConfirmEvent {
  prompt: string;
  approved: boolean;
  mode: string;
  source: string;
}

export interface SessionMetrics {
  token_usage: {
    prompt_tokens: number;
    completion_tokens: number;
    total_tokens: number;
  };
}

export interface AuditHistoryItem {
  id: string;
  title: string;
  timestamp: Date;
  target: string;
  findings: Finding[];
  summary: string;
  mode: 'file' | 'workspace';
  rawOutput: string;
  durationMs?: number;
  toolCalls?: ToolCallRecord[];
  tokenUsage?: SessionMetrics['token_usage'];
}

/** Chat sidebar types */
export interface ChatMessage {
  role: 'user' | 'assistant';
  content: string;
}

/** Messages from webview to extension */
export type WebviewMessage =
  | { type: 'sendMessage'; text: string }
  | { type: 'clearChat' };

/** Messages from extension to webview */
export type ExtensionMessage =
  | { type: 'addMessage'; message: ChatMessage }
  | { type: 'setLoading'; loading: boolean }
  | { type: 'clearChat' };

export interface SecurAgentConfig {
  executablePath: string;
  repositoryPath: string;
  cargoPath: string;
  apiKey: string;
  apiBaseUrl: string;
  model: string;
  strategy: string;
  confirmMode: string;
  workspacePrompt: string;
}
