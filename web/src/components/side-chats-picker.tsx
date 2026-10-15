import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuTrigger, DropdownMenuContent, DropdownMenuItem } from '@/components/ui/dropdown-menu'
import React, { useCallback, useState } from 'react'
import { ConfirmDialog } from './confirm-dialog'
import { IconSideChat } from './icons'
import type { ChatSession, SideChatSummary } from '../types'

type SideChatsPickerProps = {
  sideChats: SideChatSummary[]
  sessions: ChatSession[]
  activeSideChatId: string | null
  loading?: boolean
  onSelect: (sideChatId: string) => void
  onDelete: (sideChatId: string) => Promise<boolean>
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

function sideChatLabel(sc: SideChatSummary, sessions: ChatSession[]): string {
  const title = (sc.title || sc.anchor_snippet || 'Side chat').trim()
  const clipped = title.length > 48 ? `${title.slice(0, 45)}…` : title
  const sessionName = sc.session_id
    ? sessions.find((s) => s.id === sc.session_id)?.title
    : null
  if (sessionName) return `${clipped} · ${sessionName}`
  return clipped
}

export function SideChatsPicker({
  sideChats,
  sessions,
  activeSideChatId,
  loading,
  onSelect,
  onDelete,
}: SideChatsPickerProps) {
  const [deleteId, setDeleteId] = useState<string | null>(null)
  const [deleting, setDeleting] = useState(false)
  const [menuOpen, setMenuOpen] = useState(false)

  const handleConfirmDelete = useCallback(async () => {
    if (!deleteId) return
    setDeleting(true)
    try {
      const ok = await onDelete(deleteId)
      if (ok) setDeleteId(null)
    } finally {
      setDeleting(false)
    }
  }, [deleteId, onDelete])

  const count = sideChats.length
  const hasActive = activeSideChatId != null
  const deleteTarget = deleteId ? sideChats.find((s) => s.id === deleteId) : null

  return (
    <div className="flex items-center gap-1 mc-side-chats-picker" style={{ minWidth: 0 }}>
      <DropdownMenu open={menuOpen} onOpenChange={setMenuOpen}>
        <DropdownMenuTrigger>
          <Button
            variant="ghost"
            size="default"
            className="mc-side-chats-btn cursor-pointer"
            disabled={loading}
            title="Browse side chats"
            data-active={hasActive ? 'true' : 'false'}
          >
            <IconSideChat className="size-4 shrink-0" />
            <span className="mc-side-chats-btn-label">Side chats</span>
            {count > 0 ? (
              <span className="mc-badge-counter" aria-hidden>
                {count > 99 ? '99+' : count}
              </span>
            ) : null}
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent style={{ minWidth: 280, maxWidth: 380 }}>
          {sideChats.length === 0 ? (
            <DropdownMenuItem disabled>No side chats yet</DropdownMenuItem>
          ) : (
            sideChats.map((sc) => (
              <div key={sc.id} className="mc-side-chat-menu-row">
                <button
                  type="button"
                  className="mc-side-chat-menu-open"
                  onClick={() => {
                    setMenuOpen(false)
                    onSelect(sc.id)
                  }}
                >
                  <div className="truncate text-sm">
                    {activeSideChatId === sc.id ? '● ' : ''}
                    {sideChatLabel(sc, sessions)}
                  </div>
                  <div className="text-xs text-muted-foreground">
                    {formatRelativeTime(sc.updated_at)}
                    {sc.turn_count > 0 ? ` · ${sc.turn_count} turns` : ''}
                  </div>
                </button>
                <button
                  type="button"
                  className="mc-side-chat-menu-delete"
                  title="Delete side chat"
                  aria-label={`Delete ${sideChatLabel(sc, sessions)}`}
                  onClick={() => {
                    setMenuOpen(false)
                    setDeleteId(sc.id)
                  }}
                >
                  Delete
                </button>
              </div>
            ))
          )}
        </DropdownMenuContent>
      </DropdownMenu>

      <ConfirmDialog
        open={deleteId != null}
        onOpenChange={(open) => {
          if (!open) setDeleteId(null)
        }}
        title="Delete side chat?"
        description={
          deleteTarget
            ? `Remove “${(deleteTarget.title || deleteTarget.anchor_snippet || 'Side chat').slice(0, 80)}” and all of its turns. The main chat timeline is unchanged.`
            : 'This removes the side conversation and all of its turns. The main chat timeline is unchanged.'
        }
        confirmLabel="Delete"
        destructive
        loading={deleting}
        onConfirm={() => void handleConfirmDelete()}
      />
    </div>
  )
}
