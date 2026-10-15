import type { ThreadMessageLike } from '@assistant-ui/react'
import type { BackendMessage } from '../types'

function visibleContent(item: BackendMessage): string {
  const raw = item.content || ''
  if (item.origin === 'guest') {
    const name = item.sender_name?.trim() || 'Guest'
    return `[Guest: ${name}]\n${raw}`
  }
  if (item.origin === 'attached_run' || item.origin === 'attached_task') {
    const body = raw.replace(/^\[\[scheduled_(?:run|task):\d+\]\]\n?/, '')
    const kind = item.origin === 'attached_run' ? 'run' : 'task'
    return `[Attached ${kind}]\n${body}`
  }
  return raw
}

export function mapBackendHistory(messages: BackendMessage[]): ThreadMessageLike[] {
  return messages.map((item, index) => ({
    id: item.id || `history-${index}`,
    role: item.is_from_bot ? 'assistant' : 'user',
    content: [visibleContent(item), ...(item.artifacts ?? [])
      .filter(a => !(item.content || '').includes(a.url))
      .map(a => `[${a.name.replace(/[\[\]]/g, '_')}](${a.url})`)].filter(Boolean).join('\n\n'),
    createdAt: item.timestamp ? new Date(item.timestamp) : new Date(),
  }))
}

/** Compare history for sync/remount decisions: id, role, content only — ignore `createdAt` (server timestamps can jitter between polls). */
export function historiesEqual(a: ThreadMessageLike[], b: ThreadMessageLike[]): boolean {
  if (a.length !== b.length) return false
  for (let i = 0; i < a.length; i += 1) {
    const x = a[i]
    const y = b[i]
    if (x.id !== y.id) return false
    if (x.role !== y.role) return false
    if (x.content !== y.content) return false
  }
  return true
}

/** True when `next` is `prev` with older messages prepended (visible suffix unchanged). */
export function isHistoryPrepend(prev: ThreadMessageLike[], next: ThreadMessageLike[]): boolean {
  if (next.length <= prev.length) return false
  const offset = next.length - prev.length
  return historiesEqual(prev, next.slice(offset))
}
