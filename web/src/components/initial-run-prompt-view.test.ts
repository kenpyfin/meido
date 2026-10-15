import { describe, expect, it } from 'vitest'
import {
  formatMessagesTranscript,
  messageSectionLabel,
  snapshotHasPersonaContext,
} from './initial-run-prompt-view'

describe('formatMessagesTranscript', () => {
  it('formats all messages in one transcript with section labels', () => {
    const transcript = formatMessagesTranscript([
      { role: 'user', content: '[system_runtime_context]now[/system_runtime_context]' },
      { role: 'assistant', content: 'Acknowledged runtime context.' },
      {
        role: 'user',
        content:
          '[persona_context]\n## Memory\n\n- fact\n\n## Operator focus\n\nbe brief\n[/persona_context]',
      },
      { role: 'assistant', content: 'Acknowledged persona context.' },
      { role: 'user', content: '<user_message sender="alice">hi</user_message>' },
    ])
    expect(transcript).toContain('Message 1 · user · runtime context')
    expect(transcript).toContain('Message 3 · user · persona context')
    expect(transcript).toContain('Tier 2/3')
    expect(transcript).not.toContain('### Identity')
    expect(transcript).toContain('## Memory')
    expect(transcript).toContain('## Operator focus')
    expect(transcript).toContain('Message 5 · user · chat (user)')
    expect(snapshotHasPersonaContext([{ role: 'user', content: '[persona_context]x[/persona_context]' }])).toBe(
      true,
    )
  })

  it('labels persona context section', () => {
    expect(
      messageSectionLabel('[persona_context]body[/persona_context]', 'user'),
    ).toContain('persona context')
  })
})
