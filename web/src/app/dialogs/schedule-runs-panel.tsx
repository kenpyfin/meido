import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogTitle } from '@/components/ui/dialog'
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

export function ScheduleRunsPanel({
  api,
  chatId,
  taskId,
}: {
  api: Api
  chatId: number | null
  taskId?: number
}) {
  const [runs, setRuns] = React.useState<ScheduledRunItem[]>([])
  const [error, setError] = React.useState('')
  const [selected, setSelected] = React.useState<{
    id: number
    output: string
    status: string
    error: string | null
  } | null>(null)
  const [loadingId, setLoadingId] = React.useState<number | null>(null)
  const mobile = useIsMobile()

  const load = React.useCallback(async () => {
    if (chatId == null) {
      setRuns([])
      return
    }
    const params = new URLSearchParams({ chat_id: String(chatId), limit: '20' })
    if (taskId != null) params.set('task_id', String(taskId))
    const data = await api<{ runs?: ScheduledRunItem[] }>(`/api/schedules/runs?${params.toString()}`)
    setRuns(Array.isArray(data.runs) ? data.runs : [])
  }, [api, chatId, taskId])

  React.useEffect(() => {
    setSelected(null)
    setError('')
    void load().catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)))
  }, [load])

  async function openRun(id: number): Promise<void> {
    setLoadingId(id)
    setError('')
    try {
      const data = await api<{
        run?: { id: number; output?: string | null; status: string; error?: string | null }
      }>(`/api/schedules/runs/${id}`)
      const run = data.run
      setSelected({
        id,
        output: run?.output ?? '',
        status: run?.status ?? '',
        error: run?.error ?? null,
      })
      setRuns((prev) => prev.map((row) => (row.id === id ? { ...row, read: true } : row)))
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoadingId(null)
    }
  }

  const runPage = mobile && selected ? (
    <Dialog open onOpenChange={(open) => { if (!open) setSelected(null) }}>
      <DialogContent
        className="inset-0 top-0 left-0 flex h-[100dvh] max-h-[100dvh] w-full max-w-none translate-x-0 translate-y-0 flex-col gap-3 rounded-none"
        style={{ inset: 0, transform: 'none', width: '100%', maxWidth: 'none', height: '100dvh', maxHeight: '100dvh', borderRadius: 0 }}
      >
        <Button type="button" variant="outline" size="sm" className="self-start" onClick={() => setSelected(null)}>
          Back
        </Button>
        <DialogTitle>Run #{selected.id}</DialogTitle>
        <p className="m-0 text-sm text-muted-foreground">{selected.status}</p>
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
                  #{run.id} · task #{run.task_id} · {run.status}
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
      {!mobile && selected ? (
        <div className="max-h-64 overflow-auto rounded-md border p-2">
          <div className="mb-1 flex items-center justify-between gap-2">
            <span className="text-xs font-semibold">
              Run #{selected.id} · {selected.status}
            </span>
            <Button size="sm" variant="secondary" onClick={() => setSelected(null)}>
              Hide
            </Button>
          </div>
          {selected.error ? <p className="m-0 mb-2 text-xs text-red-500">{selected.error}</p> : null}
          <pre className="m-0 whitespace-pre-wrap font-mono text-xs">{selected.output || '(no output)'}</pre>
        </div>
      ) : null}
    </div>
  )
}
