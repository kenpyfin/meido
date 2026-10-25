import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { MarkdownStream } from '@/components/markdown-stream'
import { useCallback, useEffect, useMemo, useState } from 'react'
import { api } from '../../api/client'
import type { Persona, PersonaTodo } from '../../types'

const COLUMNS = ['todo', 'plan', 'build', 'verify'] as const
type Column = (typeof COLUMNS)[number]

const COLUMN_LABEL: Record<Column, string> = {
  todo: 'TODO',
  plan: 'PLAN',
  build: 'BUILD',
  verify: 'VERIFY',
}

const STAGE_BRIEF: Record<Column, string> = {
  todo: 'Capture the outcome, the constraints, and what done looks like.',
  plan: 'Write the problem, the files involved, a checklist, and open questions.',
  build: 'Do the work from the plan and record what changed.',
  verify: 'Check the work against the plan. Passing this stage closes the card.',
}

type Proposal = {
  id: number
  item_id: number | null
  change_json: string
}

type BoardEvent = {
  id: number
  from_status?: string | null
  to_status: string
  actor_kind: string
  actor_persona_id?: number | null
  at: string
}

type Props = {
  chatId: number | null
  personas: Persona[]
  onOpenPersona?: (personaId: number, sessionId: string | null) => void
  focusTodoId?: number | null
  onFocusTodoHandled?: () => void
  /** Static cards for a public example. Skips the board API. */
  fixtureItems?: PersonaTodo[]
}

function personaName(personas: Persona[], id: number | null | undefined): string {
  if (id == null) return 'Unassigned'
  return personas.find((persona) => persona.id === id)?.name ?? `Persona ${id}`
}

function stageLabel(status: string): string {
  if ((COLUMNS as readonly string[]).includes(status)) return COLUMN_LABEL[status as Column]
  return status.toUpperCase()
}

function stageBrief(status: string): string {
  if ((COLUMNS as readonly string[]).includes(status)) return STAGE_BRIEF[status as Column]
  return 'Write the instruction for this deploy.'
}

function proposalSummary(changeJson: string): string {
  try {
    const change = JSON.parse(changeJson) as {
      status?: string
      title?: string
      blocked_reason?: string
    }
    if (change.status) return `Move to ${change.status}`
    if (change.title) return change.title
    if (change.blocked_reason) return change.blocked_reason
  } catch {
    return changeJson
  }
  return changeJson
}

function formatWhen(iso: string): string {
  const ms = Date.parse(iso)
  if (!Number.isFinite(ms)) return iso
  try {
    return new Date(ms).toLocaleString()
  } catch {
    return iso
  }
}

function eventLine(event: BoardEvent): string {
  const to = stageLabel(event.to_status)
  if (event.from_status && event.from_status !== event.to_status) {
    return `${stageLabel(event.from_status)} → ${to}`
  }
  if (event.from_status === event.to_status) return `Marked complete · ${to}`
  return to
}

export function BoardView({
  chatId,
  personas,
  onOpenPersona,
  focusTodoId = null,
  onFocusTodoHandled,
  fixtureItems,
}: Props) {
  const [items, setItems] = useState<PersonaTodo[]>([])
  const [proposals, setProposals] = useState<Proposal[]>([])
  const [events, setEvents] = useState<BoardEvent[]>([])
  const [error, setError] = useState('')
  const [dragId, setDragId] = useState<number | null>(null)
  const [ownerFilter, setOwnerFilter] = useState('all')
  const [hideClosed, setHideClosed] = useState(true)
  const [title, setTitle] = useState('')
  const [newOwner, setNewOwner] = useState<number | ''>('')
  const [busyId, setBusyId] = useState<number | null>(null)
  const [ready, setReady] = useState(false)
  const [selectedId, setSelectedId] = useState<number | null>(null)
  const [deployPromptDraft, setDeployPromptDraft] = useState('')
  const [titleDraft, setTitleDraft] = useState('')

  const load = useCallback(async () => {
    if (fixtureItems) {
      setItems(fixtureItems)
      setProposals([])
      setReady(true)
      return
    }
    if (chatId == null) return
    const query = new URLSearchParams({ chat_id: String(chatId), status: 'all' })
    const [board, pending] = await Promise.all([
      api<{ todos?: PersonaTodo[] }>(`/api/todos?${query.toString()}`),
      api<{ proposals?: Proposal[] }>(`/api/coordinator/proposals?chat_id=${chatId}`),
    ])
    setItems(Array.isArray(board.todos) ? board.todos : [])
    setProposals(Array.isArray(pending.proposals) ? pending.proposals : [])
    setReady(true)
  }, [chatId, fixtureItems])

  useEffect(() => {
    void load().catch((err: unknown) => {
      setError(err instanceof Error ? err.message : 'Could not load the board')
    })
  }, [load])

  const running = items.some((item) => item.stage_outcome === 'running')
  useEffect(() => {
    if (!running) return
    const timer = window.setInterval(() => {
      void load().catch(() => undefined)
    }, 4000)
    return () => window.clearInterval(timer)
  }, [running, load])

  const selected = items.find((item) => item.id === selectedId) ?? null

  function openTicket(item: PersonaTodo) {
    setSelectedId(item.id)
    setDeployPromptDraft(item.deploy_prompt ?? '')
    setTitleDraft(item.title)
  }

  useEffect(() => {
    if (selectedId == null) {
      setEvents([])
      return
    }
    let cancelled = false
    void api<{ events?: BoardEvent[] }>(`/api/todos/${selectedId}/events`)
      .then((data) => {
        if (!cancelled) setEvents(Array.isArray(data.events) ? data.events : [])
      })
      .catch(() => {
        if (!cancelled) setEvents([])
      })
    return () => {
      cancelled = true
    }
  }, [selectedId, items])

  useEffect(() => {
    if (focusTodoId == null || !ready) return
    const item = items.find((entry) => entry.id === focusTodoId)
    if (!item) {
      setError(`Ticket #${focusTodoId} is not on this board.`)
      onFocusTodoHandled?.()
      return
    }
    const owner = String(item.owner_persona_id ?? item.persona_id)
    const needsUnhide = Boolean(item.completed_at) && hideClosed
    const needsOwner = ownerFilter !== 'all' && owner !== ownerFilter
    if (needsUnhide) setHideClosed(false)
    if (needsOwner) setOwnerFilter('all')
    if (needsUnhide || needsOwner) return
    openTicket(item)
    onFocusTodoHandled?.()
    window.requestAnimationFrame(() => {
      document.getElementById(`board-card-${item.id}`)?.scrollIntoView({ block: 'nearest' })
    })
  }, [focusTodoId, ready, items, hideClosed, ownerFilter, onFocusTodoHandled])

  const visible = useMemo(() => {
    return items.filter((item) => {
      if (hideClosed && item.completed_at) return false
      if (ownerFilter === 'all') return true
      const owner = item.owner_persona_id ?? item.persona_id
      return String(owner) === ownerFilter
    })
  }, [hideClosed, items, ownerFilter])

  const byColumn = useMemo(() => {
    const grouped: Record<Column, PersonaTodo[]> = {
      todo: [],
      plan: [],
      build: [],
      verify: [],
    }
    for (const item of visible) {
      const status = (COLUMNS as readonly string[]).includes(item.status)
        ? (item.status as Column)
        : 'todo'
      grouped[status].push(item)
    }
    for (const column of COLUMNS) {
      grouped[column].sort((a, b) => (a.position ?? a.id) - (b.position ?? b.id))
    }
    return grouped
  }, [visible])

  async function dropOn(column: Column, before: PersonaTodo | null, after: PersonaTodo | null) {
    if (dragId == null || chatId == null) return
    const id = dragId
    setDragId(null)
    const previous = items
    setItems((current) => current.map((item) => (item.id === id ? { ...item, status: column } : item)))
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

  async function patchItem(id: number, body: Record<string, unknown>) {
    setError('')
    await api(`/api/todos/${id}`, { method: 'PATCH', body: JSON.stringify(body) })
    await load()
  }

  async function runStage(id: number, deployPrompt?: string) {
    setBusyId(id)
    setError('')
    try {
      await api(`/api/todos/${id}/run`, {
        method: 'POST',
        body: JSON.stringify(deployPrompt === undefined ? {} : { deploy_prompt: deployPrompt }),
      })
      await load()
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Could not deploy this ticket')
    } finally {
      setBusyId(null)
    }
  }

  async function completeItem(id: number) {
    setError('')
    try {
      await patchItem(id, { completed: true })
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Could not complete this ticket')
    }
  }

  async function createCard() {
    const nextTitle = title.trim()
    if (!nextTitle || chatId == null) return
    setError('')
    try {
      await api('/api/todos', {
        method: 'POST',
        body: JSON.stringify({
          chat_id: chatId,
          title: nextTitle,
          persona_id: newOwner === '' ? undefined : newOwner,
        }),
      })
      setTitle('')
      await load()
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Could not add the card')
    }
  }

  async function openOwner(personaId: number) {
    if (onOpenPersona == null || chatId == null) return
    try {
      const data = await api<{ sessions?: Array<{ id: string }> }>(
        `/api/chat_sessions?chat_id=${chatId}&persona_id=${personaId}`,
      )
      onOpenPersona(personaId, data.sessions?.[0]?.id ?? null)
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Could not open that persona')
    }
  }

  async function decide(id: number, status: 'accepted' | 'rejected') {
    setError('')
    try {
      await api(`/api/coordinator/proposals/${id}`, {
        method: 'POST',
        body: JSON.stringify({ status }),
      })
      await load()
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Could not decide the proposal')
    }
  }

  function closeDetail() {
    if (selected == null) {
      setSelectedId(null)
      return
    }
    const dirty = deployPromptDraft !== (selected.deploy_prompt ?? '')
    if (dirty && !window.confirm('Discard the unsaved deploy prompt?')) return
    setSelectedId(null)
  }

  if (chatId == null && fixtureItems == null) {
    return <p className="p-4 text-sm text-[color:var(--mc-text-muted)]">Open a chat to use the board.</p>
  }

  const promptDirty = selected != null && deployPromptDraft !== (selected.deploy_prompt ?? '')
  const selectedOwner = selected ? (selected.owner_persona_id ?? selected.persona_id) : null
  const selectedRunning = selected != null && (selected.stage_outcome === 'running' || busyId === selected.id)

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <h2 className="text-lg font-semibold">Board</h2>
        <input
          value={title}
          onChange={(event) => setTitle(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter') void createCard()
          }}
          placeholder="New card"
          className="min-w-40 flex-1 rounded-md border border-[color:var(--mc-border-soft)] bg-transparent px-2 py-1 text-sm"
        />
        <select
          value={newOwner}
          onChange={(event) => setNewOwner(event.target.value === '' ? '' : Number(event.target.value))}
          className="rounded-md border border-[color:var(--mc-border-soft)] bg-transparent px-2 py-1 text-sm"
          aria-label="Owner for the new card"
        >
          <option value="">Current persona</option>
          {personas.map((persona) => (
            <option key={persona.id} value={persona.id}>
              {persona.name}
            </option>
          ))}
        </select>
        <Button type="button" size="sm" disabled={!title.trim()} onClick={() => void createCard()}>
          Add card
        </Button>
        <select
          value={ownerFilter}
          onChange={(event) => setOwnerFilter(event.target.value)}
          className="rounded-md border border-[color:var(--mc-border-soft)] bg-transparent px-2 py-1 text-sm"
          aria-label="Filter by owner"
        >
          <option value="all">All owners</option>
          {personas.map((persona) => (
            <option key={persona.id} value={persona.id}>
              {persona.name}
            </option>
          ))}
        </select>
        <label className="flex items-center gap-2 text-sm text-[color:var(--mc-text-muted)]">
          <input
            type="checkbox"
            checked={hideClosed}
            onChange={(event) => setHideClosed(event.target.checked)}
          />
          Hide closed
        </label>
        <Button size="sm" variant="secondary" type="button" onClick={() => void load()}>
          Refresh
        </Button>
      </div>
      {error ? (
        <p className="text-sm text-red-600" role="alert">
          {error}
        </p>
      ) : null}
      <div className="flex min-h-0 flex-1 gap-2 overflow-x-auto">
        {COLUMNS.map((column) => (
          <section
            key={column}
            className="flex min-h-40 w-72 shrink-0 flex-col rounded-lg border border-[color:var(--mc-border-soft)] bg-[color:var(--mc-bg-panel)] p-2"
            onDragOver={(event) => event.preventDefault()}
            onDrop={(event) => {
              event.preventDefault()
              const list = byColumn[column]
              void dropOn(column, list.length ? list[list.length - 1] : null, null)
            }}
          >
            <h3 className="mb-2 text-xs font-semibold uppercase tracking-wide text-[color:var(--mc-text-muted)]">
              {COLUMN_LABEL[column]} · {byColumn[column].length}
            </h3>
            <div className="flex min-h-0 flex-1 flex-col gap-2 overflow-auto">
              {byColumn[column].map((item, index) => {
                const ownerId = item.owner_persona_id ?? item.persona_id
                const stageRunning = item.stage_outcome === 'running' || busyId === item.id
                const cardProposals = proposals.filter((proposal) => proposal.item_id === item.id)
                return (
                  <article
                    key={item.id}
                    id={`board-card-${item.id}`}
                    draggable
                    onDragStart={() => setDragId(item.id)}
                    onDragOver={(event) => event.preventDefault()}
                    onDrop={(event) => {
                      event.preventDefault()
                      event.stopPropagation()
                      const list = byColumn[column]
                      void dropOn(column, index > 0 ? list[index - 1] : null, item)
                    }}
                    onClick={() => openTicket(item)}
                    className="cursor-grab rounded-md border border-[color:var(--mc-border-soft)] bg-[color:var(--mc-bg-sidebar)] p-2 text-sm"
                  >
                    <div className="flex items-start justify-between gap-2">
                      <div className="line-clamp-2 font-medium">{item.title}</div>
                      <span className="shrink-0 text-xs text-[color:var(--mc-text-muted)]">#{item.id}</span>
                    </div>
                    <div className="mt-1 flex flex-wrap items-center gap-2 text-xs text-[color:var(--mc-text-muted)]">
                      <select
                        value={ownerId}
                        aria-label={`Owner for ${item.title}`}
                        className="max-w-full rounded border border-[color:var(--mc-border-soft)] bg-transparent px-1 py-0.5"
                        onClick={(event) => event.stopPropagation()}
                        onChange={(event) => {
                          event.stopPropagation()
                          void patchItem(item.id, { owner_persona_id: Number(event.target.value) }).catch(
                            (err: unknown) => {
                              setError(err instanceof Error ? err.message : 'Could not change the owner')
                            },
                          )
                        }}
                      >
                        {personas.map((persona) => (
                          <option key={persona.id} value={persona.id}>
                            {persona.name}
                          </option>
                        ))}
                      </select>
                      {onOpenPersona ? (
                        <button
                          type="button"
                          className="underline decoration-[color:var(--mc-border-soft)] underline-offset-2"
                          onClick={(event) => {
                            event.stopPropagation()
                            void openOwner(ownerId)
                          }}
                        >
                          Open {personaName(personas, ownerId)}
                        </button>
                      ) : null}
                    </div>
                    {item.blocked_reason ? (
                      <div className="mt-1 line-clamp-1 text-xs text-[color:var(--mc-accent-brass)]">
                        Blocked: {item.blocked_reason}
                      </div>
                    ) : stageRunning ? (
                      <div className="mt-1 line-clamp-1 text-xs text-[color:var(--mc-accent-brass)]">Running…</div>
                    ) : null}
                    {cardProposals.map((proposal) => (
                      <div
                        key={proposal.id}
                        className="mt-2 flex flex-wrap items-center gap-1 text-xs"
                        onClick={(event) => event.stopPropagation()}
                      >
                        <span className="min-w-0 flex-1">{proposalSummary(proposal.change_json)}</span>
                        <Button size="sm" variant="outline" type="button" onClick={() => void decide(proposal.id, 'accepted')}>
                          Accept
                        </Button>
                        <Button size="sm" variant="ghost" type="button" onClick={() => void decide(proposal.id, 'rejected')}>
                          Reject
                        </Button>
                      </div>
                    ))}
                    <div className="mt-2 flex flex-wrap items-center gap-2" onClick={(event) => event.stopPropagation()}>
                      <Button
                        size="sm"
                        type="button"
                        disabled={stageRunning}
                        onClick={() => void runStage(item.id)}
                      >
                        {stageRunning ? 'Running…' : 'Deploy'}
                      </Button>
                      <Button
                        size="sm"
                        variant="secondary"
                        type="button"
                        disabled={Boolean(item.completed_at)}
                        onClick={() => void completeItem(item.id)}
                      >
                        Complete
                      </Button>
                      <label className="flex items-center gap-1 text-xs text-[color:var(--mc-text-muted)]">
                        <input
                          type="checkbox"
                          checked={item.auto_advance === true}
                          aria-label={`Auto for ${item.title}`}
                          onChange={(event) => {
                            void patchItem(item.id, { auto_advance: event.target.checked }).catch(
                              (err: unknown) => {
                                setError(err instanceof Error ? err.message : 'Could not change Auto')
                              },
                            )
                          }}
                        />
                        Auto
                      </label>
                    </div>
                  </article>
                )
              })}
            </div>
          </section>
        ))}
      </div>
      <Dialog
        open={selected != null}
        onOpenChange={(open) => {
          if (!open) closeDetail()
        }}
      >
        <DialogContent className="max-w-2xl">
          {selected ? (
            <>
              <DialogTitle>Ticket #{selected.id}</DialogTitle>
              <DialogDescription>
                {stageLabel(selected.status)}
                {selected.completed_at ? ' · Closed' : ''}
                {' · '}
                History stays in this view. The card on the board stays short.
              </DialogDescription>
              <div className="mt-3 flex flex-col gap-3">
                <label className="flex flex-col gap-1 text-sm">
                  Title
                  <input
                    value={titleDraft}
                    aria-label="Ticket title"
                    onChange={(event) => setTitleDraft(event.target.value)}
                    onBlur={() => {
                      const next = titleDraft.trim()
                      if (!next || next === selected.title) return
                      void patchItem(selected.id, { title: next }).catch((err: unknown) => {
                        setError(err instanceof Error ? err.message : 'Could not rename the ticket')
                      })
                    }}
                    className="rounded-md border border-[color:var(--mc-border-soft)] bg-transparent px-2 py-1"
                  />
                </label>
                <div className="flex flex-wrap items-center gap-3 text-sm">
                  <select
                    value={selectedOwner ?? ''}
                    aria-label="Ticket owner"
                    className="rounded-md border border-[color:var(--mc-border-soft)] bg-transparent px-2 py-1"
                    onChange={(event) => {
                      void patchItem(selected.id, { owner_persona_id: Number(event.target.value) }).catch(
                        (err: unknown) => {
                          setError(err instanceof Error ? err.message : 'Could not change the owner')
                        },
                      )
                    }}
                  >
                    {personas.map((persona) => (
                      <option key={persona.id} value={persona.id}>
                        {persona.name}
                      </option>
                    ))}
                  </select>
                  <label className="flex items-center gap-2 text-[color:var(--mc-text-muted)]">
                    <input
                      type="checkbox"
                      checked={selected.auto_advance === true}
                      onChange={(event) => {
                        void patchItem(selected.id, { auto_advance: event.target.checked }).catch(
                          (err: unknown) => {
                            setError(err instanceof Error ? err.message : 'Could not change Auto')
                          },
                        )
                      }}
                    />
                    Auto
                  </label>
                </div>
                <label className="flex flex-col gap-1 text-sm">
                  Deploy prompt
                  <textarea
                    value={deployPromptDraft}
                    aria-label="Deploy prompt"
                    rows={5}
                    placeholder={stageBrief(selected.status)}
                    onChange={(event) => setDeployPromptDraft(event.target.value)}
                    className="rounded-md border border-[color:var(--mc-border-soft)] bg-transparent px-2 py-1"
                  />
                </label>
                <div className="flex flex-wrap gap-2">
                  <Button
                    size="sm"
                    variant="outline"
                    type="button"
                    disabled={!promptDirty}
                    onClick={() => {
                      void patchItem(selected.id, { deploy_prompt: deployPromptDraft }).catch((err: unknown) => {
                        setError(err instanceof Error ? err.message : 'Could not save the deploy prompt')
                      })
                    }}
                  >
                    Save prompt
                  </Button>
                  <Button
                    size="sm"
                    type="button"
                    disabled={selectedRunning}
                    onClick={() => void runStage(selected.id, promptDirty ? deployPromptDraft : undefined)}
                  >
                    {selectedRunning ? 'Running…' : 'Deploy'}
                  </Button>
                </div>
                <section>
                  <h3 className="mb-1 text-xs font-semibold uppercase tracking-wide text-[color:var(--mc-text-muted)]">
                    Stage notes
                  </h3>
                  <div className="max-h-60 overflow-y-auto rounded-md border border-[color:var(--mc-border-soft)] p-2">
                    {selected.description?.trim() ? (
                      <MarkdownStream>{selected.description}</MarkdownStream>
                    ) : (
                      <p className="text-sm text-[color:var(--mc-text-muted)]">No stage output recorded yet.</p>
                    )}
                  </div>
                </section>
                <section>
                  <h3 className="mb-1 text-xs font-semibold uppercase tracking-wide text-[color:var(--mc-text-muted)]">
                    Transitions
                  </h3>
                  {events.length === 0 ? (
                    <p className="text-sm text-[color:var(--mc-text-muted)]">No stage changes yet.</p>
                  ) : (
                    <ul className="flex max-h-40 flex-col gap-1 overflow-y-auto text-sm">
                      {events.map((event) => (
                        <li key={event.id} className="flex flex-wrap items-baseline gap-x-2">
                          <span>{eventLine(event)}</span>
                          <span className="text-xs text-[color:var(--mc-text-muted)]">
                            {event.actor_kind}
                            {event.actor_persona_id != null
                              ? ` · ${personaName(personas, event.actor_persona_id)}`
                              : ''}
                            {` · ${formatWhen(event.at)}`}
                          </span>
                        </li>
                      ))}
                    </ul>
                  )}
                </section>
                <div className="flex flex-wrap justify-end gap-2">
                  <Button
                    size="sm"
                    variant="secondary"
                    type="button"
                    disabled={Boolean(selected.completed_at)}
                    onClick={() => void completeItem(selected.id)}
                  >
                    Complete
                  </Button>
                  <Button size="sm" variant="outline" type="button" onClick={closeDetail}>
                    Close
                  </Button>
                </div>
              </div>
            </>
          ) : null}
        </DialogContent>
      </Dialog>
    </div>
  )
}
