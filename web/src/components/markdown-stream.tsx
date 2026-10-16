import { ArtifactLink, artifactId } from './artifact-link'
import React, { Suspense, lazy, memo, useMemo } from 'react'
import { useMessagePartText, INTERNAL } from '@assistant-ui/react'
import remarkGfm from 'remark-gfm'
import { MarkdownTable } from '@/components/markdown-table'
import { cn } from '@/lib/utils'

import 'streamdown/styles.css'

const { withSmoothContextProvider, useSmoothStatus } = INTERNAL

/** Defer the Streamdown runtime (~370KB) until a message actually renders. */
const Streamdown = lazy(() =>
  import('streamdown').then((m) => ({ default: m.Streamdown })),
)

export type MarkdownStreamProps = {
  children?: string
  className?: string
  /** When true, parse incomplete markdown blocks (streaming). */
  streaming?: boolean
  /** Skip images and artifact fetches. Media stays in a separate outputs section. */
  deferMedia?: boolean
}

function MarkdownFallback({ className, children }: { className?: string; children: string }) {
  return (
    <div className={cn('aui-md-root text-sm leading-relaxed whitespace-pre-wrap', className)}>
      {children}
    </div>
  )
}

export function MarkdownStream({
  children = '',
  className,
  streaming = false,
  deferMedia = false,
}: MarkdownStreamProps) {
  return (
    <div className={cn('aui-md-root text-sm leading-relaxed', className)}>
      <Suspense fallback={<MarkdownFallback className={className}>{children}</MarkdownFallback>}>
        <Streamdown
          mode={streaming ? 'streaming' : 'static'}
          remarkPlugins={[remarkGfm]}
          /* Keep default code/table chrome; skip mermaid diagram UI (no plugin installed). */
          controls={{ mermaid: false }}
          components={{
            img: deferMedia
              ? () => null
              : ({ alt, className: imgClass, ...props }) => (
              <img
                {...props}
                alt={alt ?? ''}
                className={cn('my-2 max-h-[70vh] max-w-full rounded-lg', imgClass)}
                loading="lazy"
              />
            ),
            a: (props) => {
              if (props.href?.endsWith('#artifact-unavailable')) {
                return deferMedia
                  ? <span>{props.children}</span>
                  : <span role="status">{props.children} — file unavailable</span>
              }
              const id = artifactId(props.href)
              if (id) return deferMedia ? <span>{props.children}</span> : <ArtifactLink id={id} />
              const mergedRel = [props.rel, 'noopener', 'noreferrer'].filter(Boolean).join(' ')
              return <a {...props} target="_blank" rel={mergedRel} />
            },
            table: MarkdownTable,
          }}
        >
          {children}
        </Streamdown>
      </Suspense>
    </div>
  )
}

/** Assistant-ui message part renderer backed by Streamdown (for thread `Text` slot). */
export const StreamdownMessageText = memo(
  withSmoothContextProvider(function StreamdownMessageText() {
    const part = useMessagePartText()
    const status = useSmoothStatus()
    const streaming = status.type === 'running'
    const text = part.text ?? ''
    const className = useMemo(() => cn(streaming && 'aui-md-running'), [streaming])
    return (
      <MarkdownStream streaming={streaming} className={className}>
        {text}
      </MarkdownStream>
    )
  }),
  () => true,
)
StreamdownMessageText.displayName = 'StreamdownMessageText'
