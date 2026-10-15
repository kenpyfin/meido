import { describe, expect, it } from 'vitest'
import { messageGroupClass, messageGroupPosition } from './message-group'

describe('messageGroupPosition', () => {
  const messages = [
    { role: 'user' },
    { role: 'user' },
    { role: 'assistant' },
    { role: 'assistant' },
    { role: 'assistant' },
    { role: 'user' },
  ]

  it('marks single, start, middle, and end positions', () => {
    expect(messageGroupPosition(messages, 0)).toBe('start')
    expect(messageGroupPosition(messages, 1)).toBe('end')
    expect(messageGroupPosition(messages, 2)).toBe('start')
    expect(messageGroupPosition(messages, 3)).toBe('middle')
    expect(messageGroupPosition(messages, 4)).toBe('end')
    expect(messageGroupPosition(messages, 5)).toBe('single')
  })
})

describe('messageGroupClass', () => {
  it('includes role and position tokens', () => {
    expect(messageGroupClass('middle', 'assistant')).toBe(
      'mc-msg-group mc-msg-group-middle mc-msg-group-assistant',
    )
  })
})
