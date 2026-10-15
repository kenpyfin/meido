/** Visible snippet length in the composer quote chip (full text is still sent to the agent). */
export const REPLY_SNIPPET_MAX = 200

export type PendingReplyQuote = {
  messageId: string
  snippet: string
  fullContent: string
  senderName: string
  isFromBot: boolean
}

export function makeReplySnippet(text: string, max = REPLY_SNIPPET_MAX): string {
  const oneLine = text.replace(/\s+/g, ' ').trim()
  if (!oneLine) return ''
  if (oneLine.length <= max) return oneLine
  return `${oneLine.slice(0, max - 3)}...`
}

function escapeQuotedAttr(value: string): string {
  return value.replace(/\\/g, '\\\\').replace(/"/g, '\\"')
}

function unescapeQuotedAttr(value: string): string {
  return value.replace(/\\"/g, '"').replace(/\\\\/g, '\\')
}

const QUOTED_MESSAGE_OPEN_RE =
  /^\[quoted_message id="((?:\\.|[^"\\])*)" role="(assistant|user)" sender="((?:\\.|[^"\\])*)"\]\n/

export type DisplayReplyQuote = {
  messageId: string
  role: 'user' | 'assistant'
  sender: string
  snippet: string
}

export type ParsedReplyMessage = {
  quote: DisplayReplyQuote
  followUp: string
}

/** Parse a sent reply body for bubble display (snippet + follow-up only). */
export function parseReplyForDisplay(text: string): ParsedReplyMessage | null {
  const openMatch = text.match(QUOTED_MESSAGE_OPEN_RE)
  if (!openMatch) return null

  const afterOpen = text.slice(openMatch[0].length)
  const closeMarker = '\n[/quoted_message]'
  const closeIdx = afterOpen.indexOf(closeMarker)
  if (closeIdx < 0) return null

  const quotedContent = afterOpen.slice(0, closeIdx)
  let followUp = afterOpen.slice(closeIdx + closeMarker.length)
  if (followUp.startsWith('\n\n')) {
    followUp = followUp.slice(2)
  } else if (followUp === '\n') {
    followUp = ''
  }

  return {
    quote: {
      messageId: unescapeQuotedAttr(openMatch[1]),
      role: openMatch[2] as 'user' | 'assistant',
      sender: unescapeQuotedAttr(openMatch[3]),
      snippet: makeReplySnippet(quotedContent),
    },
    followUp,
  }
}

/** Compose the user message body sent to `/api/send_stream` (full quote + optional follow-up). */
export function formatReplyForSend(quote: PendingReplyQuote, userText: string): string {
  const role = quote.isFromBot ? 'assistant' : 'user'
  const sender = quote.senderName.trim() || role
  let block = `[quoted_message id="${escapeQuotedAttr(quote.messageId)}" role="${role}" sender="${escapeQuotedAttr(sender)}"]\n`
  block += quote.fullContent.trim()
  block += '\n[/quoted_message]'
  const trimmed = userText.trim()
  if (trimmed) {
    return `${block}\n\n${trimmed}`
  }
  return block
}

/** Human-readable clipboard text for reply messages (full quote + follow-up, not raw block). */
export function messageTextForClipboard(text: string): string {
  const parsed = parseReplyForDisplay(text)
  if (!parsed) return text

  const openMatch = text.match(QUOTED_MESSAGE_OPEN_RE)
  if (!openMatch) return text

  const afterOpen = text.slice(openMatch[0].length)
  const closeMarker = '\n[/quoted_message]'
  const closeIdx = afterOpen.indexOf(closeMarker)
  if (closeIdx < 0) return text

  const quotedContent = afterOpen.slice(0, closeIdx).trim()
  const label = parsed.quote.role === 'assistant'
    ? 'assistant'
    : (parsed.quote.sender.trim() || 'user')
  let out = `Replying to ${label}:\n${quotedContent}`
  if (parsed.followUp.trim()) {
    out += `\n\n${parsed.followUp.trim()}`
  }
  return out
}
