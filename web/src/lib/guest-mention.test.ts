import { describe, expect, it } from 'vitest'
import {
  GUEST_MAX_PER_TURN,
  guestTurnNotice,
  matchGuestPersona,
  mentionQuery,
  parseGuestMentions,
} from './guest-mention'

const personas = [
  { id: 1, name: 'pep' },
  { id: 3, name: 'Reading' },
  { id: 7, name: 'trading_developer' },
  { id: 18, name: 'Reading_Pal_Developer' },
]

describe('matchGuestPersona', () => {
  it('matches a handle followed by a space', () => {
    expect(matchGuestPersona('@Reading_Pal_Developer Give me the summary', personas, 1)).toBe(18)
  })

  it('matches a compound handle glued to the next word', () => {
    expect(matchGuestPersona('@Reading_Pal_DeveloperGive me the summary', personas, 1)).toBe(18)
    expect(matchGuestPersona('@Reading_Pal_Developergive me the summary', personas, 1)).toBe(18)
  })

  it('matches the longest handle and skips the host', () => {
    expect(matchGuestPersona('@Reading tell me', personas, 1)).toBe(3)
    expect(matchGuestPersona('@pep hello', personas, 1)).toBeNull()
    expect(matchGuestPersona('@pepper', personas, null)).toBeNull()
  })

  it('matches punctuation and ignores an @ inside a word', () => {
    expect(matchGuestPersona('ask @Reading_Pal_Developer, please', personas, 1)).toBe(18)
    expect(matchGuestPersona('mail user@Reading_Pal_Developer', personas, 1)).toBeNull()
  })
})

describe('parseGuestMentions', () => {
  it('keeps two tags in the order they were typed and highlights both handles', () => {
    const text = '@Reading_Pal_Developer then @trading_developer look at this'
    const parsed = parseGuestMentions(text, personas, 1)
    expect(parsed.speakers.map((speaker) => speaker.id)).toEqual([18, 7])
    expect(parsed.ranges).toEqual([
      { start: 0, end: '@Reading_Pal_Developer'.length, personaId: 18 },
      {
        start: text.indexOf('@trading_developer'),
        end: text.indexOf('@trading_developer') + '@trading_developer'.length,
        personaId: 7,
      },
    ])
  })

  it('dedupes a repeated tag and skips the host', () => {
    const parsed = parseGuestMentions('@Reading @pep @Reading again', personas, 1)
    expect(parsed.speakers).toEqual([{ id: 3, name: 'Reading' }])
    expect(parsed.ranges).toHaveLength(2)
  })

  it('highlights a glued compound handle without the following word', () => {
    const text = '@Reading_Pal_DeveloperGive me'
    const parsed = parseGuestMentions(text, personas, 1)
    expect(parsed.ranges).toEqual([
      { start: 0, end: '@Reading_Pal_Developer'.length, personaId: 18 },
    ])
    expect(text.slice(parsed.ranges[0].start, parsed.ranges[0].end)).toBe('@Reading_Pal_Developer')
  })
})

describe('guestTurnNotice', () => {
  it('names the turn order and says the current persona stays silent', () => {
    expect(
      guestTurnNotice(
        [
          { id: 18, name: 'Reading_Pal_Developer' },
          { id: 7, name: 'trading_developer' },
        ],
        'pep',
      ),
    ).toBe(
      'Reading_Pal_Developer, then trading_developer will answer in turn. pep will not answer.',
    )
    expect(guestTurnNotice([], 'pep')).toBeNull()
  })

  it('uses the same cap the server enforces', () => {
    expect(GUEST_MAX_PER_TURN).toBe(4)
  })
})

describe('mentionQuery', () => {
  it('returns the incomplete token at the end of the composer', () => {
    expect(mentionQuery('hello @Read')).toBe('Read')
    expect(mentionQuery('hello')).toBeNull()
  })
})
