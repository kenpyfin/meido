import { Button } from '@/components/ui/button'
import { useCallback, useEffect, useMemo, useState } from 'react'
import { api } from '../../api/client'
import type { Persona, PersonaTodo } from '../../types'

const COLUMNS = ['todo', 'doing', 'blocked', 'done'] as const
type Column = (typeof COLUMNS)[number]

type BoardItem = PersonaTodo & {
  description?: string | null
  owner_persona_id?: number | null
  priority?: number
  due_at?: string | null
  blocked_reason?: string | null
  position?: number
}

type Props = {
  chatId: number | null
  personas: Persona[]
  /** When set, only these columns are shown (Inbox uses the open ones). */
  columns?: Column[]
}

function personaName(personas: Persona[], id: number | null | undefined): string {
  if (id == null) return 'Unassigned'
  return personas.find((p) => p.id === id)?.name ?? `Persona ${id}`
}

export function BoardView({ chatId, personas, columns = [...COLUMNS] }: Props) {
  const [items, setItems] = useState<BoardItem[]>([])
  const [error, setError] = useState('')
  const [dragId, setDragId] = useState<number | null>(null)

  const load = useCallback(async () => {
    if (chatId == null) return
    const query = new URLSearchParams({ chat_id: String(chatId), status: 'all' })
    const data = await api<{ todos?: BoardItem[] }>(`/api/todos?${query.toString()}`)
    setItems(Array.isArray(data.todos) ? data.todos : [])
  }, [chatId])

  useEffect(() => {
    void load().catch((err: unknown) => {
      setError(err instanceof Error ? err.message : 'Could not load the board')
    })
  }, [load])

  const byColumn = useMemo(() => {
    const grouped: Record<Column, BoardItem[]> = { todo: [], doing: [], blocked: [], done: [] }
    for (const item of items) {
      const status = (COLUMNS as readonly string[]).includes(item.status) ? (item.status as Column) : 'todo'
      grouped[status].push(item)
    }
    for (const column of COLUMNS) {
      grouped[column].sort((a, b) => (a.position ?? a.id) - (b.position ?? b.id))
    }
    return grouped
  }, [items])

  async function dropOn(column: Column, before: BoardItem | null, after: BoardItem | null) {
    if (dragId == null || chatId == null) return
    const id = dragId
    setDragId(null)
    const previous = items
    setItems((current) =>
      current.map((item) => (item.id === id ? { ...item, status: column } : item)),
    )
    try {
      await api(`/api/todos/${id}`, {
        method: 'PATCH',
        body: JSON.stringify({
          status: column,
          before_position: before?.position ?? null,
          after_position: after?.position ?? null,
        }),
      })
      await load()
    } catch (err) {
      setItems(previous)
      setError(err instanceof Error ? err.message : 'Could not move the card')
    }
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-2">
      <div className="flex items-center justify-between">
        <span className="text-sm font-medium">Board</span>
        <Button size="sm" variant="secondary" type="button" onClick={() => void load()}>
          Refresh
        </Button>
      </div>
      {error ? <p className="text-sm text-red-600">{error}</p> : null}
      <div className="grid min-h-0 flex-1 grid-cols-1 gap-2 md:grid-cols-4">
        {columns.map((column) => (
          <section
            key={column}
            className="flex min-h-40 flex-col rounded-lg border border-[color:var(--mc-border-soft)] bg-[color:var(--mc-bg-panel)] p-2"
            onDragOver={(event) => event.preventDefault()}
            onDrop={(event) => {
              event.preventDefault()
              const list = byColumn[column]
              void dropOn(column, list.length ? list[list.length - 1] : null, null)
            }}
          >
            <h3 className="mb-2 text-xs font-semibold uppercase tracking-wide text-[color:var(--mc-text-muted)]">
              {column} · {byColumn[column].length}
            </h3>
            <div className="flex min-h-0 flex-1 flex-col gap-2 overflow-auto">
              {byColumn[column].map((item, index) => (
                <article
                  key={item.id}
                  draggable
                  onDragStart={() => setDragId(item.id)}
                  onDragOver={(event) => event.preventDefault()}
                  onDrop={(event) => {
                    event.preventDefault()
                    event.stopPropagation()
                    const list = byColumn[column]
                    void dropOn(column, index > 0 ? list[index - 1] : null, item)
                  }}
                  className="cursor-grab rounded-md border border-[color:var(--mc-border-soft)] bg-[color:var(--mc-bg-sidebar)] p-2 text-sm"
                >
                  <div className="font-medium">{item.title}</div>
                  <div className="text-xs text-[color:var(--mc-text-muted)]">
                    {personaName(personas, item.owner_persona_id ?? item.persona_id)}
                    {item.due_at ? ` · due ${item.due_at.slice(0, 10)}` : ''}
                  </div>
                  {item.blocked_reason ? (
                    <div className="mt-1 text-xs">{item.blocked_reason}</div>
                  ) : null}
                </article>
              ))}
            </div>
          </section>
        ))}
      </div>
    </div>
  )
}
