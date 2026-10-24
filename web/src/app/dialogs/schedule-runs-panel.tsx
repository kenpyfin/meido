import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { useIsMobile } from '@/lib/use-is-mobile'
import React from 'react'

export type ScheduledRunItem = {
  id: number
  task_id: number
  persona_id: number
  started_at: string
  finished_at: string
  status: string
  preview: string
  read: boolean
}

type Api = <T>(path: string, init?: RequestInit) => Promise<T>

type PersonaRef = { id: number; name: string }

function ownerName(personas: PersonaRef[], personaId: number): string {
  const name = personas.find((p) => p.id === personaId)?.name?.trim()
  return name ? name : `Persona #${personaId}`
}

export function ScheduleRunsPanel({
  api,
  chatId,
  taskId,
  personas = [],
  acknowledgeUnread = false,
  onAcknowledged,
}: {
  api: Api
  chatId: number | null
  taskId?: number
  personas?: PersonaRef[]
  /** All-runs view: mark every unread run read in one request. */
  acknowledgeUnread?: boolean
  onAcknowledged?: () => void
}) {
  const [runs, setRuns] = React.useState<ScheduledRunItem[]>([])
  const [error, setError] = React.useState('')
  const [selected, setSelected] = React.useState<{
    id: number
    output: string
    status: string
    error: string | null
    owner: string
  } | null>(null)
  const [loadingId, setLoadingId] = React.useState<number | null>(null)
  const mobile = useIsMobile()

  const onAcknowledgedRef = React.useRef(onAcknowledged)
  onAcknowledgedRef.current = onAcknowledged

  const load = React.useCallback(async () => {
    if (chatId == null) {
      setRuns([])
      return
    }
    if (acknowledgeUnread) {
      const readParams = new URLSearchParams({ chat_id: String(chatId) })
      await api(`/api/schedules/runs/read?${readParams.toString()}`, { method: 'POST' })
      onAcknowledgedRef.current?.()
    }
    const params = new URLSearchParams({ chat_id: String(chatId), limit: '20' })
    if (taskId != null) params.set('task_id', String(taskId))
    const data = await api<{ runs?: ScheduledRunItem[] }>(`/api/schedules/runs?${params.toString()}`)
    setRuns(Array.isArray(data.runs) ? data.runs : [])
  }, [acknowledgeUnread, api, chatId, taskId])

  React.useEffect(() => {
    setSelected(null)
    setError('')
    void load().catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)))
  }, [load])

  async function openRun(id: number): Promise<void> {
    setLoadingId(id)
    setError('')
    const listed = runs.find((row) => row.id === id)
    const owner = ownerName(personas, listed?.persona_id ?? 0)
    try {
      const data = await api<{
        run?: {
          id: number
          persona_id?: number
          output?: string | null
          status: string
          error?: string | null
        }
      }>(`/api/schedules/runs/${id}`)
      const run = data.run
      setSelected({
        id,
        output: run?.output ?? '',
        status: run?.status ?? '',
        error: run?.error ?? null,
        owner: run?.persona_id != null ? ownerName(personas, run.persona_id) : owner,
      })
      setRuns((prev) => prev.map((row) => (row.id === id ? { ...row, read: true } : row)))
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoadingId(null)
    }
  }

  const runPage = selected ? (
    <Dialog open onOpenChange={(open) => { if (!open) setSelected(null) }}>
      <DialogContent
        className={mobile
          ? 'mc-run-sheet inset-0 top-0 left-0 z-[60] flex h-[100dvh] max-h-[100dvh] w-full max-w-none translate-x-0 translate-y-0 flex-col gap-3 rounded-none'
          : 'mc-run-sheet z-[60] flex max-h-[min(80dvh,720px)] w-[min(92vw,640px)] max-w-[640px] flex-col gap-3 sm:max-w-[640px]'}
        style={mobile
          ? { inset: 0, transform: 'none', width: '100%', maxWidth: 'none', height: '100dvh', maxHeight: '100dvh', borderRadius: 0 }
          : undefined}
      >
        {mobile ? (
          <Button type="button" variant="outline" size="sm" className="self-start" onClick={() => setSelected(null)}>
            Back
          </Button>
        ) : null}
        <div className="mc-run-sheet-header flex flex-col gap-1">
          <DialogTitle>Run #{selected.id}</DialogTitle>
          <DialogDescription>
            Owner {selected.owner} · {selected.status}
          </DialogDescription>
        </div>
        {selected.error ? <p role="alert" className="m-0 text-sm text-destructive">{selected.error}</p> : null}
        <pre className="m-0 min-h-0 flex-1 overflow-auto whitespace-pre-wrap font-mono text-xs">{selected.output || '(no output)'}</pre>
      </DialogContent>
    </Dialog>
  ) : null

  return (
    <div className="flex min-h-0 flex-col gap-2">
      {runPage}
      {error ? <p className="m-0 text-sm text-red-500">{error}</p> : null}
      {runs.length === 0 ? (
        <p className="m-0 text-sm text-muted-foreground">No runs yet.</p>
      ) : (
        <ul className="m-0 max-h-64 list-none space-y-2 overflow-y-auto p-0">
          {runs.map((run) => (
            <li key={run.id}>
              <button
                type="button"
                className="flex w-full flex-col items-start gap-0.5 rounded-md border px-2 py-1.5 text-left text-xs"
                onClick={() => void openRun(run.id)}
              >
                <span>
                  #{run.id} · Owner {ownerName(personas, run.persona_id)} · task #{run.task_id} · {run.status}
                  {run.read ? '' : ' · unread'}
                  {loadingId === run.id ? ' · opening…' : ''}
                </span>
                <span className="text-muted-foreground">{run.finished_at}</span>
                <span className="line-clamp-2">{run.preview || '(empty)'}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}
