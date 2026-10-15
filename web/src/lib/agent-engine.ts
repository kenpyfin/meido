export type EngineId = 'classic' | 'classic_cost_routing' | 'gemini_adk' | 'cursor'

export const ENGINE_OPTIONS: { id: EngineId; label: string; subtitle: string }[] = [
  { id: 'classic', label: 'Single turn', subtitle: 'One cloud model — best reasoning continuity' },
  {
    id: 'classic_cost_routing',
    label: 'Classic · Cost routing',
    subtitle: 'Local read-only discovery + delegate sub-jobs',
  },
  {
    id: 'gemini_adk',
    label: 'Gemini ADK',
    subtitle: 'Native multi-agent (coordinator + specialists)',
  },
  { id: 'cursor', label: 'Cursor (SDK)', subtitle: 'Full turn via Cursor sidecar' },
]

export function asEngineId(raw: string | undefined | null): EngineId {
  switch (raw) {
    case 'classic_cost_routing':
    case 'gemini_adk':
    case 'cursor':
      return raw
    case 'deterministic':
    case 'pipeline':
      return 'classic'
    default:
      return 'classic'
  }
}

export function engineLabel(id: EngineId): string {
  return ENGINE_OPTIONS.find((opt) => opt.id === id)?.label ?? id
}
