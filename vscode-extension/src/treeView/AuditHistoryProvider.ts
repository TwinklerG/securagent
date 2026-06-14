import * as vscode from 'vscode';
import type { AuditHistoryItem, Finding } from '../types';

const severityIcon: Record<string, string> = {
  Critical: 'error',
  High: 'warning',
  Medium: 'circle-outline',
  Low: 'info',
  Info: 'question',
};

class AuditNode extends vscode.TreeItem {
  constructor(
    public readonly label: string,
    public readonly collapsible: vscode.TreeItemCollapsibleState,
    public readonly contextValue: string,
    public readonly auditItem?: AuditHistoryItem,
    public readonly finding?: Finding,
    public readonly findingIndex?: number
  ) {
    super(label, collapsible);
  }
}

type StoredItem = Omit<AuditHistoryItem, 'timestamp'> & { timestamp: string };

function serialize(item: AuditHistoryItem): StoredItem {
  return { ...item, timestamp: item.timestamp.toISOString() };
}

function deserialize(item: StoredItem): AuditHistoryItem {
  return { ...item, timestamp: new Date(item.timestamp) };
}

export class AuditHistoryProvider implements vscode.TreeDataProvider<AuditNode> {
  private _onDidChangeTreeData = new vscode.EventEmitter<AuditNode | undefined>();
  readonly onDidChangeTreeData = this._onDidChangeTreeData.event;

  private history: AuditHistoryItem[] = [];
  private context: vscode.ExtensionContext;

  constructor(context: vscode.ExtensionContext) {
    this.context = context;
    const saved = context.globalState.get<StoredItem[]>('auditHistory', []);
    this.history = saved.map(deserialize);
  }

  getTreeItem(element: AuditNode): vscode.TreeItem {
    const item = new vscode.TreeItem(element.label, element.collapsible);
    item.contextValue = element.contextValue;
    item.tooltip = element.finding
      ? `${element.finding.severity}: ${element.finding.description}`
      : element.auditItem
        ? `${element.auditItem.title} — ${element.auditItem.findings.length} finding(s) at ${element.auditItem.timestamp.toLocaleTimeString()}`
        : element.label;

    if (element.contextValue === 'audit' && element.auditItem) {
      const count = element.auditItem.findings.length;
      item.description = `${count} issue${count !== 1 ? 's' : ''} — ${element.auditItem.timestamp.toLocaleTimeString()}`;
      item.iconPath = new vscode.ThemeIcon(count > 0 ? 'warning' : 'pass');
      item.command = {
        command: 'securagent.showAuditResult',
        title: 'Show Audit Result',
        arguments: [element.auditItem],
      };
    }

    if (element.contextValue === 'finding' && element.finding) {
      const iconName = severityIcon[element.finding.severity] || 'circle-outline';
      item.iconPath = new vscode.ThemeIcon(iconName);
      item.description = element.finding.cwe_id || '';
      if (element.finding.location) {
        item.command = {
          command: 'securagent.goToLocation',
          title: 'Go to Location',
          arguments: [element.finding.location],
        };
      }
    }

    return item;
  }

  getChildren(element?: AuditNode): AuditNode[] {
    if (!element) {
      return this.history.map(
        (h) =>
          new AuditNode(
            h.title,
            vscode.TreeItemCollapsibleState.Collapsed,
            'audit',
            h
          )
      );
    }

    if (element.contextValue === 'audit' && element.auditItem) {
      if (element.auditItem.findings.length === 0) {
        return [
          new AuditNode(
            'No vulnerabilities found',
            vscode.TreeItemCollapsibleState.None,
            'empty'
          ),
        ];
      }
      return element.auditItem.findings.map(
        (f, i) =>
          new AuditNode(
            `${f.cwe_id || 'Issue'}: ${truncate(f.description, 60)}`,
            vscode.TreeItemCollapsibleState.None,
            'finding',
            undefined,
            f,
            i
          )
      );
    }

    return [];
  }

  addItem(item: AuditHistoryItem): void {
    this.history.unshift(item);
    if (this.history.length > 20) {
      this.history = this.history.slice(0, 20);
    }
    this.persist();
    this._onDidChangeTreeData.fire(undefined);
  }

  clear(): void {
    this.history = [];
    this.persist();
    this._onDidChangeTreeData.fire(undefined);
  }

  getLastItem(): AuditHistoryItem | undefined {
    return this.history[0];
  }

  private persist(): void {
    this.context.globalState.update('auditHistory', this.history.map(serialize));
  }
}

function truncate(s: string, max: number): string {
  return s.length > max ? s.slice(0, max - 1) + '…' : s;
}
