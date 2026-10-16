import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { Select } from '@/components/ui/select'
import { Dialog, DialogContent, DialogTitle, DialogDescription, DialogClose } from '@/components/ui/dialog'
import React from 'react'

import type { AgentsMdDialogProps } from './types'

export function AgentsMdDialog(props: AgentsMdDialogProps) {
const { appearance, agentsMd } = props
  const agentsMdOpen = agentsMd.open
  const setAgentsMdOpen = agentsMd.onOpenChange
  const agentsMdPath = agentsMd.path
  const agentsMdError = agentsMd.error
  const setAgentsMdError = agentsMd.setError
  const setAgentsMdBusy = agentsMd.setBusy
  const agentsMdContent = agentsMd.content
  const setAgentsMdContent = agentsMd.setContent
  const agentsMdMtimeMs = agentsMd.mtimeMs
  const agentsMdBusy = agentsMd.busy
  const loadWorkspaceAgentsMd = agentsMd.load
  const saveWorkspaceAgentsMd = agentsMd.save
  return (
    <Dialog
      open={agentsMdOpen}
      onOpenChange={agentsMd.onOpenChange}
    >
      <DialogContent className="max-w-[900px]">
        <DialogTitle>Workspace principles (AGENTS.md)</DialogTitle>
        <DialogDescription className="mb-3">
          Shared agent principles for this workspace. Same file the bot loads from your configured workspace path.
        </DialogDescription>
        {agentsMdPath ? (
          <span className="mb-2 block break-all">
            {agentsMdPath}
          </span>
        ) : null}
        {agentsMdError ? (
          <div role="status" className="rounded-lg border border-destructive/30 bg-destructive/10 p-3 text-sm text-destructive mb-2">{agentsMdError}</div>
        ) : null}
        <textarea
          value={agentsMdContent}
          onChange={(e) => setAgentsMdContent(e.target.value)}
          spellCheck={false}
          className={appearance === 'dark'
            ? 'h-[420px] w-full rounded-md border border-[color:var(--mc-border-soft)] bg-[color:var(--mc-bg-panel)] p-3 font-mono text-xs text-[color:var(--mc-text-primary)]'
            : 'h-[420px] w-full rounded-md border border-[color:var(--mc-border-strong)] bg-[color:var(--mc-surface-elevated)] p-3 font-mono text-xs text-[color:var(--mc-text-primary)]'}
        />
        <div className="flex justify-between items-center mt-3 flex-wrap gap-2">
          <span>
            {agentsMdMtimeMs != null ? `mtime: ${agentsMdMtimeMs}` : ''}
          </span>
          <div className="flex gap-2">
            <Button size="sm" variant="secondary" onClick={() => void loadWorkspaceAgentsMd()} disabled={agentsMdBusy}>
              Reload
            </Button>
            <Button size="sm" onClick={() => void saveWorkspaceAgentsMd()} disabled={agentsMdBusy}>
              {agentsMdBusy ? 'Saving…' : 'Save'}
            </Button>
            <DialogClose>
              <Button size="sm" variant="secondary">Close</Button>
            </DialogClose>
          </div>
        </div>
      </DialogContent>
    </Dialog>
    
  )
}
