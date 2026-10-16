import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { Select, SelectTrigger, SelectContent, SelectItem, SelectValue } from '@/components/ui/select'
import { Dialog, DialogContent, DialogTitle, DialogDescription, DialogClose } from '@/components/ui/dialog'
import React from 'react'

import type { ScheduleDetailDialogProps } from './types'

export function ScheduleDetailDialog(props: ScheduleDetailDialogProps) {
const { appearance, personas, scheduleDetail } = props
  const scheduleDetailTask = scheduleDetail.task
  const scheduleDetailPrompt = scheduleDetail.prompt
  const setScheduleDetailPrompt = scheduleDetail.setPrompt
  const scheduleDetailScheduleType = scheduleDetail.scheduleType
  const setScheduleDetailScheduleType = scheduleDetail.setScheduleType
  const scheduleDetailScheduleValue = scheduleDetail.scheduleValue
  const setScheduleDetailScheduleValue = scheduleDetail.setScheduleValue
  const scheduleDetailBusy = scheduleDetail.busy
  const setScheduleDetailBusy = scheduleDetail.setBusy
  const updateScheduleDetail = scheduleDetail.updateSchedule
  return (
    <Dialog
      open={scheduleDetailTask != null}
      onOpenChange={scheduleDetail.onOpenChange}
    >
      <DialogContent className="max-w-[720px]">
        <DialogTitle>
          {scheduleDetailTask != null ? `Schedule #${scheduleDetailTask.id}` : 'Schedule'}
        </DialogTitle>
        <DialogDescription className="mb-3">
          View metadata, edit the prompt, or change the cron/once expression (server runs the same preflight as new schedules).
        </DialogDescription>
        {scheduleDetailTask != null ? (
          <>
            <div className="mb-3 grid grid-cols-[120px_minmax(0,1fr)] gap-x-3 gap-y-1 text-sm">
              <span className="block">Persona</span>
              <span className="block">
                {personas.find((p) => p.id === scheduleDetailTask.persona_id)?.name ?? scheduleDetailTask.persona_id}
              </span>
              <span className="block">Type</span>
              <span className="block">{scheduleDetailTask.schedule_type}</span>
              <span className="block">Schedule</span>
              <span className="block break-all">{scheduleDetailTask.schedule_value}</span>
              <span className="block">Next run</span>
              <span className="block break-all">{scheduleDetailTask.next_run ?? '-'}</span>
              <span className="block">Last run</span>
              <span className="block break-all">{scheduleDetailTask.last_run ?? '-'}</span>
              <span className="block">Status</span>
              <span className="block">{scheduleDetailTask.status}</span>
              <span className="block">Created</span>
              <span className="block break-all">{scheduleDetailTask.created_at ?? '-'}</span>
            </div>
            <span className="mb-1 block font-semibold">Prompt</span>
            <textarea
              value={scheduleDetailPrompt}
              onChange={(e) => setScheduleDetailPrompt(e.target.value)}
              spellCheck={false}
              disabled={scheduleDetailTask.status === 'cancelled'}
              className={appearance === 'dark'
                ? 'min-h-[160px] w-full rounded-md border border-[color:var(--mc-border-soft)] bg-[color:var(--mc-bg-panel)] p-3 font-mono text-xs text-[color:var(--mc-text-primary)]'
                : 'min-h-[160px] w-full rounded-md border border-[color:var(--mc-border-strong)] bg-[color:var(--mc-surface-elevated)] p-3 font-mono text-xs text-[color:var(--mc-text-primary)]'}
            />
            <span className="mb-1 mt-3 block font-semibold">Schedule</span>
            <div className="flex gap-2 items-center flex-wrap mb-2">
              <Select
                value={scheduleDetailScheduleType}
                onValueChange={(v) => setScheduleDetailScheduleType(v as 'cron' | 'once')}
                disabled={scheduleDetailTask.status === 'cancelled'}
              >
                <SelectTrigger className="w-[100px]" ><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="cron">Cron</SelectItem>
                  <SelectItem value="once">Once</SelectItem>
                </SelectContent>
              </Select>
              <input
                type="text"
                value={scheduleDetailScheduleValue}
                onChange={(e) => setScheduleDetailScheduleValue(e.target.value)}
                spellCheck={false}
                disabled={scheduleDetailTask.status === 'cancelled'}
                placeholder={scheduleDetailScheduleType === 'cron' ? '0 9 * * * *' : '2099-12-31T23:59:59+00:00'}
                className={appearance === 'dark'
                  ? 'min-w-[200px] flex-1 rounded-md border border-[color:var(--mc-border-soft)] bg-[color:var(--mc-bg-panel)] px-2 py-1 font-mono text-xs text-[color:var(--mc-text-primary)]'
                  : 'min-w-[200px] flex-1 rounded-md border border-[color:var(--mc-border-strong)] bg-[color:var(--mc-surface-elevated)] px-2 py-1 font-mono text-xs text-[color:var(--mc-text-primary)]'}
              />
            </div>
            <div className="flex justify-end gap-2 mt-3 flex-wrap">
              <DialogClose>
                <Button variant="secondary" size="sm">Close</Button>
              </DialogClose>
              <Button
                size="sm"
                disabled={
                  scheduleDetailBusy
                  || scheduleDetailTask.status === 'cancelled'
                  || (
                    scheduleDetailScheduleType === (scheduleDetailTask.schedule_type === 'once' ? 'once' : 'cron')
                    && scheduleDetailScheduleValue.trim() === scheduleDetailTask.schedule_value.trim()
                  )
                  || scheduleDetailScheduleValue.trim().length === 0
                }
                onClick={() => {
                  if (scheduleDetailTask == null) return
                  setScheduleDetailBusy(true)
                  updateScheduleDetail(scheduleDetailTask.id, {
                    schedule_type: scheduleDetailScheduleType,
                    schedule_value: scheduleDetailScheduleValue.trim(),
                  })
                    .then(() => scheduleDetail.close())
                    .catch(() => { /* api throws */ })
                    .finally(() => setScheduleDetailBusy(false))
                }}
              >
                {scheduleDetailBusy ? 'Saving…' : 'Save schedule'}
              </Button>
              <Button
                size="sm"
                disabled={
                  scheduleDetailBusy
                  || scheduleDetailTask.status === 'cancelled'
                  || scheduleDetailPrompt.trim() === scheduleDetailTask.prompt.trim()
                  || scheduleDetailPrompt.trim().length === 0
                }
                onClick={() => {
                  if (scheduleDetailTask == null) return
                  setScheduleDetailBusy(true)
                  updateScheduleDetail(scheduleDetailTask.id, { prompt: scheduleDetailPrompt.trim() })
                    .then(() => scheduleDetail.close())
                    .catch(() => { /* api throws */ })
                    .finally(() => setScheduleDetailBusy(false))
                }}
              >
                {scheduleDetailBusy ? 'Saving…' : 'Save prompt'}
              </Button>
            </div>
          </>
        ) : null}
      </DialogContent>
    </Dialog>
  )
}
