import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { Select } from '@/components/ui/select'
import { Dialog, DialogContent, DialogTitle, DialogDescription, DialogClose } from '@/components/ui/dialog'
import React from 'react'

import type { MemoryDialogProps } from './types'

export function MemoryDialog(props: MemoryDialogProps) {
const { appearance, activePersonaId, memory } = props
  const memoryDialogOpen = memory.open
  const setMemoryDialogOpen = memory.onOpenChange
  const setMemoryError = memory.setError
  const setMemoryBusy = memory.setBusy
  const memoryPathHint = memory.pathHint
  const memoryError = memory.error
  const memoryContent = memory.content
  const setMemoryContent = memory.setContent
  const memoryMtimeMs = memory.mtimeMs
  const memoryBusy = memory.busy
  const loadPersonaMemory = memory.load
  const savePersonaMemory = memory.save
  return (
    <Dialog
      open={memoryDialogOpen}
      onOpenChange={memory.onOpenChange}
    >
      <DialogContent className="max-w-[900px]">
        <DialogTitle>Persona memory</DialogTitle>
        <DialogDescription className="mb-3">
          Edit this persona’s tiered memory file. Memory is context, not a task queue.
        </DialogDescription>
    
        {memoryPathHint ? (
          <span className="mb-2 block">
            {memoryPathHint}
          </span>
        ) : null}
    
        {memoryError ? (
          <div role="status" className="rounded-lg border border-destructive/30 bg-destructive/10 p-3 text-sm text-destructive mb-2">{memoryError}</div>
        ) : null}
    
        <textarea
          value={memoryContent}
          onChange={(e) => setMemoryContent(e.target.value)}
          spellCheck={false}
          className={appearance === 'dark'
            ? 'h-[420px] w-full rounded-md border border-[color:var(--mc-border-soft)] bg-[color:var(--mc-bg-panel)] p-3 font-mono text-xs text-[color:var(--mc-text-primary)]'
            : 'h-[420px] w-full rounded-md border border-[color:var(--mc-border-strong)] bg-[color:var(--mc-surface-elevated)] p-3 font-mono text-xs text-[color:var(--mc-text-primary)]'}
        />
    
        <div className="flex justify-between items-center mt-3 flex-wrap gap-2">
          <span>
            {memoryMtimeMs != null ? `mtime: ${memoryMtimeMs}` : ''}
          </span>
          <div className="flex gap-2">
            <Button
              size="sm"
              variant="secondary"
              onClick={() => {
                if (activePersonaId != null) void loadPersonaMemory(activePersonaId)
              }}
              disabled={memoryBusy || activePersonaId == null}
            >
              Reload
            </Button>
            <Button
              size="sm"
              onClick={() => {
                if (activePersonaId != null) void savePersonaMemory(activePersonaId)
              }}
              disabled={memoryBusy || activePersonaId == null}
            >
              {memoryBusy ? 'Saving…' : 'Save'}
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
