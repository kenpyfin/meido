import { Button } from '@/components/ui/button'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import { api } from '../../api/client'
import type { Persona } from '../../types'
import { BoardView } from './board-view'

type Proposal = {
  id: number
  item_id: number | null
  change_json: string
  status: string
  created_at: string
}

type Props = {
  chatId: number | null
  personas: Persona[]
  onOpenPersona: (personaId: number, sessionId: string | null) => void
}

export function CoordinatorWorkspace({ chatId, personas, onOpenPersona }: Props) {
  const queryClient = useQueryClient()
  const [hour, setHour] = useState(7)
  const [brief, setBrief] = useState(true)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const coordinator = personas.find((persona) => persona.name.toLowerCase() === 'coordinator')

  const digest = useQuery({
    queryKey: ['coordinatorDigest', chatId],
    enabled: chatId != null && coordinator != null,
    queryFn: async () => {
      const data = await api<{ digest?: string }>('/api/coordinator/digest')
      return data.digest ?? ''
    },
  })

  const proposals = useQuery({
    queryKey: ['coordinatorProposals', chatId],
    enabled: chatId != null && coordinator != null,
    queryFn: async () => {
      const data = await api<{ proposals?: Proposal[] }>(`/api/coordinator/proposals?chat_id=${chatId}`)
      return data.proposals ?? []
    },
  })

  async function enable() {
    setBusy(true)
    setError('')
    try {
      const created = await api<{ persona_id: number; session_id?: string }>('/api/coordinator/enable', {
        method: 'POST',
        body: JSON.stringify({ brief_enabled: brief, brief_hour: hour }),
      })
      const session = await api<{ persona_id: number; session_id: string }>('/api/coordinator/session', {
        method: 'POST',
        body: JSON.stringify({}),
      })
      await queryClient.invalidateQueries({ queryKey: ['opsPoll', chatId] })
      onOpenPersona(session.persona_id || created.persona_id, session.session_id)
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Could not enable the Coordinator')
    } finally {
      setBusy(false)
    }
  }

  async function decide(id: number, status: 'accepted' | 'rejected') {
    await api(`/api/coordinator/proposals/${id}`, {
      method: 'POST',
      body: JSON.stringify({ status }),
    })
    await queryClient.invalidateQueries({ queryKey: ['coordinatorProposals', chatId] })
  }

  if (coordinator == null) {
    return (
      <div className="mx-auto flex max-w-lg flex-col gap-3 p-6">
        <h2 className="text-lg font-semibold">Coordinator</h2>
        <p className="text-sm text-[color:var(--mc-text-muted)]">
          The Coordinator is off. Enabling it creates one persona and, if you choose, one daily brief. Nothing runs until then.
        </p>
        <label className="flex items-center gap-2 text-sm">
          <input type="checkbox" checked={brief} onChange={(event) => setBrief(event.target.checked)} />
          Daily brief at hour
          <input
            type="number"
            min={0}
            max={23}
            value={hour}
            onChange={(event) => setHour(Number(event.target.value))}
            className="w-16 rounded-md border border-[color:var(--mc-border-soft)] bg-transparent px-2 py-1"
          />
        </label>
        {error ? <p className="text-sm text-red-600">{error}</p> : null}
        <Button type="button" disabled={busy} onClick={() => void enable()}>
          {busy ? 'Enabling…' : 'Enable Coordinator'}
        </Button>
      </div>
    )
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-auto p-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h2 className="text-lg font-semibold">Coordinator</h2>
        <Button
          size="sm"
          type="button"
          onClick={() => {
            void api<{ persona_id: number; session_id: string }>('/api/coordinator/session', {
              method: 'POST',
              body: JSON.stringify({}),
            }).then((session) => onOpenPersona(session.persona_id, session.session_id))
          }}
        >
          Open today’s chat
        </Button>
      </div>
      <section className="rounded-lg border border-[color:var(--mc-border-soft)] p-3">
        <h3 className="mb-2 text-sm font-medium">Today’s digest</h3>
        <pre className="whitespace-pre-wrap text-sm">{digest.data || (digest.isFetching ? 'Loading…' : 'No digest yet.')}</pre>
      </section>
      {(proposals.data ?? []).length > 0 ? (
        <section className="flex flex-col gap-2">
          <h3 className="text-sm font-medium">Proposals</h3>
          {(proposals.data ?? []).map((proposal) => (
            <div key={proposal.id} className="flex flex-wrap items-center gap-2 rounded-md border border-[color:var(--mc-border-soft)] p-2 text-sm">
              <span className="min-w-0 flex-1 truncate">{proposal.change_json}</span>
              <Button size="sm" type="button" onClick={() => void decide(proposal.id, 'accepted')}>
                Accept
              </Button>
              <Button size="sm" variant="secondary" type="button" onClick={() => void decide(proposal.id, 'rejected')}>
                Reject
              </Button>
            </div>
          ))}
        </section>
      ) : null}
      <BoardView chatId={chatId} personas={personas} />
    </div>
  )
}
