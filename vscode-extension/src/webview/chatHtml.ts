import type { ChatMessage } from '../types';

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

export function getChatHtml(nonce: string, initialMessages: ChatMessage[] = []): string {
  const messagesJson = JSON.stringify(initialMessages);

  return /* html */ `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'nonce-${nonce}'; script-src 'nonce-${nonce}';">
<title>SecurAgent Chat</title>
<style nonce="${nonce}">
  * { box-sizing: border-box; margin: 0; padding: 0; }
  body {
    font-family: var(--vscode-font-family, sans-serif);
    font-size: var(--vscode-font-size, 13px);
    color: var(--vscode-foreground);
    background: var(--vscode-sideBar-background, var(--vscode-editor-background));
    display: flex;
    flex-direction: column;
    height: 100vh;
  }
  #messages {
    flex: 1;
    overflow-y: auto;
    padding: 8px 12px;
  }
  .msg {
    margin-bottom: 12px;
    padding: 8px 10px;
    border-radius: 6px;
    line-height: 1.5;
    word-wrap: break-word;
    white-space: pre-wrap;
  }
  .msg-user {
    background: var(--vscode-textBlockQuote-background, rgba(128,128,128,0.1));
    border-left: 3px solid var(--vscode-textLink-foreground, #58a6ff);
  }
  .msg-assistant {
    background: var(--vscode-editorWidget-background, rgba(128,128,128,0.08));
    border-left: 3px solid var(--vscode-charts-green, #3fb950);
  }
  .msg-role {
    font-size: 0.85em;
    font-weight: 600;
    margin-bottom: 4px;
    color: var(--vscode-descriptionForeground, #8b949e);
  }
  .msg-user .msg-role { color: var(--vscode-textLink-foreground, #58a6ff); }
  .msg-assistant .msg-role { color: var(--vscode-charts-green, #3fb950); }
  #input-area {
    display: flex;
    gap: 6px;
    padding: 8px 12px;
    border-top: 1px solid var(--vscode-panel-border, #333);
  }
  #input {
    flex: 1;
    padding: 6px 8px;
    border: 1px solid var(--vscode-input-border, #3c3c3c);
    border-radius: 4px;
    background: var(--vscode-input-background, #3c3c3c);
    color: var(--vscode-input-foreground, #ccc);
    font-family: inherit;
    font-size: inherit;
    outline: none;
    resize: none;
    min-height: 32px;
    max-height: 120px;
  }
  #input:focus { border-color: var(--vscode-focusBorder, #007fd4); }
  #send {
    padding: 6px 14px;
    border: none;
    border-radius: 4px;
    background: var(--vscode-button-background, #0e639c);
    color: var(--vscode-button-foreground, #fff);
    cursor: pointer;
    font-family: inherit;
    font-size: inherit;
    align-self: flex-end;
  }
  #send:hover { background: var(--vscode-button-hoverBackground, #1177bb); }
  #send:disabled { opacity: 0.5; cursor: not-allowed; }
  .loading {
    color: var(--vscode-descriptionForeground, #8b949e);
    font-style: italic;
    padding: 8px 10px;
    margin-bottom: 12px;
  }
  .empty-state {
    color: var(--vscode-descriptionForeground, #8b949e);
    text-align: center;
    margin-top: 40px;
    line-height: 2;
  }
  .toolbar {
    display: flex;
    justify-content: flex-end;
    padding: 4px 12px;
    border-bottom: 1px solid var(--vscode-panel-border, #333);
  }
  .toolbar button {
    background: none;
    border: none;
    color: var(--vscode-descriptionForeground, #8b949e);
    cursor: pointer;
    font-size: 0.85em;
    padding: 2px 6px;
  }
  .toolbar button:hover { color: var(--vscode-foreground); }
</style>
</head>
<body>
<div class="toolbar"><button id="clear-btn">Clear</button></div>
<div id="messages"></div>
<div id="input-area">
  <textarea id="input" placeholder="Ask SecurAgent..." rows="1"></textarea>
  <button id="send">Send</button>
</div>
<script nonce="${nonce}">
(function() {
  const vscode = acquireVsCodeApi();
  const messagesEl = document.getElementById('messages');
  const inputEl = document.getElementById('input');
  const sendBtn = document.getElementById('send');
  const clearBtn = document.getElementById('clear-btn');

  let messages = ${messagesJson};
  let isLoading = false;

  function renderMessages() {
    if (messages.length === 0) {
      messagesEl.innerHTML = '<div class="empty-state">Send a message to start a SecurAgent audit.</div>';
      return;
    }
    messagesEl.innerHTML = messages.map(function(m) {
      var roleLabel = m.role === 'user' ? 'You' : 'SecurAgent';
      var cls = m.role === 'user' ? 'msg-user' : 'msg-assistant';
      return '<div class="msg ' + cls + '"><div class="msg-role">' + roleLabel + '</div><div>' + escapeHtml(m.content) + '</div></div>';
    }).join('');
    messagesEl.scrollTop = messagesEl.scrollHeight;
  }

  function escapeHtml(text) {
    var d = document.createElement('div');
    d.textContent = text;
    return d.innerHTML;
  }

  function addLoading() {
    var el = document.createElement('div');
    el.id = 'loading-indicator';
    el.className = 'loading';
    el.textContent = 'SecurAgent is working...';
    messagesEl.appendChild(el);
    messagesEl.scrollTop = messagesEl.scrollHeight;
  }

  function removeLoading() {
    var el = document.getElementById('loading-indicator');
    if (el) { el.remove(); }
  }

  function sendMessage() {
    var text = inputEl.value.trim();
    if (!text || isLoading) { return; }
    inputEl.value = '';
    inputEl.style.height = 'auto';
    isLoading = true;
    sendBtn.disabled = true;
    messages.push({ role: 'user', content: text });
    renderMessages();
    addLoading();
    vscode.postMessage({ type: 'sendMessage', text: text });
  }

  sendBtn.addEventListener('click', sendMessage);
  inputEl.addEventListener('keydown', function(e) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      sendMessage();
    }
  });
  inputEl.addEventListener('input', function() {
    inputEl.style.height = 'auto';
    inputEl.style.height = Math.min(inputEl.scrollHeight, 120) + 'px';
  });
  clearBtn.addEventListener('click', function() {
    messages = [];
    renderMessages();
    vscode.postMessage({ type: 'clearChat' });
  });

  window.addEventListener('message', function(event) {
    var msg = event.data;
    if (msg.type === 'addMessage') {
      messages.push(msg.message);
      removeLoading();
      renderMessages();
      isLoading = false;
      sendBtn.disabled = false;
      inputEl.focus();
    } else if (msg.type === 'setLoading') {
      if (msg.loading) {
        addLoading();
        isLoading = true;
        sendBtn.disabled = true;
      } else {
        removeLoading();
        isLoading = false;
        sendBtn.disabled = false;
      }
    } else if (msg.type === 'clearChat') {
      messages = [];
      renderMessages();
    }
  });

  renderMessages();
  inputEl.focus();
})();
</script>
</body>
</html>`;
}
