import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { Select } from '@/components/ui/select'
import { Dialog, DialogContent, DialogTitle, DialogDescription, DialogClose } from '@/components/ui/dialog'
import React from 'react'

import { SettingsPanelSkeleton } from '../../components/skeleton'
const TerminalPane = React.lazy(() =>
  import('../../components/terminal-pane').then((m) => ({ default: m.TerminalPane })),
)

import type { TerminalDialogProps } from './types'

export function TerminalDialog(props: TerminalDialogProps) {
const { terminal } = props
  const terminalDialogOpen = terminal.open
  const setTerminalDialogOpen = terminal.onOpenChange
  const terminalError = terminal.error
  const setTerminalError = terminal.setError
  return (
    <Dialog open={terminalDialogOpen} onOpenChange={setTerminalDialogOpen}>
      <DialogContent style={{ maxWidth: 1080, width: 'min(96vw, 1080px)' }} className="flex max-h-[min(88vh,900px)] flex-col">
        <DialogTitle>Terminal</DialogTitle>
        <DialogDescription className="mb-3">
          Interactive shell in the gateway workspace. Operator-only; requires WEB_AUTH_TOKEN and WEB_TERMINAL_ENABLED.
        </DialogDescription>
        {terminalError ? (
          <div role="status" className="rounded-lg border border-destructive/30 bg-destructive/10 p-3 text-sm text-destructive mb-2 shrink-0">{terminalError}</div>
        ) : null}
        <React.Suspense fallback={<SettingsPanelSkeleton />}>
          <TerminalPane
            active={terminalDialogOpen}
            onError={(message) => setTerminalError(message)}
          />
        </React.Suspense>
        <div className="flex justify-end mt-3 shrink-0">
          <DialogClose>
            <Button size="sm" variant="secondary">Close</Button>
          </DialogClose>
        </div>
      </DialogContent>
    </Dialog>
  )
}
