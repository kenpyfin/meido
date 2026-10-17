import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogTitle, DialogDescription, DialogClose } from '@/components/ui/dialog'
import React from 'react'
import type { InstallationStatus, QueueLane } from '../types'
import { IconInbox } from './icons'

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

function OpsGlyph({ d }: { d: string }) {
  return (
    <svg className="size-5 shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d={d} />
    </svg>
  )
}

function OpsRow({
  icon,
  label,
  onClick,
  badge = 0,
  disabled = false,
}: {
  icon: React.ReactNode
  label: string
  onClick: () => void
  badge?: number
  disabled?: boolean
}) {
  return (
    <button
      type="button"
      className="flex h-12 w-full items-center gap-3 px-1 text-left text-sm text-[color:var(--mc-text-primary)] disabled:opacity-40"
      disabled={disabled}
      onClick={onClick}
    >
      {icon}
      <span className="min-w-0 flex-1">{label}</span>
      {badge > 0 ? (
        <span className="mc-inbox-launch__badge" aria-hidden>
          {badge > 99 ? '99+' : badge}
        </span>
      ) : null}
    </button>
  )
}

export type MobileOpsSheetProps = {
  open: boolean
  onOpenChange: (open: boolean) => void
  onOpenQueue: () => void
  onOpenInbox: () => void
  inboxBadgeCount?: number
  schedulesBadgeCount?: number
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
  schedulesBadgeCount = 0,
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
        <div className="flex flex-col">
          <OpsRow
            icon={<IconInbox className="size-5 shrink-0" />}
            label="Inbox"
            badge={inboxBadgeCount}
            onClick={pick(onOpenInbox)}
          />
          <OpsRow icon={<OpsGlyph d="M4 6h16M4 12h16M4 18h10" />} label="Queue" onClick={pick(onOpenQueue)} />
          <OpsRow icon={<OpsGlyph d="M8 7V3m8 4V3M4 11h16M6 5h12a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2z" />} label="Schedules" badge={schedulesBadgeCount} onClick={pick(onOpenSchedules)} />
          <OpsRow icon={<OpsGlyph d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8zM14 2v6h6M8 13h8M8 17h5" />} label="Principles" onClick={pick(onOpenPrinciples)} />
          <OpsRow icon={<OpsGlyph d="M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z" />} label="Artifacts" onClick={pick(onOpenArtifacts)} />
          {terminalAvailable && onOpenTerminal ? (
            <OpsRow icon={<OpsGlyph d="M4 17l6-5-6-5M12 19h8" />} label="Terminal" onClick={pick(onOpenTerminal)} />
          ) : null}
          <OpsRow icon={<OpsGlyph d="M12 3a9 9 0 1 0 9 9M12 7v5l3 2" />} label="Memory" onClick={pick(onOpenMemory)} />
          <OpsRow
            icon={<OpsGlyph d="M12 8v4l3 3M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0z" />}
            label="Last agent run"
            disabled={agentHistoryDisabled}
            onClick={pick(onOpenAgentHistory)}
          />
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
