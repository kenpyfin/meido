export type MentionPersona = { id: number; name: string }

export type GuestMentionRange = { start: number; end: number; personaId: number }

export type GuestSpeaker = { id: number; name: string }

export type ParsedGuestMentions = {
  ranges: GuestMentionRange[]
  speakers: GuestSpeaker[]
}

/** Matches `GUEST_MAX_PER_TURN` in `src/collab/mod.rs`. */
export const GUEST_MAX_PER_TURN = 4

export const GUEST_TURN_CAP_NOTICE = `Tag at most ${GUEST_MAX_PER_TURN} personas.`

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

function bestMention(
  rest: string,
  personas: MentionPersona[],
  hostId: number | null,
): { id: number; name: string; len: number } | null {
  let best: { id: number; name: string; len: number } | null = null
  for (const persona of personas) {
    if (hostId != null && persona.id === hostId) continue
    const name = persona.name.trim()
    if (!name) continue
    if (!rest.toLowerCase().startsWith(name.toLowerCase())) continue
    if (!isGuestMentionBoundary(rest, name)) continue
    if (!best || name.length > best.len) best = { id: persona.id, name, len: name.length }
  }
  return best
}

/** Every matched @persona in order. Speakers are deduped; the host is skipped. */
export function parseGuestMentions(
  text: string,
  personas: MentionPersona[],
  hostId: number | null,
): ParsedGuestMentions {
  const ranges: GuestMentionRange[] = []
  const speakers: GuestSpeaker[] = []
  const seen = new Set<number>()
  const re = /(?:^|\s)@/g
  let match: RegExpExecArray | null
  while ((match = re.exec(text)) !== null) {
    const at = match.index + match[0].length - 1
    const rest = text.slice(at + 1)
    const found = bestMention(rest, personas, hostId)
    if (!found) continue
    ranges.push({ start: at, end: at + 1 + found.len, personaId: found.id })
    if (!seen.has(found.id)) {
      seen.add(found.id)
      speakers.push({ id: found.id, name: found.name })
    }
    re.lastIndex = at + 1 + found.len
  }
  return { ranges, speakers }
}

/** First tagged persona, if any. The host persona is skipped. */
export function matchGuestPersona(
  text: string,
  personas: MentionPersona[],
  hostId: number | null,
): number | null {
  return parseGuestMentions(text, personas, hostId).speakers[0]?.id ?? null
}

/** Line above the composer when one or more personas are tagged. */
export function guestTurnNotice(speakers: GuestSpeaker[], hostName: string | null): string | null {
  if (speakers.length === 0) return null
  const names = speakers.map((speaker) => speaker.name)
  const order =
    names.length === 1
      ? `${names[0]} will answer.`
      : `${names.slice(0, -1).join(', ')}, then ${names[names.length - 1]} will answer in turn.`
  const host = hostName?.trim() ? hostName.trim() : 'The current persona'
  return `${order} ${host} will not answer.`
}

/** Incomplete @token at the end of the composer, for autocomplete. */
export function mentionQuery(text: string): string | null {
  const match = text.match(/(?:^|\s)@([^\s@]*)$/)
  return match ? match[1] ?? '' : null
}
