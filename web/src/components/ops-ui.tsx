import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogTitle, DialogDescription, DialogClose } from '@/components/ui/dialog'
import React from 'react'
import type { InstallationStatus, QueueLane } from '../types'

export type CockpitStatusChipProps = {
  queueLane: QueueLane | null
  backgroundActiveCount: number
  installationStatus: InstallationStatus | null
  statusText: string
  onClick?: () => void
  className?: string
}

export function CockpitStatusChip({
  queueLane,
  backgroundActiveCount,
  installationStatus,
  statusText,
  onClick,
  className,
}: CockpitStatusChipProps) {
  const pending = queueLane?.pending ?? 0
  const setupOk =
    installationStatus?.llm_ready === true && installationStatus?.channel_ready === true
  const parts: string[] = []
  if (pending > 0) parts.push(`Queue: ${pending}`)
  if (backgroundActiveCount > 0) parts.push(`Jobs: ${backgroundActiveCount}`)
  parts.push(setupOk ? 'Setup OK' : 'Setup needed')
  const busy =
    statusText.startsWith('Uploading') ||
    statusText.startsWith('Sending') ||
    statusText === 'Queued' ||
    statusText.startsWith('Loading quote')
  if (busy && statusText !== 'Idle') {
    parts.unshift(statusText)
  }

  return (
    <button
      type="button"
      className={`mc-cockpit-status-chip cursor-pointer ${className ?? ''}`.trim()}
      onClick={onClick}
      aria-label={`Session status: ${parts.join(', ')}. Expand for details.`}
      title="Session status"
    >
      {parts.join(' · ')}
    </button>
  )
}

export type MobileOpsSheetProps = {
  open: boolean
  onOpenChange: (open: boolean) => void
  onOpenQueue: () => void
  onOpenInbox: () => void
  inboxBadgeCount?: number
  onOpenSchedules: () => void
  onOpenPrinciples: () => void
  onOpenArtifacts: () => void
  onOpenMemory: () => void
  onOpenAgentHistory: () => void
  onOpenTerminal?: () => void
  terminalAvailable?: boolean
  agentHistoryDisabled?: boolean
}

export function MobileOpsSheet({
  open,
  onOpenChange,
  onOpenQueue,
  onOpenInbox,
  inboxBadgeCount = 0,
  onOpenSchedules,
  onOpenPrinciples,
  onOpenArtifacts,
  onOpenMemory,
  onOpenAgentHistory,
  onOpenTerminal,
  terminalAvailable = false,
  agentHistoryDisabled,
}: MobileOpsSheetProps) {
  const pick = (fn: () => void) => () => {
    onOpenChange(false)
    fn()
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="mc-mobile-ops-sheet max-w-[400px]">
        <DialogTitle>Operator tools</DialogTitle>
        <DialogDescription className="mb-3">
          Queue, schedules, memory, and diagnostics.
        </DialogDescription>
        <div className="flex flex-col gap-2">
          <button
            type="button"
            className="mc-inbox-launch mc-inbox-launch--sheet cursor-pointer"
            data-has-items={inboxBadgeCount > 0 ? 'true' : 'false'}
            onClick={pick(onOpenInbox)}
          >
            Inbox
            {inboxBadgeCount > 0 ? (
              <span className="mc-inbox-launch__badge" aria-hidden>
                {inboxBadgeCount > 99 ? '99+' : inboxBadgeCount}
              </span>
            ) : null}
          </button>
          <Button variant="secondary" className="cursor-pointer justify-start" onClick={pick(onOpenQueue)}>
            Queue
          </Button>
          <Button variant="secondary" className="cursor-pointer justify-start" onClick={pick(onOpenSchedules)}>
            Schedules
          </Button>
          <Button variant="secondary" className="cursor-pointer justify-start" onClick={pick(onOpenPrinciples)}>
            Principles
          </Button>
          <Button variant="secondary" className="cursor-pointer justify-start" onClick={pick(onOpenArtifacts)}>
            Artifacts
          </Button>
          {terminalAvailable && onOpenTerminal ? (
            <Button variant="secondary" className="cursor-pointer justify-start" onClick={pick(onOpenTerminal)}>
              Terminal
            </Button>
          ) : null}
          <Button variant="secondary" className="cursor-pointer justify-start" onClick={pick(onOpenMemory)}>
            Memory
          </Button>
          <Button
            variant="secondary"
            className="cursor-pointer justify-start"
            disabled={agentHistoryDisabled}
            onClick={pick(onOpenAgentHistory)}
          >
            Last agent run
          </Button>
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

export type ShortcutsDialogProps = {
  open: boolean
  onOpenChange: (open: boolean) => void
}

const SHORTCUTS = [
  { keys: 'Cmd/Ctrl + K', action: 'Open command palette' },
  { keys: '?', action: 'Show keyboard shortcuts' },
  { keys: '/', action: 'Focus message composer' },
  { keys: 'Esc', action: 'Dismiss quoted reply' },
  { keys: 'Cmd/Ctrl + Enter', action: 'Send message (in composer)' },
  { keys: 'Cmd/Ctrl + Enter', action: 'Create session (in new session dialog)' },
]

export function ShortcutsDialog({ open, onOpenChange }: ShortcutsDialogProps) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-[440px]">
        <DialogTitle>Keyboard shortcuts</DialogTitle>
        <DialogDescription className="mb-3">
          Desktop only. Composer must be focused for send shortcut.
        </DialogDescription>
        <ul className="mc-shortcuts-list">
          {SHORTCUTS.map((row, i) => (
            <li key={`${row.keys}-${i}`} className="mc-shortcuts-row">
              <kbd className="mc-shortcuts-kbd">{row.keys}</kbd>
              <span>{row.action}</span>
            </li>
          ))}
        </ul>
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
