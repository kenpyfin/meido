export type TierRouteInfo = {
  tier: string
  provider: string
  model: string
  endpoint: string
}

export type PteDecision = {
  iteration: number
  action: string
  durationMs?: number
  reason: string
  providerLabel?: string
  /** llm | heuristic | disabled | error */
  source?: string
}

export type PdqeEvalDetail = {
  verdict?: string
  confidence?: number
  issues?: string[]
  feedback?: string
  note?: string
  reason?: string
  error?: string
  raw?: string
}

export type PdqeStep = {
  at?: string
  step: string
  detail?: string
  eval?: PdqeEvalDetail
  providerLabel?: string
}

export type ParsedAgentHistory = {
  runHeader: string
  /** `Engine: …` line from the run header, when present. */
  engine?: string
  iterations: { index: number; body: string; tier?: TierRouteInfo }[]
  /** Pretty JSON from the server (`initial_llm_request_v1`); null for runs saved before snapshots existed. */
  initialPromptJson: string | null
  pteDecisions: PteDecision[]
  pdqeSteps: PdqeStep[]
}

/** Must match `TierEndpointSnapshot::format_tier_line` / iteration markdown in Rust. */
const TIER_LINE_RE =
  /^Model tier:\s*([^|]+)\|\s*provider:\s*([^|]+)\|\s*model:\s*([^|]+)\|\s*endpoint:\s*(.+)$/m

/** Must match `AgentRunRecord::to_markdown` engine line. */
const ENGINE_LINE_RE = /^Engine:\s*(.+)$/m

export function parseEngineLine(header: string): string | null {
  const match = header.match(ENGINE_LINE_RE)
  const engine = match?.[1]?.trim()
  return engine ? engine : null
}

const PTE_LINE_RE =
  /^- PTE: (?:(disabled)|skipped — (.+)|(\w+)(?: \((\d+)ms\))?(?: — "(.*)")?(?: \[(.+)\])?)$/m

const PDQE_HEADER_RE = /^- \*\*(.+?)\*\* `([^`]+)`(?: \[(.+)\])?(?: — (.+))?$/


export function parseTierLine(body: string): TierRouteInfo | null {
  const match = body.match(TIER_LINE_RE)
  if (!match) return null
  const tier = match[1]?.trim() ?? ''
  const provider = match[2]?.trim() ?? ''
  const model = match[3]?.trim() ?? ''
  const endpoint = match[4]?.trim() ?? ''
  if (!tier) return null
  return { tier, provider, model, endpoint }
}

/** Must match `agent_history::SNAPSHOT_SECTION_START` in Rust. */
export const AGENT_HISTORY_SNAPSHOT_SECTION_MARKER = '\n## Initial LLM prompt (debug snapshot)\n'

/** Must match `agent_history::QUALITY_EVAL_SECTION_START` in Rust. */
export const AGENT_HISTORY_QUALITY_EVAL_SECTION_MARKER = '\n## Post-delivery quality evaluation\n'

export function splitAgentHistoryRaw(content: string): {
  traceMarkdown: string
  qualityEvalMarkdown: string | null
  initialPromptJson: string | null
} {
  const pdqeIdx = content.indexOf(AGENT_HISTORY_QUALITY_EVAL_SECTION_MARKER)
  const snapIdx = content.indexOf(AGENT_HISTORY_SNAPSHOT_SECTION_MARKER)
  let traceEnd = content.length
  if (pdqeIdx >= 0) traceEnd = pdqeIdx
  if (snapIdx >= 0) traceEnd = Math.min(traceEnd, snapIdx)
  const traceMarkdown = content.slice(0, traceEnd).trimEnd()

  let qualityEvalMarkdown: string | null = null
  if (pdqeIdx >= 0) {
    const pdqeEnd = snapIdx >= 0 && snapIdx > pdqeIdx ? snapIdx : content.length
    qualityEvalMarkdown =
      content
        .slice(pdqeIdx + AGENT_HISTORY_QUALITY_EVAL_SECTION_MARKER.length, pdqeEnd)
        .trim() || null
  }

  let initialPromptJson: string | null = null
  if (snapIdx >= 0) {
    initialPromptJson =
      content.slice(snapIdx + AGENT_HISTORY_SNAPSHOT_SECTION_MARKER.length).trim() || null
  }

  return { traceMarkdown, qualityEvalMarkdown, initialPromptJson }
}

function inferPteSource(action: string, providerLabel?: string): string | undefined {
  if (action === 'disabled') return 'disabled'
  if (action === 'skipped') return 'error'
  if (providerLabel === 'heuristic') return 'heuristic'
  if (providerLabel) return 'llm'
  return undefined
}

function parsePteLine(line: string): Omit<PteDecision, 'iteration'> | null {
  const trimmed = line.trim()
  if (trimmed.startsWith('- Hook: PTE synthesis:')) {
    return {
      action: 'complete',
      reason: 'Final response synthesized after PTE complete verdict',
      providerLabel: trimmed.slice('- Hook: PTE synthesis:'.length).trim(),
      source: 'llm',
    }
  }
  const match = trimmed.match(PTE_LINE_RE)
  if (!match) return null
  if (match[1] === 'disabled') {
    return { action: 'disabled', reason: '', source: 'disabled' }
  }
  if (match[2] != null) {
    return { action: 'skipped', reason: match[2], source: 'error' }
  }
  const action = match[3] ?? 'continue'
  const durationMs = match[4] != null ? parseInt(match[4], 10) : undefined
  const reason = match[5] ?? ''
  const providerLabel = match[6]?.trim()
  return {
    action,
    ...(Number.isFinite(durationMs) ? { durationMs } : {}),
    reason,
    ...(providerLabel ? { providerLabel } : {}),
    ...(inferPteSource(action, providerLabel) ? { source: inferPteSource(action, providerLabel) } : {}),
  }
}

/** Parse legacy `confidence=0.92` or structured JSON eval payloads. */
export function parsePdqeEvalDetail(detail?: string): PdqeEvalDetail | null {
  if (!detail?.trim()) return null
  const trimmed = detail.trim()
  if (trimmed.startsWith('{')) {
    try {
      const parsed = JSON.parse(trimmed) as Record<string, unknown>
      const out: PdqeEvalDetail = { raw: trimmed }
      if (typeof parsed.verdict === 'string') out.verdict = parsed.verdict
      if (typeof parsed.confidence === 'number') out.confidence = parsed.confidence
      if (Array.isArray(parsed.issues)) {
        out.issues = parsed.issues.filter((x): x is string => typeof x === 'string')
      }
      if (typeof parsed.feedback === 'string') out.feedback = parsed.feedback
      if (typeof parsed.note === 'string') out.note = parsed.note
      if (typeof parsed.reason === 'string') out.reason = parsed.reason
      if (typeof parsed.error === 'string') out.error = parsed.error
      return out
    } catch {
      return { raw: trimmed }
    }
  }
  const legacy: PdqeEvalDetail = { raw: trimmed }
  const conf = trimmed.match(/confidence=([0-9.]+)/)
  if (conf?.[1]) legacy.confidence = parseFloat(conf[1])
  const retry = trimmed.match(/retry=(\d+\/\d+)/)
  if (retry?.[1]) legacy.note = `retry ${retry[1]}`
  return legacy
}

export function parsePteDecisions(parsed: {
  iterations: { index: number; body: string }[]
}): PteDecision[] {
  const out: PteDecision[] = []
  for (const iter of parsed.iterations) {
    for (const line of iter.body.split('\n')) {
      const pte = parsePteLine(line)
      if (pte) {
        out.push({ iteration: iter.index, ...pte })
      }
    }
  }
  return out
}

export function parsePdqeSteps(qualityEvalMarkdown: string | null): PdqeStep[] {
  if (!qualityEvalMarkdown?.trim()) return []
  const lines = qualityEvalMarkdown.split('\n')
  const out: PdqeStep[] = []
  for (let i = 0; i < lines.length; i++) {
    const trimmed = lines[i]!.trim()
    if (!trimmed.startsWith('- **')) continue
    const match = trimmed.match(PDQE_HEADER_RE)
    if (!match) continue
    const at = match[1]?.trim()
    const step = match[2]?.trim() ?? ''
    const providerLabel = match[3]?.trim()
    let detail = match[4]?.trim()
    const next = lines[i + 1]?.trim()
    if (next?.startsWith('eval: ')) {
      detail = next.slice('eval: '.length).trim()
      i += 1
    }
    const evalDetail = parsePdqeEvalDetail(detail)
    out.push({
      ...(at ? { at } : {}),
      step,
      ...(detail ? { detail } : {}),
      ...(evalDetail ? { eval: evalDetail } : {}),
      ...(providerLabel ? { providerLabel } : {}),
    })
  }
  return out
}

/**
 * Splits persisted agent run markdown (`AgentRunRecord::to_markdown`) into a run header
 * and per-iteration sections (`## Iteration N`). Pass **trace-only** markdown (no snapshot suffix)
 * if you already called `splitAgentHistoryRaw`.
 */
export function parseAgentHistoryMarkdown(traceMarkdown: string): Omit<
  ParsedAgentHistory,
  'initialPromptJson' | 'pteDecisions' | 'pdqeSteps'
> {
  const re = /^## Iteration (\d+)\s*$/gm
  const parts = traceMarkdown.split(re)
  const runHeader = (parts[0] ?? '').trimEnd()
  const iterations: { index: number; body: string; tier?: TierRouteInfo }[] = []
  for (let i = 1; i + 1 < parts.length; i += 2) {
    const idx = parseInt(parts[i]!, 10)
    const body = (parts[i + 1] ?? '').trim()
    if (Number.isFinite(idx)) {
      const tier = parseTierLine(body) ?? undefined
      iterations.push({ index: idx, body, tier })
    }
  }
  const engine = parseEngineLine(runHeader) ?? undefined
  return { runHeader, ...(engine ? { engine } : {}), iterations }
}

/** Compact label for the run-header engine badge. */
export function formatEngineBadgeLabel(engine: string): string {
  const trimmed = engine.trim()
  if (!trimmed) return 'unknown'
  if (trimmed === 'cursor') return 'Cursor'
  if (trimmed === 'classic') return 'Classic'
  if (trimmed.startsWith('classic')) return trimmed
  if (trimmed === 'gemini_adk' || trimmed === 'adk') return 'Gemini ADK'
  if (trimmed === 'deterministic') return 'Classic' // legacy label
  return trimmed
}

/** Compact label for iteration stepper badge. */
export function formatTierBadgeLabel(tier: TierRouteInfo): string {
  let host = tier.endpoint
  try {
    const url = new URL(tier.endpoint)
    host = url.host
  } catch {
    // keep raw endpoint
  }
  return `${tier.tier} · ${tier.model} · ${host}`
}

export function pdqeStepLabel(step: string): string {
  return step
    .replace(/^quality_eval_/, '')
    .replace(/_/g, ' ')
}

export function formatConfidence(confidence?: number): string | null {
  if (confidence == null || !Number.isFinite(confidence)) return null
  return `${Math.round(confidence * 100)}%`
}

export function pdqeStepBadgeKind(step: string, evalDetail?: PdqeEvalDetail): 'pass' | 'fail' | 'skip' | 'neutral' {
  const verdict = evalDetail?.verdict ?? step
  if (verdict.includes('pass') || step.includes('started')) return 'pass'
  if (verdict.includes('fail') || step.includes('retry')) return 'fail'
  if (verdict.includes('skip') || step.includes('skipped') || step.includes('error')) return 'skip'
  return 'neutral'
}
