import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { Select, SelectTrigger, SelectContent, SelectItem, SelectValue } from '@/components/ui/select'
import { Dialog, DialogContent, DialogTitle, DialogDescription, DialogClose } from '@/components/ui/dialog'
import React from 'react'

import type { SchedulesDialogProps } from './types'

export function SchedulesDialog(props: SchedulesDialogProps) {
  const { appearance, activePersonaId, personas, schedules } = props
  const schedulesDialogOpen = schedules.open
  const setSchedulesDialogOpen = schedules.onOpenChange
  const schedulesShowArchived = schedules.showArchived
  const setSchedulesShowArchived = schedules.setShowArchived
  const schedulesFiltered = schedules.filtered
  const schedulesList = schedules.schedules
  const newSchedulePrompt = schedules.newPrompt
  const setNewSchedulePrompt = schedules.setNewPrompt
  const newScheduleType = schedules.newType
  const setNewScheduleType = schedules.setNewType
  const newScheduleValue = schedules.newValue
  const setNewScheduleValue = schedules.setNewValue
  const newSchedulePersonaId = schedules.newPersonaId
  const setNewSchedulePersonaId = schedules.setNewPersonaId
  const createSchedule = schedules.createSchedule
  const updateSchedule = schedules.updateSchedule
  const openScheduleDetail = schedules.openDetail

  const borderStyle =
    appearance === 'dark'
      ? { borderColor: 'var(--mc-border-soft)', background: 'var(--mc-bg-panel)' }
      : { borderColor: 'var(--gray-6)', background: 'var(--gray-2)' }
  const rowBorder =
    appearance === 'dark' ? { borderColor: 'var(--mc-border-soft)' } : { borderColor: 'var(--gray-6)' }

  return (
    <Dialog open={schedulesDialogOpen} onOpenChange={(open) => setSchedulesDialogOpen(open)}>
      <DialogContent className="flex w-[min(96vw,920px)] max-w-[920px] flex-col gap-3 overflow-hidden p-4 sm:max-w-[920px]">
          <DialogTitle>Schedules</DialogTitle>
          <DialogDescription>
            Create and manage scheduled prompts for this chat.
          </DialogDescription>

          <div className="flex flex-wrap items-center justify-between gap-3">
            <span className="font-medium">Active schedules</span>
            <label htmlFor="sched-archived" className="flex cursor-pointer items-center gap-2 text-sm">
              <span>Show completed / cancelled</span>
              <Switch
                id="sched-archived"
                checked={schedulesShowArchived}
                onCheckedChange={setSchedulesShowArchived}
              />
            </label>
          </div>

          <div
            className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden rounded-lg border p-3"
            style={borderStyle}
          >
            <ul className="mb-3 min-h-0 min-w-0 flex-1 list-none space-y-3 overflow-y-auto overflow-x-hidden overscroll-contain pr-1">
              {schedulesFiltered.length === 0 ? (
                <li className="rounded-lg border border-dashed px-4 py-10 text-center" style={rowBorder}>
                  <span>
                    {schedulesList.length === 0
                      ? 'No schedules yet. Add one below.'
                      : 'No active schedules. Enable “Show completed / cancelled” to see finished runs.'}
                  </span>
                </li>
              ) : null}
              {schedulesFiltered.map((t) => (
                <li
                  key={t.id}
                  className="mc-schedule-row flex min-w-0 flex-col gap-2 rounded-lg border p-3"
                  style={rowBorder}
                >
                  <p className="mc-schedule-prompt m-0 min-w-0 truncate text-sm leading-snug" title={t.prompt}>
                    {t.prompt}
                  </p>
                  <div className="flex min-w-0 flex-wrap items-center gap-2">
                    <Select
                      value={String(t.persona_id)}
                      onValueChange={(v) => void updateSchedule(t.id, { persona_id: Number(v) })}
                    >
                      <SelectTrigger className="w-[120px]">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {personas.map((p) => (
                          <SelectItem key={p.id} value={String(p.id)}>
                            {p.name}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                    <span className="shrink-0 text-xs text-muted-foreground">
                      {t.schedule_type} · {t.next_run ?? '-'}
                    </span>
                    <span className="shrink-0 text-xs capitalize text-muted-foreground">
                      {t.status === 'running' ? 'active' : t.status}
                    </span>
                    <div className="ml-auto flex flex-wrap gap-1.5">
                      <Button size="sm" variant="secondary" onClick={() => openScheduleDetail(t)}>
                        Details
                      </Button>
                      {t.status === 'active' ? (
                        <Button
                          size="sm"
                          variant="secondary"
                          onClick={() => void updateSchedule(t.id, { status: 'paused' })}
                        >
                          Pause
                        </Button>
                      ) : t.status === 'paused' ? (
                        <Button
                          size="sm"
                          variant="secondary"
                          onClick={() => void updateSchedule(t.id, { status: 'active' })}
                        >
                          Resume
                        </Button>
                      ) : null}
                      {t.status !== 'cancelled' ? (
                        <Button
                          size="sm"
                          variant="destructive"
                          onClick={() => void updateSchedule(t.id, { status: 'cancelled' })}
                        >
                          Cancel
                        </Button>
                      ) : null}
                    </div>
                  </div>
                </li>
              ))}
            </ul>

            <div className="flex min-w-0 flex-wrap items-end gap-2 border-t pt-3" style={rowBorder}>
              <Input
                placeholder="Prompt"
                value={newSchedulePrompt}
                onChange={(e) => setNewSchedulePrompt(e.target.value)}
                className="min-w-0 flex-1 basis-[16rem]"
              />
              <Select value={newScheduleType} onValueChange={(v) => setNewScheduleType(v as 'cron' | 'once')}>
                <SelectTrigger className="w-[100px]">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="cron">Cron</SelectItem>
                  <SelectItem value="once">Once</SelectItem>
                </SelectContent>
              </Select>
              <Input
                placeholder={newScheduleType === 'cron' ? '0 9 * * *' : '2025-12-31T09:00:00Z'}
                value={newScheduleValue}
                onChange={(e) => setNewScheduleValue(e.target.value)}
                className="min-w-0 basis-[12rem] sm:w-[200px]"
              />
              <Select
                value={newSchedulePersonaId != null ? String(newSchedulePersonaId) : ''}
                onValueChange={(v) => setNewSchedulePersonaId(Number(v))}
              >
                <SelectTrigger className="w-[140px]">
                  <SelectValue placeholder="Persona" />
                </SelectTrigger>
                <SelectContent>
                  {personas.map((p) => (
                    <SelectItem key={p.id} value={String(p.id)}>
                      {p.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <Button
                size="sm"
                onClick={() => {
                  if (newSchedulePrompt.trim()) {
                    void createSchedule(
                      newSchedulePrompt.trim(),
                      newScheduleType,
                      newScheduleValue,
                      newSchedulePersonaId ?? activePersonaId,
                    )
                    setNewSchedulePrompt('')
                  }
                }}
              >
                Add
              </Button>
            </div>
          </div>

          <div className="flex justify-end gap-2">
            <DialogClose>
              <Button variant="secondary">Close</Button>
            </DialogClose>
          </div>
        </DialogContent>
    </Dialog>
  )
}
