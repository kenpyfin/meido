const FENCE = /^\s{0,3}(`{3,}|~{3,})/
const INLINE_CODE = /(`+)[\s\S]*?(?<!`)\1(?!`)/g
const DISPLAY = /\\\[([\s\S]+?)\\\]/g
const INLINE = /\\\(([\s\S]+?)\\\)/g

function convertProse(text: string): string {
  return text
    .replace(DISPLAY, (match, inner: string, offset: number, whole: string) => {
      const lineStart = whole.lastIndexOf('\n', offset - 1) + 1
      const indent = whole.slice(lineStart, offset)
      const lineEnd = whole.indexOf('\n', offset + match.length)
      const rest = whole.slice(offset + match.length, lineEnd < 0 ? undefined : lineEnd)
      // A block on its own line becomes a `$$` fence so it renders in display mode.
      if (/^\s*$/.test(indent) && /^\s*$/.test(rest)) {
        return `$$\n${indent}${inner.trim()}\n${indent}$$`
      }
      return `$$${inner.trim()}$$`
    })
    .replace(INLINE, (_match, inner: string) => `$${inner.trim()}$`)
}

function convertOutsideInlineCode(text: string): string {
  let out = ''
  let last = 0
  for (const m of text.matchAll(INLINE_CODE)) {
    out += convertProse(text.slice(last, m.index)) + m[0]
    last = m.index + m[0].length
  }
  return out + convertProse(text.slice(last))
}

/**
 * Rewrite LaTeX `\[…\]` / `\(…\)` delimiters into the `$$…$$` / `$…$` form remark-math parses.
 * Fenced code blocks and inline code spans are left untouched.
 */
export function normalizeMathDelimiters(markdown: string): string {
  if (!markdown.includes('\\[') && !markdown.includes('\\(')) return markdown
  const lines = markdown.split('\n')
  const out: string[] = []
  let prose: string[] = []
  let fence: string | null = null
  const flush = () => {
    if (prose.length) out.push(convertOutsideInlineCode(prose.join('\n')))
    prose = []
  }
  for (const line of lines) {
    const marker = FENCE.exec(line)?.[1]
    if (fence === null) {
      if (marker) {
        flush()
        fence = marker
        out.push(line)
      } else {
        prose.push(line)
      }
    } else {
      out.push(line)
      if (marker && marker[0] === fence[0] && marker.length >= fence.length && line.trim() === marker) {
        fence = null
      }
    }
  }
  flush()
  return out.join('\n')
}
