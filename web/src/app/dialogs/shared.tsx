import React from 'react'
import remarkGfm from 'remark-gfm'
import ReactMarkdown from 'react-markdown'
import { MarkdownTable } from '../../components/markdown-table'
import { PdqeStepBlock, PteDecisionBlock } from '../../components/agent-blocks'
import type { ParsedAgentHistory } from '../../parse-agent-history'
import type { ArtifactItem } from '../../types'

export { AgentHistoryEngineBadge, AgentHistoryTierBadge } from '../../components/agent-blocks'

export function formatBytes(value: number | null | undefined): string {
  if (typeof value !== 'number' || !Number.isFinite(value)) return 'unknown size'
  if (value < 1024) return `${value} B`
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`
  return `${(value / (1024 * 1024)).toFixed(1)} MB`
}

export function artifactPreviewUrl(item: ArtifactItem): string {
  if (item.kind === 'html') return item.preview_url || `${item.url}?preview=1`
  return item.url
}

function MarkdownExternalLink(props: React.ComponentPropsWithoutRef<'a'>) {
  const mergedRel = [props.rel, 'noopener', 'noreferrer'].filter(Boolean).join(' ')
  return <a {...props} target="_blank" rel={mergedRel} />
}

export function AgentHistoryMarkdownBody({ markdown }: { markdown: string }) {
  return (
    <div className="aui-md-root text-sm leading-relaxed">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          a: (props) => <MarkdownExternalLink {...props} />,
          table: MarkdownTable,
        }}
      >
        {markdown}
      </ReactMarkdown>
    </div>
  )
}

export function AgentHistoryEvaluatorsPanel({ parsed }: { parsed: ParsedAgentHistory }) {
  const pte = parsed.pteDecisions ?? []
  const pdqe = parsed.pdqeSteps ?? []

  return (
    <div className="flex flex-col gap-4">
      <div>
        <span className="mb-2 block text-sm font-medium">Post-tool evaluator (PTE)</span>
        {pte.length === 0 ? (
          <p className="text-sm text-muted-foreground">PTE disabled or no tool iterations evaluated.</p>
        ) : (
          <div className="flex flex-col gap-2">
            {pte.map((row, i) => (
              <PteDecisionBlock key={`pte-${row.iteration}-${i}`} row={row} />
            ))}
          </div>
        )}
      </div>
      <div>
        <span className="mb-2 block text-sm font-medium">Pre-delivery quality (PDQE)</span>
        {pdqe.length === 0 ? (
          <p className="text-sm text-muted-foreground">
            No PDQE steps recorded for this run (evaluator disabled, skipped, or run saved before this feature).
          </p>
        ) : (
          <div className="flex flex-col gap-2">
            {pdqe.map((step, i) => (
              <PdqeStepBlock key={`pdqe-${step.step}-${i}`} step={step} />
            ))}
          </div>
        )}
      </div>
    </div>
  )
}
