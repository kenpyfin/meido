import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogTitle, DialogDescription, DialogClose } from '@/components/ui/dialog'
import React from 'react'
import type { Persona, PersonaTodo } from '../types'

export type InboxUnreadItem = {
  personaId: number
  personaName: string
  lastBotMessageAt: string | null
  /** Null = main chat for that persona. */
  sessionId: string | null
  sessionTitle: string | null
}

export type InboxOpenTarget = {
  personaId: number
  sessionId: string | null
}

export type InboxPanelProps = {
  appearance: 'dark' | 'light'
  open: boolean
  onOpenChange: (open: boolean) => void
  unread: InboxUnreadItem[]
  todos: PersonaTodo[]
  personas: Persona[]
  loading: boolean
  busyTodoId: number | null
  onRefresh: () => void
  onOpenTarget: (target: InboxOpenTarget) => void
  onCompleteTodo: (todoId: number) => void
}

function personaMeta(personas: Persona[], personaId: number): Persona | undefined {
  return personas.find((p) => p.id === personaId)
}

function personaName(personas: Persona[], personaId: number): string {
  return personaMeta(personas, personaId)?.name ?? `Persona ${personaId}`
}

function sessionLabel(sessionId: string | null, sessionTitle: string | null): string {
  if (sessionId == null) return 'Main chat'
  const title = sessionTitle?.trim()
  return title && title.length > 0 ? title : 'Focused session'
}

function formatWhen(iso: string | null | undefined): string {
  if (!iso) return ''
  const ms = Date.parse(iso)
  if (!Number.isFinite(ms)) return iso
  try {
    return new Date(ms).toLocaleString()
  } catch {
    return iso
  }
}

export function InboxPanel({
  appearance,
  open,
  onOpenChange,
  unread,
  todos,
  personas,
  loading,
  busyTodoId,
  onRefresh,
  onOpenTarget,
  onCompleteTodo,
}: InboxPanelProps) {
  const borderStyle =
    appearance === 'dark'
      ? { borderColor: 'var(--mc-border-soft)', background: 'var(--mc-bg-panel)' }
      : { borderColor: 'var(--gray-6)', background: 'var(--gray-2)' }
  const itemBorder =
    appearance === 'dark' ? { borderColor: 'var(--mc-border-soft)' } : { borderColor: 'var(--gray-6)' }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-[720px]">
        <DialogTitle>Inbox</DialogTitle>
        <DialogDescription className="mb-3">
          New bot messages across personas and open operator todos from conversations.
        </DialogDescription>

        <div className="flex items-center justify-between gap-3 mb-3 flex-wrap">
          <span>
            {unread.length} unread · {todos.length} open todos
          </span>
          <Button size="sm" variant="secondary" disabled={loading} onClick={onRefresh}>
            {loading ? 'Refreshing…' : 'Refresh'}
          </Button>
        </div>

        <div className="flex flex-col gap-4">
          <section>
            <span className="font-medium mb-2 block">
              New messages
            </span>
            <div className="rounded-lg border p-3" style={borderStyle}>
              {unread.length === 0 ? (
                <span>
                  No unread bot messages.
                </span>
              ) : (
                <ul className="list-none space-y-2">
                  {unread.map((item) => (
                    <li
                      key={`${item.personaId}:${item.sessionId ?? 'main'}`}
                      className="flex flex-wrap items-center gap-2 rounded-lg border p-2"
                      style={itemBorder}
                    >
                      <div className="min-w-0 flex-1">
                        <span className="block truncate font-medium">{item.personaName}</span>
                        <span className="block truncate">
                          {sessionLabel(item.sessionId, item.sessionTitle)}
                          {item.lastBotMessageAt ? ` · ${formatWhen(item.lastBotMessageAt)}` : ' · recent'}
                        </span>
                      </div>
                      <Button
                        size="sm"
                        variant="secondary"
                        onClick={() => {
                          onOpenTarget({
                            personaId: item.personaId,
                            sessionId: item.sessionId,
                          })
                          onOpenChange(false)
                        }}
                      >
                        Open
                      </Button>
                    </li>
                  ))}
                </ul>
              )}
            </div>
          </section>

          <section>
            <span className="font-medium mb-2 block">
              Todos
            </span>
            <div className="rounded-lg border p-3" style={borderStyle}>
              {todos.length === 0 ? (
                <span>
                  No open todos. The agent can create them with add_todo.
                </span>
              ) : (
                <ul className="list-none space-y-2">
                  {todos.map((todo) => {
                    const persona = personaMeta(personas, todo.persona_id)
                    const sessionId = persona?.last_bot_message_session_id ?? null
                    const sessionTitle = persona?.last_bot_message_session_title ?? null
                    return (
                      <li
                        key={todo.id}
                        className="flex flex-wrap items-start gap-2 rounded-lg border p-2"
                        style={itemBorder}
                      >
                        <div className="min-w-0 flex-1">
                          <span className="font-medium block">
                            {todo.title}
                          </span>
                          <span className="block">
                            {personaName(personas, todo.persona_id)}
                            {` · ${sessionLabel(sessionId, sessionTitle)}`}
                            {todo.source_hint ? ` · ${todo.source_hint}` : ''}
                            {todo.updated_at ? ` · ${formatWhen(todo.updated_at)}` : ''}
                          </span>
                        </div>
                        <Button
                          size="sm"
                          variant="secondary"
                          onClick={() => {
                            onOpenTarget({
                              personaId: todo.persona_id,
                              sessionId,
                            })
                            onOpenChange(false)
                          }}
                        >
                          Open
                        </Button>
                        <Button
                          size="sm"
                          variant="default"
                          disabled={busyTodoId === todo.id}
                          onClick={() => onCompleteTodo(todo.id)}
                        >
                          {busyTodoId === todo.id ? '…' : 'Complete'}
                        </Button>
                      </li>
                    )
                  })}
                </ul>
              )}
            </div>
          </section>
        </div>

        <div className="flex justify-end mt-4">
          <DialogClose>
            <Button variant="outline">
              Close
            </Button>
          </DialogClose>
        </div>
      </DialogContent>
    </Dialog>
  )
}
