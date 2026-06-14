import * as vscode from 'vscode';
import * as crypto from 'crypto';
import type { AuditHistoryItem } from '../types';
import { getWebviewHtml } from './resultHtml';

export class ResultPanel {
  public static currentPanel: ResultPanel | undefined;

  private readonly panel: vscode.WebviewPanel;
  private item: AuditHistoryItem;

  private constructor(panel: vscode.WebviewPanel, item: AuditHistoryItem) {
    this.panel = panel;
    this.item = item;
    this.update();

    this.panel.onDidDispose(() => {
      ResultPanel.currentPanel = undefined;
    });
  }

  public static createOrShow(item: AuditHistoryItem, extensionUri: vscode.Uri): void {
    const column = vscode.ViewColumn.Beside;

    if (ResultPanel.currentPanel) {
      ResultPanel.currentPanel.item = item;
      ResultPanel.currentPanel.update();
      ResultPanel.currentPanel.panel.reveal(column);
      return;
    }

    const panel = vscode.window.createWebviewPanel(
      'securagentResult',
      `SecurAgent: ${item.title}`,
      column,
      {
        enableScripts: false,
        retainContextWhenHidden: true,
        localResourceRoots: [],
      }
    );

    ResultPanel.currentPanel = new ResultPanel(panel, item);
  }

  public dispose(): void {
    this.panel.dispose();
  }

  private update(): void {
    const nonce = crypto.randomBytes(16).toString('hex');
    this.panel.title = `SecurAgent: ${this.item.title}`;
    this.panel.webview.html = getWebviewHtml(this.item, nonce);
  }
}
