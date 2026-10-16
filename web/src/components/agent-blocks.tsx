import React, { useState } from 'react'
import type { ToolCallMessagePartProps } from '@assistant-ui/react'
import {
  formatConfidence,
  formatEngineBadgeLabel,
  formatTierBadgeLabel,
  pdqeStepBadgeKind,
  pdqeStepLabel,
  type PdqeEvalDetail,
  type PdqeStep,
  type PteDecision,
  type TierRouteInfo,
} from '@/parse-agent-history'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { cn } from '@/lib/utils'

function asObject(value: unknown): Record<string, unknown> {
  if (typeof value === 'object' && value !== null && !Array.isArray(value)) {
    return value as Record<string, unknown>
  }
  return {}
}

function formatUnknown(value: unknown): string {
  if (typeof value === 'string') return value
  try {
    return JSON.stringify(value, null, 2)
  } catch {
    return String(value)
  }
}

function pteSourceLabel(source?: string): string | null {
  switch (source) {
    case 'llm':
      return 'LLM evaluation'
    case 'heuristic':
      return 'Heuristic stall detector'
    case 'disabled':
      return 'Disabled'
    case 'error':
      return 'Skipped / error'
    default:
      return null
  }
}

function pteBadgeVariant(action: string): 'default' | 'secondary' | 'destructive' | 'outline' {
  if (action === 'complete') return 'default'
  if (action === 'disabled' || action === 'skipped') return 'secondary'
  return 'outline'
}

function tierBadgeClass(tier: string): string {
  switch (tier) {
    case 'technical':
      return 'border-[color:var(--mc-accent-primary)]/35 bg-[color:var(--mc-accent-primary)]/10 text-[color:var(--mc-accent-primary)]'
    case 'knowledge':
      return 'border-[color:var(--mc-accent-secondary)]/35 bg-[color:var(--mc-accent-secondary)]/10 text-[color:var(--mc-text-secondary)]'
    default:
      return 'border-[color:var(--mc-border-strong)] bg-[color:var(--mc-surface-elevated)] text-[color:var(--mc-text-primary)]'
  }
}

function engineBadgeClass(engine: string): string {
  const key = engine.trim().split(/\s+/)[0] ?? ''
  switch (key) {
    case 'cursor':
      return 'border-[color:var(--mc-accent-primary)]/35 bg-[color:var(--mc-accent-primary)]/10 text-[color:var(--mc-accent-primary)]'
    case 'classic':
      return 'border-[color:var(--mc-accent-secondary)]/35 bg-[color:var(--mc-accent-secondary)]/10 text-[color:var(--mc-text-secondary)]'
    default:
      return 'border-[color:var(--mc-border-strong)] bg-[color:var(--mc-surface-elevated)] text-[color:var(--mc-text-primary)]'
  }
}

function CollapsibleBlock({
  title,
  badge,
  defaultOpen = false,
  children,
  className,
}: {
  title: React.ReactNode
  badge?: React.ReactNode
  defaultOpen?: boolean
  children: React.ReactNode
  className?: string
}) {
  const [open, setOpen] = useState(defaultOpen)
  return (
    <Card className={cn('gap-0 py-3 shadow-none', className)}>
      <CardHeader className="flex flex-row items-center justify-between gap-2 space-y-0 px-3 pb-2">
        <CardTitle className="text-sm font-medium">{title}</CardTitle>
        <div className="flex items-center gap-2">
          {badge}
          <Button type="button" variant="ghost" size="sm" className="h-7 px-2" onClick={() => setOpen((v) => !v)}>
            {open ? 'Hide' : 'Show'}
          </Button>
        </div>
      </CardHeader>
      {open ? <CardContent className="px-3 pt-0">{children}</CardContent> : null}
    </Card>
  )
}

export function AgentToolCallBlock(props: ToolCallMessagePartProps) {
  const result = asObject(props.result)
  const hasResult = Object.keys(result).length > 0
  const output = result.output
  const duration = result.duration_ms
  const bytes = result.bytes
  const statusCode = result.status_code
  const errorType = result.error_type
  const state = hasResult ? (props.isError ? 'error' : 'done') : 'running'

  return (
    <CollapsibleBlock
      title={props.toolName}
      badge={
        <Badge
          variant={state === 'error' ? 'destructive' : state === 'running' ? 'secondary' : 'outline'}
        >
          {state}
        </Badge>
      }
      className="tool-card border-[color:var(--mc-border-soft)] bg-[color:var(--mc-surface-elevated)]"
    >
      {Object.keys(props.args || {}).length > 0 ? (
        <pre className="tool-card-pre mb-2 overflow-x-auto rounded-md bg-muted/40 p-2 text-xs">
          {JSON.stringify(props.args, null, 2)}
        </pre>
      ) : null}
      {hasResult ? (
        <div className="tool-card-meta mb-2 flex flex-wrap gap-2 text-xs text-muted-foreground">
          {typeof duration === 'number' ? <span>{duration}ms</span> : null}
          {typeof bytes === 'number' ? <span>{bytes}b</span> : null}
          {typeof statusCode === 'number' ? <span>HTTP {statusCode}</span> : null}
          {typeof errorType === 'string' && errorType ? <span>{errorType}</span> : null}
        </div>
      ) : null}
      {output !== undefined ? (
        <pre className="tool-card-pre overflow-x-auto rounded-md bg-muted/40 p-2 text-xs">{formatUnknown(output)}</pre>
      ) : null}
    </CollapsibleBlock>
  )
}

export function PteDecisionBlock({ row }: { row: PteDecision }) {
  const sourceLabel = pteSourceLabel(row.source)
  return (
    <Card className="mc-eval-row gap-2 py-3 shadow-none">
      <CardHeader className="flex flex-row flex-wrap items-start justify-between gap-2 space-y-0 px-3 pb-1">
        <CardTitle className="text-sm">Iteration {row.iteration}</CardTitle>
        <Badge variant={pteBadgeVariant(row.action)}>{row.action}</Badge>
      </CardHeader>
      <CardContent className="space-y-2 px-3 pt-0 text-sm">
        {sourceLabel ? (
          <p className="text-xs text-muted-foreground">
            {sourceLabel}
            {row.durationMs != null ? ` · ${row.durationMs}ms` : ''}
          </p>
        ) : row.durationMs != null ? (
          <p className="text-xs text-muted-foreground">{row.durationMs}ms</p>
        ) : null}
        {row.reason ? (
          <div>
            <p className="mb-1 text-xs font-medium text-muted-foreground">Rationale</p>
            <p className="whitespace-pre-wrap">{row.reason}</p>
          </div>
        ) : null}
        {row.providerLabel && row.providerLabel !== 'heuristic' ? (
          <p className="text-xs text-muted-foreground">{row.providerLabel}</p>
        ) : null}
      </CardContent>
    </Card>
  )
}

export function PdqeEvalDetailBlock({ evalDetail }: { evalDetail: PdqeEvalDetail }) {
  const confidenceLabel = formatConfidence(evalDetail.confidence)
  return (
    <div className="mt-2 space-y-2 text-sm">
      {evalDetail.verdict ? (
        <p className="text-xs text-muted-foreground">
          Verdict: <span className="font-medium text-foreground">{evalDetail.verdict}</span>
          {confidenceLabel ? ` · confidence ${confidenceLabel}` : ''}
        </p>
      ) : confidenceLabel ? (
        <p className="text-xs text-muted-foreground">
          Confidence: <span className="font-medium text-foreground">{confidenceLabel}</span>
        </p>
      ) : null}
      {evalDetail.note ? <p className="text-xs text-muted-foreground">{evalDetail.note}</p> : null}
      {evalDetail.reason ? <p>Skip reason: {evalDetail.reason}</p> : null}
      {evalDetail.error ? <p className="text-orange-600 dark:text-orange-400">{evalDetail.error}</p> : null}
      {evalDetail.issues && evalDetail.issues.length > 0 ? (
        <div>
          <p className="mb-1 text-xs font-medium">Issues</p>
          <ul className="mc-eval-issues list-disc pl-4">
            {evalDetail.issues.map((issue, idx) => (
              <li key={idx}>{issue}</li>
            ))}
          </ul>
        </div>
      ) : null}
      {evalDetail.feedback ? (
        <div className="mc-eval-feedback rounded-md border border-[color:var(--mc-border-soft)] bg-[color:var(--mc-surface-elevated)] p-2">
          <p className="mb-1 text-xs font-medium">Evaluator feedback</p>
          <p className="whitespace-pre-wrap">{evalDetail.feedback}</p>
        </div>
      ) : null}
    </div>
  )
}

export function PdqeStepBlock({ step }: { step: PdqeStep }) {
  const kind = pdqeStepBadgeKind(step.step, step.eval)
  const title = pdqeStepLabel(step.step)
  return (
    <Card className="mc-eval-row gap-2 py-3 shadow-none">
      <CardHeader className="flex flex-row flex-wrap items-start justify-between gap-2 space-y-0 px-3 pb-1">
        <CardTitle className="text-sm">{title}</CardTitle>
        <span className={`mc-eval-badge mc-eval-badge--${kind}`}>{step.eval?.verdict ?? kind}</span>
      </CardHeader>
      <CardContent className="px-3 pt-0 text-sm">
        {step.at ? <p className="text-xs text-muted-foreground">{step.at}</p> : null}
        {step.providerLabel ? (
          <p className="text-xs text-muted-foreground">{step.providerLabel}</p>
        ) : null}
        {step.eval ? (
          <PdqeEvalDetailBlock evalDetail={step.eval} />
        ) : step.detail ? (
          <p className="mt-2 whitespace-pre-wrap">{step.detail}</p>
        ) : null}
      </CardContent>
    </Card>
  )
}

export function AgentHistoryEngineBadge({ engine }: { engine: string }) {
  return (
    <Badge
      variant="outline"
      className={cn('max-w-full font-mono text-[11px] leading-snug', engineBadgeClass(engine))}
      title={engine}
    >
      Engine · {formatEngineBadgeLabel(engine)}
    </Badge>
  )
}

export function AgentHistoryTierBadge({ tier }: { tier: TierRouteInfo }) {
  return (
    <Badge
      variant="outline"
      className={cn('max-w-full font-mono text-[11px] leading-snug', tierBadgeClass(tier.tier))}
      title={`${tier.provider} · ${tier.endpoint}`}
    >
      {formatTierBadgeLabel(tier)}
    </Badge>
  )
}
