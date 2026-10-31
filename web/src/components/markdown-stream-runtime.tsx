import React, { isValidElement, useEffect, useRef, useState } from 'react'
import { CheckIcon, CopyIcon, DownloadIcon } from 'lucide-react'
import { toast } from 'sonner'
import { CodeBlock, Streamdown, useIsCodeFenceIncomplete, type ExtraProps, type StreamdownProps } from 'streamdown'
import { createMathPlugin } from '@streamdown/math'
import { copyTextToClipboard } from '@/lib/copy-to-clipboard'
import { codeFileExtension, downloadTextFile } from '@/lib/download-file'
import { cn } from '@/lib/utils'

import 'katex/dist/katex.min.css'

const plugins = { math: createMathPlugin({ singleDollarTextMath: true }) }

const actionButtonClass =
  'cursor-pointer p-1 text-muted-foreground transition-all hover:text-foreground'

/** Copy via the shared helper: falls back to execCommand when the page is not a secure context. */
function CodeCopyButton({ code }: { code: string }) {
  const [copied, setCopied] = useState(false)
  const timer = useRef(0)
  useEffect(() => () => window.clearTimeout(timer.current), [])
  const onClick = () => {
    void copyTextToClipboard(code, { trim: false }).then((ok) => {
      if (!ok) {
        toast.error('Copy failed')
        return
      }
      setCopied(true)
      window.clearTimeout(timer.current)
      timer.current = window.setTimeout(() => setCopied(false), 2000)
    })
  }
  const Icon = copied ? CheckIcon : CopyIcon
  return (
    <button
      aria-label="Copy code"
      className={actionButtonClass}
      data-streamdown="code-block-copy-button"
      onClick={onClick}
      title="Copy code"
      type="button"
    >
      <Icon aria-hidden="true" size={14} />
    </button>
  )
}

function CodeDownloadButton({ code, language }: { code: string; language: string }) {
  return (
    <button
      aria-label="Download file"
      className={actionButtonClass}
      data-streamdown="code-block-download-button"
      onClick={() => downloadTextFile(`file.${codeFileExtension(language)}`, code)}
      title="Download file"
      type="button"
    >
      <DownloadIcon aria-hidden="true" size={14} />
    </button>
  )
}

function codeText(children: React.ReactNode): string {
  if (typeof children === 'string') return children
  if (isValidElement<{ children?: unknown }>(children) && typeof children.props.children === 'string') {
    return children.props.children
  }
  return ''
}

/** Replaces Streamdown's code renderer so the block actions use our copy/download helpers. */
function MarkdownCode({
  node,
  className,
  children,
  ...props
}: React.ComponentProps<'code'> & ExtraProps) {
  const isIncomplete = useIsCodeFenceIncomplete()
  if (!('data-block' in props)) {
    return (
      <code
        className={cn('rounded bg-muted px-1.5 py-0.5 font-mono text-sm', className)}
        data-streamdown="inline-code"
        {...props}
      >
        {children}
      </code>
    )
  }
  const language = className?.match(/language-([^\s]+)/)?.[1] ?? ''
  const meta = node?.properties?.metastring
  const metastring = typeof meta === 'string' ? meta : ''
  const startLine = Number.parseInt(metastring.match(/startLine=(\d+)/)?.[1] ?? '', 10)
  const code = codeText(children)
  return (
    <CodeBlock
      className={className}
      code={code}
      isIncomplete={isIncomplete}
      language={language}
      lineNumbers={!/\bnoLineNumbers\b/.test(metastring)}
      startLine={startLine >= 1 ? startLine : undefined}
    >
      <CodeDownloadButton code={code} language={language} />
      <CodeCopyButton code={code} />
    </CodeBlock>
  )
}

/** Streamdown with KaTeX math and our code-block actions; loaded lazily by MarkdownStream. */
export default function StreamdownWithPlugins({ components, ...props }: StreamdownProps) {
  return <Streamdown {...props} components={{ ...components, code: MarkdownCode }} plugins={plugins} />
}
