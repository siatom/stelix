import { marked } from 'marked'
import DOMPurify from 'dompurify'

/** 消息只允许 Markdown 展示标签；不加载远程图片或执行模型输出的 HTML。 */
export function renderMarkdown(text: string): string {
  const html = marked.parse(text, { async: false, gfm: true, breaks: true })
  const fragment = DOMPurify.sanitize(html, {
    ALLOWED_TAGS: ['p', 'br', 'strong', 'em', 'del', 's', 'code', 'pre', 'blockquote', 'ul', 'ol', 'li', 'h1', 'h2', 'h3', 'h4', 'hr', 'table', 'thead', 'tbody', 'tr', 'th', 'td', 'a'],
    ALLOWED_ATTR: ['href', 'title', 'class', 'start'],
    ALLOW_DATA_ATTR: false,
    ALLOW_ARIA_ATTR: false,
    RETURN_DOM_FRAGMENT: true,
  })
  for (const link of fragment.querySelectorAll('a')) {
    const href = link.getAttribute('href') ?? ''
    if (!/^https?:\/\//i.test(href)) link.removeAttribute('href')
  }
  const container = document.createElement('div')
  container.append(fragment)
  return container.innerHTML
}
