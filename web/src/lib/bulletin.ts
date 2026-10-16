import type { PersonaBulletinHistorySuffix, PersonaHistorySuffixSide } from '../types'

export function parsePersonaBulletinHistorySuffix(raw: unknown): PersonaBulletinHistorySuffix | null {
  if (raw == null || typeof raw !== 'object') return null
  const o = raw as Record<string, unknown>
  const parseSide = (side: unknown): PersonaHistorySuffixSide | null => {
    if (side == null || typeof side !== 'object') return null
    const s = side as Record<string, unknown>
    const effective = typeof s.effective === 'number' ? s.effective : NaN
    const uses_default = typeof s.uses_default === 'boolean' ? s.uses_default : null
    if (!Number.isFinite(effective) || uses_default == null) return null
    const po = s.persona_override
    const persona_override =
      po === null || typeof po === 'number' ? (po as number | null) : null
    return { effective, persona_override, uses_default }
  }
  const min_user = parseSide(o.min_user)
  const min_assistant = parseSide(o.min_assistant)
  const def = o.defaults
  if (min_user == null || min_assistant == null || def == null || typeof def !== 'object') return null
  const dr = def as Record<string, unknown>
  const du = dr.min_user
  const da = dr.min_assistant
  if (typeof du !== 'number' || typeof da !== 'number') return null
  return {
    min_user,
    min_assistant,
    defaults: { min_user: du, min_assistant: da },
  }
}
