import { describe, expect, it } from 'vitest'
import { matchGuestPersona, mentionQuery } from './guest-mention'

const personas = [
  { id: 1, name: 'pep' },
  { id: 3, name: 'Reading' },
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

describe('mentionQuery', () => {
  it('returns the incomplete token at the end of the composer', () => {
    expect(mentionQuery('hello @Read')).toBe('Read')
    expect(mentionQuery('hello')).toBeNull()
  })
})
