export type MentionPersona = { id: number; name: string }

/**
 * The character after a persona name ends the mention when it is absent,
 * whitespace, or punctuation. A capital letter also ends it, so
 * `@Reading_Pal_DeveloperGive` still addresses that persona. A name that
 * contains `_` stays a mention when the next word is glued on in lowercase.
 * A lowercase letter or `_` after a single-word name does not, so `@pepper`
 * does not address `pep`.
 */
function isGuestMentionBoundary(rest: string, name: string): boolean {
  const boundary = rest[name.length]
  if (boundary == null || /\s/u.test(boundary)) return true
  if (/[^\p{L}\p{N}_]/u.test(boundary)) return true
  const startsNextWord = boundary !== boundary.toLowerCase() && boundary === boundary.toUpperCase()
  if (startsNextWord) return true
  return name.includes('_') && /\p{L}/u.test(boundary)
}

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
    if (!isGuestMentionBoundary(rest, name)) continue
    if (!best || name.length > best.len) best = { id: persona.id, len: name.length }
  }
  return best?.id ?? null
}

/** Incomplete @token at the end of the composer, for autocomplete. */
export function mentionQuery(text: string): string | null {
  const match = text.match(/(?:^|\s)@([^\s@]*)$/)
  return match ? match[1] ?? '' : null
}
