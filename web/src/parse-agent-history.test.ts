import { describe, expect, it } from 'vitest'
import {
  formatConfidence,
  formatEngineBadgeLabel,
  formatTierBadgeLabel,
  parseAgentHistoryMarkdown,
  parseEngineLine,
  parsePdqeEvalDetail,
  parsePdqeSteps,
  parsePteDecisions,
  parseTierLine,
  splitAgentHistoryRaw,
} from './parse-agent-history'

describe('parseTierLine', () => {
  it('parses stable tier line from iteration body', () => {
    const body = `Stop: tool_use
Model tier: technical | provider: llama | model: qwen-test | endpoint: http://127.0.0.1:8080/v1
- Tool: bash (12ms) OK`
    expect(parseTierLine(body)).toEqual({
      tier: 'technical',
      provider: 'llama',
      model: 'qwen-test',
      endpoint: 'http://127.0.0.1:8080/v1',
    })
  })

  it('returns null for legacy runs without tier line', () => {
    const body = `Stop: end_turn
Assistant: "hello"`
    expect(parseTierLine(body)).toBeNull()
  })
})

describe('parseEngineLine', () => {
  it('reads the Engine line from a run header', () => {
    const header = `# Run 2026-08-31
Channel: web | User: "retry the fill"
Engine: cursor
Total: 1 iteration(s)`
    expect(parseEngineLine(header)).toBe('cursor')
    expect(formatEngineBadgeLabel('cursor')).toBe('Cursor')
  })

  it('returns null when the header has no Engine line', () => {
    expect(parseEngineLine('# Run legacy\nTotal: 1')).toBeNull()
  })
})

describe('parseAgentHistoryMarkdown', () => {
  it('extracts engine from the run header', () => {
    const md = `# Run 2026-08-31
Engine: cursor
Cursor sidecar: composer-2.5 @ http://127.0.0.1:3848

## Iteration 1
Stop: tool_use`
    const parsed = parseAgentHistoryMarkdown(md)
    expect(parsed.engine).toBe('cursor')
  })

  it('extracts tier from iterations when present', () => {
    const md = `# Run 2026-06-17
Multi-model: enabled
- Tier 1 (technical): qwen @ http://127.0.0.1:8080/v1

## Iteration 1
Stop: tool_use
Model tier: knowledge | provider: llama | model: mistral | endpoint: http://127.0.0.1:8081/v1
- Tool: search_vault (5ms) OK`
    const parsed = parseAgentHistoryMarkdown(md)
    expect(parsed.iterations).toHaveLength(1)
    expect(parsed.iterations[0]?.tier).toEqual({
      tier: 'knowledge',
      provider: 'llama',
      model: 'mistral',
      endpoint: 'http://127.0.0.1:8081/v1',
    })
  })

  it('parses legacy markdown without tier fields', () => {
    const md = `# Run legacy
## Iteration 1
Stop: end_turn
Assistant: "hi"`
    const parsed = parseAgentHistoryMarkdown(md)
    expect(parsed.iterations[0]?.tier).toBeUndefined()
  })
})

describe('splitAgentHistoryRaw', () => {
  it('splits trace, PDQE section, and snapshot', () => {
    const content = `# Run
## Iteration 1
Stop: tool_use
- PTE: continue (842ms) — "partial" [local · qwen @ http://127.0.0.1:8080/v1]

## Post-delivery quality evaluation
- **2026-06-26 19:39:29 UTC** \`quality_eval_pass\` — confidence=0.92 [local · qwen @ http://127.0.0.1:8080/v1]

## Initial LLM prompt (debug snapshot)
{"schema":"initial_llm_request_v1"}`
    const split = splitAgentHistoryRaw(content)
    expect(split.traceMarkdown).toContain('- PTE: continue')
    expect(split.qualityEvalMarkdown).toContain('quality_eval_pass')
    expect(split.initialPromptJson).toContain('initial_llm_request_v1')
  })
})

describe('parsePteDecisions', () => {
  it('extracts PTE lines from iteration bodies', () => {
    const md = `## Iteration 1
Stop: tool_use
- PTE: continue (842ms) — "tool results partial" [local · qwen @ http://127.0.0.1:8080/v1]`
    const parsed = parseAgentHistoryMarkdown(md)
    const pte = parsePteDecisions(parsed)
    expect(pte).toHaveLength(1)
    expect(pte[0]?.action).toBe('continue')
    expect(pte[0]?.durationMs).toBe(842)
    expect(pte[0]?.reason).toBe('tool results partial')
  })
})

describe('parsePdqeSteps', () => {
  it('parses PDQE timeline bullets with structured eval JSON', () => {
    const md = `- **2026-06-26 19:39:29 UTC** \`quality_eval_started\`
- **2026-06-26 19:39:31 UTC** \`quality_eval_fail\` [local · qwen @ http://127.0.0.1:8080/v1]
  eval: {"verdict":"fail","confidence":0.95,"issues":["incomplete reply"],"feedback":"Add the missing deployment steps."}`
    const steps = parsePdqeSteps(md)
    expect(steps).toHaveLength(2)
    expect(steps[1]?.eval?.verdict).toBe('fail')
    expect(steps[1]?.eval?.issues).toEqual(['incomplete reply'])
    expect(steps[1]?.eval?.feedback).toContain('deployment steps')
    expect(formatConfidence(steps[1]?.eval?.confidence)).toBe('95%')
  })

  it('parses legacy confidence-only detail lines', () => {
    const detail = parsePdqeEvalDetail('retry=1/1 confidence=0.95')
    expect(detail?.confidence).toBe(0.95)
    expect(detail?.note).toBe('retry 1/1')
  })
})

describe('formatTierBadgeLabel', () => {
  it('shortens endpoint to host', () => {
    expect(
      formatTierBadgeLabel({
        tier: 'technical',
        provider: 'llama',
        model: 'qwen-test',
        endpoint: 'http://127.0.0.1:8080/v1',
      }),
    ).toBe('technical · qwen-test · 127.0.0.1:8080')
  })
})
