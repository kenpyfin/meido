import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogTitle, DialogDescription, DialogClose } from '@/components/ui/dialog'
import { useIsMobile } from '@/lib/use-is-mobile'
import React, { useEffect, useState } from 'react'
import type { QueueDialogProps } from './types'

/** Compact duration: `12s`, `3m 04s`, or `1h 02m`. */
function formatElapsed(ms: number): string {
  const totalSec = Math.max(0, Math.floor(ms / 1000))
  const hours = Math.floor(totalSec / 3600)
  const minutes = Math.floor((totalSec % 3600) / 60)
  const seconds = totalSec % 60
  if (hours > 0) return `${hours}h ${String(minutes).padStart(2, '0')}m`
  if (minutes > 0) return `${minutes}m ${String(seconds).padStart(2, '0')}s`
  return `${seconds}s`
}

function elapsedText(
  elapsedMs: number | undefined,
  sampledAtMs: number,
  now: number,
): string {
  if (elapsedMs == null || !Number.isFinite(elapsedMs)) return '-'
  const extra = sampledAtMs > 0 ? Math.max(0, now - sampledAtMs) : 0
  return formatElapsed(elapsedMs + extra)
}

function elapsedTitle(state: string): string {
  return state === 'running' ? 'running' : 'waiting'
}

export function QueueDialog(props: QueueDialogProps) {
  const { appearance, queue } = props
  const queueDialogOpen = queue.open
  const setQueueDialogOpen = queue.onOpenChange
  const queueShowAllPersonas = queue.showAllPersonas
  const setQueueShowAllPersonas = queue.setShowAllPersonas
  const queueDialogItems = queue.items
  const stoppingRunIds = queue.stoppingRunIds
  const handleQueueAction = queue.handleQueueAction
  const backgroundJobsVisible = queue.backgroundJobs
  const stoppingBackgroundJobIds = queue.stoppingBackgroundJobIds
  const isActiveBackgroundJobStatus = queue.isActiveBackgroundJobStatus
  const handleBackgroundJobStop = queue.handleBackgroundJobStop
  const sampledAtMs = queue.sampledAtMs
  const [now, setNow] = useState(() => Date.now())
  const [pickedRunId, setPickedRunId] = useState<string | null>(null)
  const [pickedJobId, setPickedJobId] = useState<string | null>(null)
  const mobile = useIsMobile()

  useEffect(() => {
    if (!queueDialogOpen) {
      setPickedRunId(null)
      setPickedJobId(null)
    }
  }, [queueDialogOpen])

  useEffect(() => {
    if (!queueDialogOpen) return
    setNow(Date.now())
    const id = window.setInterval(() => setNow(Date.now()), 1000)
    return () => window.clearInterval(id)
  }, [queueDialogOpen, sampledAtMs])

  const borderColor = 'var(--mc-border-soft)'
  void appearance
  const pickedRun = mobile ? queueDialogItems.find((it) => it.run_id === pickedRunId) ?? null : null
  const pickedJob = mobile && pickedRun == null
    ? backgroundJobsVisible.find((job) => job.id === pickedJobId) ?? null
    : null
  const jobKind = (kind: string | undefined) =>
    kind === 'shell' ? 'shell' : kind === 'run_optimize' ? 'optimize' : 'agent'

  const pageClass = 'inset-0 top-0 left-0 flex h-[100dvh] max-h-[100dvh] w-full max-w-none translate-x-0 translate-y-0 flex-col gap-3 rounded-none'
  const pageStyle = { inset: 0, transform: 'none', width: '100%', maxWidth: 'none', height: '100dvh', maxHeight: '100dvh', borderRadius: 0 }

  return (
    <Dialog open={queueDialogOpen} onOpenChange={setQueueDialogOpen}>
      <DialogContent className="max-w-[920px]">
        <DialogTitle>Run queue</DialogTitle>
        <DialogDescription className="mb-3">
          Pending and running agent work (FIFO per persona). Queued items can be removed
          immediately; running items can be stopped.
        </DialogDescription>
        <div className="mb-2 flex items-center gap-3">
          <label className="flex cursor-pointer items-center gap-2 text-sm">
            <input
              type="checkbox"
              checked={queueShowAllPersonas}
              onChange={(e) => setQueueShowAllPersonas(e.target.checked)}
            />
            All personas
          </label>
        </div>
        <div
          className="max-h-[min(420px,60vh)] overflow-auto rounded-md border p-2"
          style={{ borderColor }}
        >
          {queueDialogItems.length === 0 ? (
            <div className="flex flex-col gap-2">
              <span>No queued runs (lane idle or diagnostics loading).</span>
              <Button
                size="sm"
                variant="secondary"
                className="cursor-pointer self-start"
                onClick={() => {
                  setQueueDialogOpen(false)
                  queue.onOpenSchedules()
                }}
              >
                Open schedules
              </Button>
            </div>
          ) : (
            <>
              <table className="hidden w-full border-collapse text-left text-sm md:table">
                <thead>
                  <tr className="text-[color:var(--mc-text-muted)]">
                    <th className="p-1 pr-2">#</th>
                    <th className="p-1 pr-2">State</th>
                    <th className="p-1 pr-2">Persona</th>
                    <th className="p-1 pr-2">Source</th>
                    <th className="p-1 pr-2">Context</th>
                    <th className="p-1 pr-2">Elapsed</th>
                    <th className="p-1 text-right"> </th>
                  </tr>
                </thead>
                <tbody>
                  {queueDialogItems.map((it) => {
                    const isStopping = stoppingRunIds.includes(it.run_id)
                    const isRunning = it.state === 'running'
                    return (
                      <tr
                        key={it.run_id}
                        className="border-t border-[color:var(--mc-border-soft)] align-top"
                      >
                        <td className="p-1 pr-2 font-mono text-xs">{it.position}</td>
                        <td className="p-1 pr-2">{it.state}</td>
                        <td className="p-1 pr-2">{it.persona_name}</td>
                        <td className="p-1 pr-2">{it.source}</td>
                        <td className="max-w-[280px] break-words p-1 pr-2" title={it.label}>
                          {it.label || '-'}
                        </td>
                        <td
                          className="p-1 pr-2 font-mono text-xs"
                          title={elapsedTitle(it.state)}
                        >
                          {elapsedText(it.elapsed_ms, sampledAtMs, now)}
                        </td>
                        <td className="p-1 text-right">
                          <Button
                            size="sm"
                            variant="destructive"
                            disabled={isStopping}
                            onClick={() => void handleQueueAction(it.run_id, it.state)}
                          >
                            {isStopping
                              ? isRunning
                                ? 'Stopping...'
                                : 'Removing...'
                              : isRunning
                                ? 'Stop'
                                : 'Remove'}
                          </Button>
                        </td>
                      </tr>
                    )
                  })}
                </tbody>
              </table>
              <div className="flex flex-col gap-2 md:hidden">
                {queueDialogItems.map((it) => (
                  <button
                    key={it.run_id}
                    type="button"
                    className="rounded-lg border border-[color:var(--mc-border-soft)] p-3 text-left text-sm"
                    onClick={() => {
                      setPickedJobId(null)
                      setPickedRunId(it.run_id)
                    }}
                  >
                    <span className="block font-semibold">#{it.position} · {it.state}</span>
                    <span className="block truncate">{it.label || it.persona_name}</span>
                  </button>
                ))}
              </div>
            </>
          )}
        </div>
        <span className="mb-1 mt-3 block text-sm font-medium">Background jobs</span>
        <span className="mb-2 block text-xs text-muted-foreground">
          Recent background jobs for this chat. Stop requests cooperative cancellation.
        </span>
        <div
          className="max-h-[min(320px,45vh)] overflow-auto rounded-md border p-2"
          style={{ borderColor }}
        >
          {backgroundJobsVisible.length === 0 ? (
            <span>No background jobs found for this chat.</span>
          ) : (
            <>
              <table className="hidden w-full border-collapse text-left text-sm md:table">
                <thead>
                  <tr className="text-[color:var(--mc-text-muted)]">
                    <th className="p-1 pr-2">Status</th>
                    <th className="p-1 pr-2">Kind</th>
                    <th className="p-1 pr-2">ID</th>
                    <th className="p-1 pr-2">Label</th>
                    <th className="p-1 pr-2">Updated</th>
                    <th className="p-1 text-right"> </th>
                  </tr>
                </thead>
                <tbody>
                  {backgroundJobsVisible.map((job) => {
                    const isActive = isActiveBackgroundJobStatus(job.status)
                    const isStopping = stoppingBackgroundJobIds.includes(job.id)
                    const updatedAt = job.finished_at || job.started_at || job.created_at
                    return (
                      <tr
                        key={job.id}
                        className="border-t border-[color:var(--mc-border-soft)] align-top"
                      >
                        <td className="p-1 pr-2">{job.status}</td>
                        <td className="p-1 pr-2 text-xs">
                          {job.job_kind === 'shell'
                            ? 'shell'
                            : job.job_kind === 'run_optimize'
                              ? 'optimize'
                              : 'agent'}
                        </td>
                        <td className="p-1 pr-2 font-mono text-xs">{job.id}</td>
                        <td
                          className="max-w-[260px] break-words p-1 pr-2"
                          title={job.label || job.prompt}
                        >
                          {job.label || job.prompt || '-'}
                        </td>
                        <td className="p-1 pr-2 text-xs">
                          {updatedAt ? new Date(updatedAt).toLocaleString() : '-'}
                        </td>
                        <td className="p-1 text-right">
                          {isActive ? (
                            <Button
                              size="sm"
                              variant="destructive"
                              disabled={isStopping}
                              onClick={() => void handleBackgroundJobStop(job.id)}
                            >
                              {isStopping ? 'Stopping...' : 'Stop'}
                            </Button>
                          ) : (
                            <span className="text-muted-foreground">—</span>
                          )}
                        </td>
                      </tr>
                    )
                  })}
                </tbody>
              </table>
              <div className="flex flex-col gap-2 md:hidden">
                {backgroundJobsVisible.map((job) => (
                  <button
                    key={job.id}
                    type="button"
                    className="rounded-lg border border-[color:var(--mc-border-soft)] p-3 text-left text-sm"
                    onClick={() => {
                      setPickedRunId(null)
                      setPickedJobId(job.id)
                    }}
                  >
                    <span className="block font-semibold">{job.status} · {jobKind(job.job_kind)}</span>
                    <span className="block truncate">{job.label || job.prompt || job.id}</span>
                  </button>
                ))}
              </div>
            </>
          )}
        </div>
        <div className="mt-3 flex justify-end">
          <DialogClose asChild>
            <Button variant="secondary">Close</Button>
          </DialogClose>
        </div>
      </DialogContent>
      {pickedRun ? (
        <Dialog open onOpenChange={(open) => { if (!open) setPickedRunId(null) }}>
          <DialogContent className={pageClass} style={pageStyle}>
            <Button type="button" variant="outline" size="sm" className="self-start" onClick={() => setPickedRunId(null)}>Back</Button>
            <DialogTitle>#{pickedRun.position} · {pickedRun.state}</DialogTitle>
            <p className="m-0">{pickedRun.persona_name} · {pickedRun.source}</p>
            <p className="m-0 break-words">{pickedRun.label || '-'}</p>
            <p className="m-0 font-mono" title={elapsedTitle(pickedRun.state)}>{elapsedText(pickedRun.elapsed_ms, sampledAtMs, now)}</p>
            <Button
              size="sm"
              variant="destructive"
              className="self-start"
              disabled={stoppingRunIds.includes(pickedRun.run_id)}
              onClick={() => void handleQueueAction(pickedRun.run_id, pickedRun.state)}
            >
              {stoppingRunIds.includes(pickedRun.run_id)
                ? pickedRun.state === 'running' ? 'Stopping...' : 'Removing...'
                : pickedRun.state === 'running' ? 'Stop' : 'Remove'}
            </Button>
          </DialogContent>
        </Dialog>
      ) : null}
      {pickedJob ? (
        <Dialog open onOpenChange={(open) => { if (!open) setPickedJobId(null) }}>
          <DialogContent className={pageClass} style={pageStyle}>
            <Button type="button" variant="outline" size="sm" className="self-start" onClick={() => setPickedJobId(null)}>Back</Button>
            <DialogTitle>{pickedJob.status} · {jobKind(pickedJob.job_kind)}</DialogTitle>
            <p className="m-0 font-mono">{pickedJob.id}</p>
            <p className="m-0 break-words">{pickedJob.prompt || '-'}</p>
            <p className="m-0">{(pickedJob.finished_at || pickedJob.started_at || pickedJob.created_at)
              ? new Date(pickedJob.finished_at || pickedJob.started_at || pickedJob.created_at).toLocaleString()
              : '-'}</p>
            {isActiveBackgroundJobStatus(pickedJob.status) ? (
              <Button
                size="sm"
                variant="destructive"
                className="self-start"
                disabled={stoppingBackgroundJobIds.includes(pickedJob.id)}
                onClick={() => void handleBackgroundJobStop(pickedJob.id)}
              >
                {stoppingBackgroundJobIds.includes(pickedJob.id) ? 'Stopping...' : 'Stop'}
              </Button>
            ) : null}
          </DialogContent>
        </Dialog>
      ) : null}
    </Dialog>
  )
}
