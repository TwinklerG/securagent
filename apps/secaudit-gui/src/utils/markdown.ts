import hljs from "highlight.js/lib/common";
import MarkdownIt from "markdown-it";

const markdown = new MarkdownIt({
  breaks: false,
  highlight: (source, language) => highlightCode(source, language),
  html: false,
  linkify: true,
  typographer: false,
});

export function renderMarkdown(source: string): string {
  return markdown.render(source);
}

function highlightCode(source: string, language: string): string {
  const normalizedLanguage = language.trim().split(/\s+/)[0] ?? "";
  if (normalizedLanguage && hljs.getLanguage(normalizedLanguage)) {
    try {
      const highlighted = hljs.highlight(source, {
        language: normalizedLanguage,
        ignoreIllegals: true,
      }).value;
      return `<pre><code class="hljs language-${markdown.utils.escapeHtml(normalizedLanguage)}">${highlighted}</code></pre>`;
    } catch {
      // 回退到安全转义的纯文本代码块。
    }
  }

  return `<pre><code class="hljs">${markdown.utils.escapeHtml(source)}</code></pre>`;
}
