import { describe, expect, it } from 'vitest'
import { formatReplyForSend, makeReplySnippet, messageTextForClipboard, parseReplyForDisplay } from './reply-quote'

describe('makeReplySnippet', () => {
  it('collapses whitespace and truncates long text', () => {
    const long = 'a'.repeat(250)
    expect(makeReplySnippet(`line one\n\nline two`)).toBe('line one line two')
    expect(makeReplySnippet(long)).toBe(`${'a'.repeat(197)}...`)
  })
})

describe('formatReplyForSend', () => {
  it('wraps full content and appends user follow-up', () => {
    const out = formatReplyForSend(
      {
        messageId: 'msg-1',
        snippet: 'hi',
        fullContent: 'full body\nwith lines',
        senderName: 'bot',
        isFromBot: true,
      },
      'follow up',
    )
    expect(out).toContain('[quoted_message id="msg-1" role="assistant" sender="bot"]')
    expect(out).toContain('full body\nwith lines')
    expect(out).toContain('[/quoted_message]')
    expect(out.endsWith('follow up')).toBe(true)
  })
})

describe('parseReplyForDisplay', () => {
  it('returns snippet and follow-up without full quoted body', () => {
    const body = formatReplyForSend(
      {
        messageId: 'msg-1',
        snippet: 'ignored',
        fullContent: 'full body\nwith lines',
        senderName: 'bot',
        isFromBot: true,
      },
      'follow up',
    )
    const parsed = parseReplyForDisplay(body)
    expect(parsed).not.toBeNull()
    expect(parsed?.quote).toMatchObject({
      messageId: 'msg-1',
      role: 'assistant',
      sender: 'bot',
      snippet: 'full body with lines',
    })
    expect(parsed?.followUp).toBe('follow up')
  })

  it('returns null for plain messages', () => {
    expect(parseReplyForDisplay('hello')).toBeNull()
  })
})

describe('messageTextForClipboard', () => {
  it('formats reply bodies for clipboard without the raw quote block', () => {
    const body = formatReplyForSend(
      {
        messageId: 'msg-1',
        snippet: 'ignored',
        fullContent: 'full body\nwith lines',
        senderName: 'bot',
        isFromBot: true,
      },
      'follow up',
    )
    expect(messageTextForClipboard(body)).toBe('Replying to assistant:\nfull body\nwith lines\n\nfollow up')
  })
})
