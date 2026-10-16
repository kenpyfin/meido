import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import { Checkbox } from '@/components/ui/checkbox'
import { Select, SelectTrigger, SelectContent, SelectItem, SelectSeparator, SelectValue } from '@/components/ui/select'
import { DropdownMenu, DropdownMenuTrigger, DropdownMenuContent, DropdownMenuItem } from '@/components/ui/dropdown-menu'
import { Dialog, DialogContent, DialogTitle, DialogDescription, DialogClose } from '@/components/ui/dialog'
import React, { useCallback, useState } from 'react'
import { ConfirmDialog } from './confirm-dialog'
import { IconMoreVertical } from './icons'
import type { ChatSession } from '../types'

type SessionPickerProps = {
  sessions: ChatSession[]
  activeSessionId: string | null
  onSelectSession: (sessionId: string | null) => void
  onCreateSession: (intent: string, mirrorMainChat: boolean) => Promise<boolean>
  onDeleteSession: (sessionId: string) => Promise<void>
  loading?: boolean
  /** Tighter layout for header row */
  compact?: boolean
}

function formatRelativeTime(isoDate: string): string {
  const diff = Date.now() - new Date(isoDate).getTime()
  const mins = Math.floor(diff / 60000)
  if (mins < 1) return 'now'
  if (mins < 60) return `${mins}m ago`
  const hours = Math.floor(mins / 60)
  if (hours < 24) return `${hours}h ago`
  const days = Math.floor(hours / 24)
  return `${days}d ago`
}

export function SessionPicker({
  sessions,
  activeSessionId,
  onSelectSession,
  onCreateSession,
  onDeleteSession,
  loading,
  compact = false,
}: SessionPickerProps) {
  const [newDialogOpen, setNewDialogOpen] = useState(false)
  const [intentDraft, setIntentDraft] = useState('')
  const [mirrorMainChatDraft, setMirrorMainChatDraft] = useState(false)
  const [creating, setCreating] = useState(false)
  const [deleteConfirmOpen, setDeleteConfirmOpen] = useState(false)
  const [deleting, setDeleting] = useState(false)

  const handleCreate = useCallback(async () => {
    if (!intentDraft.trim()) return
    setCreating(true)
    try {
      await onCreateSession(intentDraft.trim(), mirrorMainChatDraft)
      setIntentDraft('')
      setMirrorMainChatDraft(false)
      setNewDialogOpen(false)
    } finally {
      setCreating(false)
    }
  }, [intentDraft, mirrorMainChatDraft, onCreateSession])

  const currentLabel = activeSessionId
    ? sessions.find((s) => s.id === activeSessionId)?.title || 'Session'
    : 'Main chat'
  const activeSession = activeSessionId
    ? sessions.find((s) => s.id === activeSessionId) ?? null
    : null

  const handleConfirmDelete = useCallback(async () => {
    if (!activeSessionId) return
    setDeleting(true)
    try {
      await onDeleteSession(activeSessionId)
      setDeleteConfirmOpen(false)
    } finally {
      setDeleting(false)
    }
  }, [activeSessionId, onDeleteSession])

  return (
    <div className="flex items-center mc-session-picker" data-compact={compact ? 'true' : 'false'}
      style={{ minWidth: 0 }}>
      <Select
        value={activeSessionId ?? '__main__'}
        onValueChange={(val) => onSelectSession(val === '__main__' ? null : val)}
        disabled={loading}
      >
        <SelectTrigger className="mc-session-picker-trigger cursor-pointer border-0 bg-transparent shadow-none hover:bg-muted">
          <SelectValue>{currentLabel}</SelectValue>
        </SelectTrigger>
        <SelectContent position="popper" sideOffset={4}>
          <SelectItem value="__main__">Main chat</SelectItem>
          {sessions.length > 0 && <SelectSeparator />}
          {sessions.map((s) => (
            <SelectItem key={s.id} value={s.id}>
              {s.title}
              {s.mirror_main_chat ? ' · main' : ''} ({formatRelativeTime(s.last_active_at)})
            </SelectItem>
          ))}
        </SelectContent>
      </Select>

      <Button
        variant="ghost"
        size="default"
        className="mc-session-picker-btn cursor-pointer"
        onClick={() => setNewDialogOpen(true)}
      >
        {compact ? (
          <>
            <span className="md:hidden">+</span>
            <span className="hidden md:inline">+ Session</span>
          </>
        ) : (
          '+ Session'
        )}
      </Button>

      {activeSessionId && activeSession ? (
        <DropdownMenu>
          <DropdownMenuTrigger>
            <Button
              variant="ghost"
              size="default"
              className="mc-session-picker-btn cursor-pointer"
              aria-label="Session actions"
            >
              <IconMoreVertical />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent>
            <DropdownMenuItem
              variant="destructive"
              onSelect={() => setDeleteConfirmOpen(true)}
            >
              Delete session
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      ) : null}

      <Dialog open={newDialogOpen} onOpenChange={setNewDialogOpen}>
        <DialogContent className="max-w-[420px]">
          <DialogTitle>New session</DialogTitle>
          <DialogDescription>
            Describe a focus area to spin up context from your vault and skills.
          </DialogDescription>
          <div className="flex flex-col gap-3 mt-4">
            <Textarea
              placeholder="e.g. Refactor the auth module to use JWT..."
              value={intentDraft}
              onChange={(e) => setIntentDraft(e.target.value)}
              rows={3}
              autoFocus
              onKeyDown={(e) => {
                if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
                  void handleCreate()
                }
              }}
            />
            <span className="text-xs text-muted-foreground">
              Max 500 characters. Press Cmd+Enter to create.
            </span>
            <label className="flex cursor-pointer gap-2 items-start text-sm">
              <Checkbox
                checked={mirrorMainChatDraft}
                onCheckedChange={(checked) => setMirrorMainChatDraft(checked === true)}
              />
              <span>
                Include messages in main chat
                <span className="mt-0.5 block text-xs text-muted-foreground">
                  Off by default — session history stays isolated unless you enable this.
                </span>
              </span>
            </label>
          </div>
          <div className="flex gap-3 mt-4 justify-end">
            <DialogClose>
              <Button variant="outline">
                Cancel
              </Button>
            </DialogClose>
            <Button
              onClick={() => void handleCreate()}
              disabled={!intentDraft.trim() || intentDraft.length > 500 || creating}
            >
              {creating ? 'Creating…' : 'Create session'}
            </Button>
          </div>
        </DialogContent>
      </Dialog>

      <ConfirmDialog
        open={deleteConfirmOpen}
        onOpenChange={setDeleteConfirmOpen}
        title="Delete session"
        description="This permanently removes the session and its messages from the database. This cannot be undone."
        confirmLabel="Delete session"
        destructive
        loading={deleting}
        onConfirm={handleConfirmDelete}
      />
    </div>
  )
}
