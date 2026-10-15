export type MentionPersona = { id: number; name: string }

/** Longest persona-name prefix after an @ mention. The host persona is skipped. */
export function matchGuestPersona(
  text: string,
  personas: MentionPersona[],
  hostId: number | null,
): number | null {
  const at = text.search(/(?:^|\s)@/)
  if (at < 0) return null
  const start = text.indexOf('@', at)
  const rest = text.slice(start + 1)
  let best: { id: number; len: number } | null = null
  for (const persona of personas) {
    if (hostId != null && persona.id === hostId) continue
    const name = persona.name.trim()
    if (!name) continue
    if (!rest.toLowerCase().startsWith(name.toLowerCase())) continue
    const boundary = rest[name.length]
    if (boundary != null && !/\s/.test(boundary)) continue
    if (!best || name.length > best.len) best = { id: persona.id, len: name.length }
  }
  return best?.id ?? null
}

/** Incomplete @token at the end of the composer, for autocomplete. */
export function mentionQuery(text: string): string | null {
  const match = text.match(/(?:^|\s)@([^\s@]*)$/)
  return match ? match[1] ?? '' : null
}
