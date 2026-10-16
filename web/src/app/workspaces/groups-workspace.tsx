import { Button } from '@/components/ui/button'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect, useMemo, useState } from 'react'
import { api } from '../../api/client'
import type { LlmConfigResponse, Persona, QueueLane } from '../../types'

type Member = { persona_id: number; name: string; role_note: string; position: number }
type Group = {
  id: string
  name: string
  turn_mode: string
  moderator_provider: string
  moderator_model: string
  max_speakers: number
  archived_at?: string | null
  members: Member[]
}
type GroupMessage = {
  id: string
  speaker_persona_id: number | null
  content: string
  timestamp: string
}

type Props = {
  chatId: number | null
  personas: Persona[]
  queueLanes: QueueLane[]
}

export function GroupsWorkspace({ chatId, personas, queueLanes }: Props) {
  const queryClient = useQueryClient()
  const [selected, setSelected] = useState<string | null>(null)
  const [name, setName] = useState('')
  const [draft, setDraft] = useState('')
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [error, setError] = useState('')

  const groups = useQuery({
    queryKey: ['groups', chatId],
    enabled: chatId != null,
    queryFn: async () => {
      const data = await api<{ groups?: Group[] }>(`/api/groups?chat_id=${chatId}`)
      return data.groups ?? []
    },
  })

  const group = (groups.data ?? []).find((item) => item.id === selected) ?? null
  const running = useMemo(
    () =>
      queueLanes.some((lane) =>
        (lane.items ?? []).some((item) => selected != null && item.label === `group:${selected}`),
      ),
    [queueLanes, selected],
  )

  const history = useQuery({
    queryKey: ['groupHistory', selected],
    enabled: selected != null,
    queryFn: async () => {
      const data = await api<{ messages?: GroupMessage[]; summary?: string; session_id?: string }>(
        `/api/groups/${selected}/history`,
      )
      return data
    },
  })

  useEffect(() => {
    if (!running && selected) {
      void queryClient.invalidateQueries({ queryKey: ['groupHistory', selected] })
    }
  }, [queryClient, running, selected])

  async function createGroup() {
    const trimmed = name.trim()
    if (!trimmed) return
    const data = await api<{ group: Group }>('/api/groups', {
      method: 'POST',
      body: JSON.stringify({ name: trimmed }),
    })
    setName('')
    await queryClient.invalidateQueries({ queryKey: ['groups', chatId] })
    setSelected(data.group.id)
  }

  async function send() {
    if (!selected || !draft.trim()) return
    setError('')
    try {
      await api(`/api/groups/${selected}/send`, {
        method: 'POST',
        body: JSON.stringify({ message: draft.trim() }),
      })
      setDraft('')
      await queryClient.invalidateQueries({ queryKey: ['groupHistory', selected] })
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Could not send')
    }
  }

  return (
    <div className="flex min-h-0 flex-1">
      <aside className="flex w-56 shrink-0 flex-col gap-2 border-r border-[color:var(--mc-border-soft)] p-3">
        <h2 className="text-sm font-semibold">Groups</h2>
        <form
          className="flex gap-1"
          onSubmit={(event) => {
            event.preventDefault()
            void createGroup()
          }}
        >
          <input
            value={name}
            onChange={(event) => setName(event.target.value)}
            placeholder="New group"
            className="min-w-0 flex-1 rounded-md border border-[color:var(--mc-border-soft)] bg-transparent px-2 py-1 text-sm"
          />
          <Button size="sm" type="submit">
            Add
          </Button>
        </form>
        <div className="flex flex-col gap-1 overflow-auto">
          {(groups.data ?? []).map((item) => (
            <button
              key={item.id}
              type="button"
              className={
                item.id === selected
                  ? 'rounded-md bg-[color:var(--mc-bg-panel)] px-2 py-1 text-left text-sm'
                  : 'rounded-md px-2 py-1 text-left text-sm text-[color:var(--mc-text-muted)]'
              }
              onClick={() => setSelected(item.id)}
            >
              {item.name}
            </button>
          ))}
        </div>
      </aside>
      <section className="flex min-w-0 flex-1 flex-col">
        {group == null ? (
          <p className="p-4 text-sm text-[color:var(--mc-text-muted)]">Create a group, then add personas in settings.</p>
        ) : (
          <>
            <header className="flex items-center justify-between gap-2 border-b border-[color:var(--mc-border-soft)] px-3 py-2">
              <div>
                <div className="font-medium">{group.name}</div>
                <div className="text-xs text-[color:var(--mc-text-muted)]">
                  {group.turn_mode} · up to {group.max_speakers} speakers
                  {running ? ' · speaking' : ''}
                </div>
              </div>
              <div className="flex gap-1">
                <Button size="sm" variant="secondary" type="button" onClick={() => setSettingsOpen((open) => !open)}>
                  Settings
                </Button>
                <Button
                  size="sm"
                  variant="secondary"
                  type="button"
                  onClick={() => void api(`/api/groups/${group.id}/sessions`, { method: 'POST' }).then(() => history.refetch())}
                >
                  New session
                </Button>
                {running ? (
                  <Button size="sm" variant="secondary" type="button" onClick={() => void api(`/api/groups/${group.id}/cancel`, { method: 'POST' })}>
                    Stop
                  </Button>
                ) : null}
              </div>
            </header>
            {settingsOpen ? (
              <GroupSettings
                group={group}
                personas={personas}
                onSaved={() => {
                  setSettingsOpen(false)
                  void queryClient.invalidateQueries({ queryKey: ['groups', chatId] })
                }}
              />
            ) : null}
            {history.data?.summary ? (
              <p className="border-b border-[color:var(--mc-border-soft)] px-3 py-2 text-xs text-[color:var(--mc-text-muted)]">
                {history.data.summary}
              </p>
            ) : null}
            <div className="flex min-h-0 flex-1 flex-col gap-2 overflow-auto p-3">
              {(history.data?.messages ?? []).map((message) => {
                const speaker = group.members.find((member) => member.persona_id === message.speaker_persona_id)
                return (
                  <article key={message.id} className="rounded-md border border-[color:var(--mc-border-soft)] p-2 text-sm">
                    <div className="mb-1 flex items-center justify-between text-xs text-[color:var(--mc-text-muted)]">
                      <span>{speaker?.name ?? (message.speaker_persona_id == null ? 'You' : 'Persona')}</span>
                      <button
                        type="button"
                        className="underline"
                        onClick={() =>
                          void api(`/api/groups/${group.id}/messages/${message.id}/card`, {
                            method: 'POST',
                            body: JSON.stringify({ title: message.content.slice(0, 80) }),
                          })
                        }
                      >
                        Make card
                      </button>
                    </div>
                    <p className="whitespace-pre-wrap">{message.content}</p>
                  </article>
                )
              })}
            </div>
            {error ? <p className="px-3 text-sm text-red-600">{error}</p> : null}
            <form
              className="flex gap-2 border-t border-[color:var(--mc-border-soft)] p-3"
              onSubmit={(event) => {
                event.preventDefault()
                void send()
              }}
            >
              <input
                value={draft}
                onChange={(event) => setDraft(event.target.value)}
                placeholder="Message the group"
                className="min-w-0 flex-1 rounded-md border border-[color:var(--mc-border-soft)] bg-transparent px-2 py-1 text-sm"
              />
              <Button type="submit" disabled={!draft.trim()}>
                Send
              </Button>
            </form>
          </>
        )}
      </section>
    </div>
  )
}

function GroupSettings({
  group,
  personas,
  onSaved,
}: {
  group: Group
  personas: Persona[]
  onSaved: () => void
}) {
  const [mode, setMode] = useState(group.turn_mode)
  const [maxSpeakers, setMaxSpeakers] = useState(group.max_speakers)
  const [provider, setProvider] = useState(group.moderator_provider)
  const [model, setModel] = useState(group.moderator_model)
  const [members, setMembers] = useState<Member[]>(group.members)
  const [role, setRole] = useState('')
  const [personaId, setPersonaId] = useState<number | ''>(personas[0]?.id ?? '')
  const catalog = useQuery({
    queryKey: ['llmCatalog'],
    queryFn: () => api<LlmConfigResponse>('/api/llm'),
    staleTime: 60_000,
  })
  const providers = catalog.data?.providers ?? []
  const models = providers.find((item) => item.id === provider)?.models ?? []

  async function save() {
    await api(`/api/groups/${group.id}`, {
      method: 'PATCH',
      body: JSON.stringify({
        turn_mode: mode,
        max_speakers: maxSpeakers,
        moderator_provider: provider,
        moderator_model: model,
      }),
    })
    await api(`/api/groups/${group.id}/members`, {
      method: 'POST',
      body: JSON.stringify({
        members: members.map((member) => ({
          persona_id: member.persona_id,
          role_note: member.role_note,
        })),
      }),
    })
    onSaved()
  }

  return (
    <div className="flex flex-col gap-2 border-b border-[color:var(--mc-border-soft)] p-3 text-sm">
      <label className="flex items-center gap-2">
        Turn
        <select className="rounded-md border border-[color:var(--mc-border-soft)] bg-transparent px-2 py-1" value={mode} onChange={(event) => setMode(event.target.value)}>
          <option value="moderator">Moderator</option>
          <option value="roundtable">Roundtable</option>
        </select>
        Speakers
        <input
          type="number"
          min={1}
          max={4}
          value={maxSpeakers}
          onChange={(event) => setMaxSpeakers(Number(event.target.value))}
          className="w-16 rounded-md border border-[color:var(--mc-border-soft)] bg-transparent px-2 py-1"
        />
      </label>
      <label className="flex flex-wrap items-center gap-2">
        Moderator
        <select className="rounded-md border border-[color:var(--mc-border-soft)] bg-transparent px-2 py-1" value={provider} onChange={(event) => setProvider(event.target.value)}>
          <option value="">Topic model</option>
          {providers.map((item) => (
            <option key={item.id} value={item.id}>
              {item.label}
            </option>
          ))}
        </select>
        <select className="rounded-md border border-[color:var(--mc-border-soft)] bg-transparent px-2 py-1" value={model} onChange={(event) => setModel(event.target.value)}>
          <option value="">Default</option>
          {models.map((item) => (
            <option key={item.id} value={item.id}>
              {item.id}
            </option>
          ))}
        </select>
      </label>
      <div className="flex flex-col gap-1">
        {members.map((member) => (
          <div key={member.persona_id} className="flex items-center gap-2">
            <span className="w-28 truncate">{member.name}</span>
            <input
              value={member.role_note}
              onChange={(event) =>
                setMembers((current) =>
                  current.map((row) => (row.persona_id === member.persona_id ? { ...row, role_note: event.target.value } : row)),
                )
              }
              placeholder="Role note"
              className="min-w-0 flex-1 rounded-md border border-[color:var(--mc-border-soft)] bg-transparent px-2 py-1"
            />
            <button type="button" className="text-xs underline" onClick={() => setMembers((current) => current.filter((row) => row.persona_id !== member.persona_id))}>
              Remove
            </button>
          </div>
        ))}
        <div className="flex gap-2">
          <select className="rounded-md border border-[color:var(--mc-border-soft)] bg-transparent px-2 py-1" value={personaId} onChange={(event) => setPersonaId(Number(event.target.value))}>
            {personas.map((persona) => (
              <option key={persona.id} value={persona.id}>
                {persona.name}
              </option>
            ))}
          </select>
          <input value={role} onChange={(event) => setRole(event.target.value)} placeholder="Role note" className="min-w-0 flex-1 rounded-md border border-[color:var(--mc-border-soft)] bg-transparent px-2 py-1" />
          <Button
            size="sm"
            variant="secondary"
            type="button"
            onClick={() => {
              if (personaId === '' || members.some((member) => member.persona_id === personaId)) return
              const persona = personas.find((item) => item.id === personaId)
              setMembers((current) => [
                ...current,
                { persona_id: personaId, name: persona?.name ?? 'Persona', role_note: role, position: current.length },
              ])
              setRole('')
            }}
          >
            Add
          </Button>
        </div>
      </div>
      <Button size="sm" type="button" onClick={() => void save()}>
        Save group
      </Button>
    </div>
  )
}
