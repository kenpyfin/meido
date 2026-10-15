export type MessageGroupPosition = 'single' | 'start' | 'middle' | 'end'

export function messageGroupPosition(
  messages: ReadonlyArray<{ role: string }>,
  index: number,
): MessageGroupPosition {
  if (index < 0 || index >= messages.length) return 'single'

  const role = messages[index]?.role
  const prevSame = index > 0 && messages[index - 1]?.role === role
  const nextSame = index < messages.length - 1 && messages[index + 1]?.role === role

  if (prevSame && nextSame) return 'middle'
  if (prevSame) return 'end'
  if (nextSame) return 'start'
  return 'single'
}

export function messageGroupClass(position: MessageGroupPosition, role: 'user' | 'assistant'): string {
  return `mc-msg-group mc-msg-group-${position} mc-msg-group-${role}`
}
