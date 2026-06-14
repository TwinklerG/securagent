import { marked, Renderer } from 'marked';
import type { AuditHistoryItem } from '../types';

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

const ALLOWED_PROTOCOLS = ['https:', 'http:', 'mailto:'];

function isUrlSafe(href: string): boolean {
  try {
    const url = new URL(href, 'https://placeholder');
    return ALLOWED_PROTOCOLS.includes(url.protocol);
  } catch {
    return false;
  }
}

function createSafeRenderer(): Renderer {
  const renderer = new Renderer();
  renderer.link = ({ href, title, text }) => {
    if (!isUrlSafe(href)) {
      return `<span>${text}</span>`;
    }
    const titleAttr = title ? ` title="${escapeHtml(title)}"` : '';
    return `<a href="${escapeHtml(href)}"${titleAttr} rel="noopener noreferrer">${text}</a>`;
  };
  renderer.image = ({ href, title, text }) => {
    if (!isUrlSafe(href)) {
      return `<span>[${escapeHtml(text || 'image')}]</span>`;
    }
    const titleAttr = title ? ` title="${escapeHtml(title)}"` : '';
    return `<img src="${escapeHtml(href)}" alt="${escapeHtml(text || '')}"${titleAttr}>`;
  };
  renderer.html = () => '';
  return renderer;
}

const safeRenderer = createSafeRenderer();

marked.use({
  renderer: safeRenderer,
  breaks: false,
  gfm: true,
  pedantic: false,
});

export function getWebviewHtml(item: AuditHistoryItem, webviewNonce: string): string {
  const header = `
    <div class="report-header">
      <h1>${escapeHtml(item.title)}</h1>
      <div class="meta">
        <span>${escapeHtml(item.target)}</span>
        <span>${item.timestamp.toLocaleString()}</span>
      </div>
    </div>`;

  let rawSection = '';
  if (item.mode === 'workspace' && item.rawOutput) {
    rawSection = `
    <details class="raw-output">
      <summary>Raw JSON Output</summary>
      <pre><code>${escapeHtml(item.rawOutput)}</code></pre>
    </details>`;
  }

  const body = `${header}<div class="markdown-body">${marked.parse(item.summary) as string}</div>${rawSection}`;

  return /* html */ `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'nonce-${webviewNonce}';">
<title>SecurAgent Audit</title>
<style nonce="${webviewNonce}">
  :root {
    --font-size: 13px;
  }
  body {
    font-family: var(--vscode-font-family, sans-serif), "Microsoft YaHei", "PingFang SC", "Hiragino Sans GB", "Source Han Sans CN", "WenQuanYi Micro Hei", sans-serif;
    font-size: var(--vscode-font-size, var(--font-size));
    color: var(--vscode-foreground);
    background: var(--vscode-editor-background);
    padding: 16px 24px;
    line-height: 1.6;
  }
  h1 { font-size: 1.4em; margin-bottom: 4px; }
  h2 { font-size: 1.15em; margin-top: 24px; margin-bottom: 8px; border-bottom: 1px solid var(--vscode-panel-border, #333); padding-bottom: 4px; }
  h3 { font-size: 1em; margin-top: 16px; margin-bottom: 6px; }
  code {
    font-family: var(--vscode-editor-font-family, monospace), "Microsoft YaHei", "PingFang SC", monospace;
    font-size: 0.92em;
    background: var(--vscode-textCodeBlock-background, rgba(128,128,128,0.15));
    padding: 1px 4px;
    border-radius: 3px;
  }
  a { color: var(--vscode-textLink-foreground, #58a6ff); }
  a:hover { color: var(--vscode-textLink-activeForeground, #79c0ff); }

  .report-header { margin-bottom: 16px; }
  .meta { display: flex; gap: 16px; flex-wrap: wrap; color: var(--vscode-descriptionForeground, #8b949e); font-size: 0.9em; }

  .markdown-body { line-height: 1.7; }
  .markdown-body p { margin: 4px 0 8px; }
  .markdown-body h2, .markdown-body h3, .markdown-body h4 {
    margin-top: 18px; margin-bottom: 6px;
  }
  .markdown-body pre {
    background: var(--vscode-textCodeBlock-background, rgba(128,128,128,0.12));
    border-radius: 4px;
    padding: 10px 14px;
    overflow-x: auto;
    margin: 8px 0;
  }
  .markdown-body pre code { background: none; padding: 0; font-size: 0.9em; }
  .markdown-body table {
    border-collapse: collapse; margin: 8px 0; font-size: 0.92em; width: 100%;
  }
  .markdown-body th, .markdown-body td {
    border: 1px solid var(--vscode-panel-border, #444);
    padding: 5px 10px; text-align: left;
  }
  .markdown-body th {
    background: var(--vscode-editorWidget-background, rgba(128,128,128,0.12));
    font-weight: 600;
  }
  .markdown-body tr:hover td {
    background: var(--vscode-list-hoverBackground, rgba(128,128,128,0.08));
  }
  .markdown-body ul, .markdown-body ol { margin: 4px 0; padding-left: 24px; }
  .markdown-body li { margin: 2px 0; }
  .markdown-body hr { border: none; border-top: 1px solid var(--vscode-panel-border, #444); margin: 12px 0; }
  .markdown-body strong { font-weight: 600; }
  .markdown-body em { font-style: italic; }

  .raw-output { margin-top: 20px; }
  .raw-output summary {
    cursor: pointer;
    color: var(--vscode-descriptionForeground, #8b949e);
    font-size: 0.9em;
    margin-bottom: 8px;
  }
  .raw-output pre {
    background: var(--vscode-textCodeBlock-background, rgba(128,128,128,0.12));
    border-radius: 4px;
    padding: 10px 14px;
    overflow-x: auto;
    font-size: 0.85em;
    max-height: 400px;
    overflow-y: auto;
  }
  .raw-output pre code { background: none; padding: 0; }
</style>
</head>
<body>
${body}
</body>
</html>`;
}
