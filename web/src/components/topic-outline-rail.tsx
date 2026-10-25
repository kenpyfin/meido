import React from 'react'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { IconListTree, IconMoreVertical } from './icons'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from './ui/dropdown-menu'
import {
  filterTopicTree,
  formatTopicWhen,
  sideLabel,
  sortTopicsByTimeDesc,
  topicHasEarlierNeighbor,
  topicIdForTimestamp,
  type ConversationTopic,
} from '../lib/conversation-topics'

type TopicRow =
  | { kind: 'topic'; id: string; topic: ConversationTopic; index: number }
  | { kind: 'side'; id: string; topicId: string; sideChatId: string; anchorMessageId: string }

export type TopicOutlineTreeProps = {
  topics: ConversationTopic[]
  activeTopicId: string | null
  segmenting: boolean
  unsegmentedCount: number
  loading?: boolean
  error?: string
  filter: string
  onFilterChange: (value: string) => void
  onJump: (messageId: string) => void
  onSelectSideChat: (sideChatId: string) => void
  onRename: (topicId: string, title: string) => void
  onTogglePin: (topicId: string, pinned: boolean) => void
  onMerge: (topicId: string) => void
  onDelete: (topicId: string) => void
  onCreateSession: (topicId: string, title: string) => Promise<void>
  onRefresh: () => void
  collapsed?: boolean
  onToggleCollapsed?: () => void
  showCollapse?: boolean
}

export function TopicOutlineTree({
  topics,
  activeTopicId,
  segmenting,
  unsegmentedCount,
  loading = false,
  error = '',
  filter,
  onFilterChange,
  onJump,
  onSelectSideChat,
  onRename,
  onTogglePin,
  onMerge,
  onDelete,
  onCreateSession,
  onRefresh,
  collapsed = false,
  onToggleCollapsed,
  showCollapse = false,
}: TopicOutlineTreeProps) {
  const [expanded, setExpanded] = React.useState<Record<string, boolean>>({})
  const [cursor, setCursor] = React.useState(0)
  const [editingId, setEditingId] = React.useState<string | null>(null)
  const [draft, setDraft] = React.useState('')
  const [sessionTopicId, setSessionTopicId] = React.useState<string | null>(null)
  const [sessionTitle, setSessionTitle] = React.useState('')
  const [sessionBusy, setSessionBusy] = React.useState(false)
  const [sessionError, setSessionError] = React.useState('')
  const visible = React.useMemo(
    () => filterTopicTree(sortTopicsByTimeDesc(topics), filter),
    [filter, topics],
  )
  const rows = React.useMemo(() => flattenRows(visible, expanded), [expanded, visible])

  React.useEffect(() => {
    setCursor((index) => Math.min(index, Math.max(rows.length - 1, 0)))
  }, [rows.length])

  const activate = (row: TopicRow | undefined) => {
    if (!row) return
    if (row.kind === 'topic') onJump(row.topic.start_message_id)
    else onSelectSideChat(row.sideChatId)
  }

  const onKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (editingId || rows.length === 0) return
    const target = event.target
    if (target instanceof Element && target.closest('[data-slot="dropdown-menu-trigger"]')) return
    if (event.key === 'ArrowDown') {
      event.preventDefault()
      setCursor((index) => Math.min(index + 1, rows.length - 1))
    } else if (event.key === 'ArrowUp') {
      event.preventDefault()
      setCursor((index) => Math.max(index - 1, 0))
    } else if (event.key === 'Enter') {
      event.preventDefault()
      activate(rows[cursor])
    } else if (event.key === 'ArrowRight' || event.key === 'ArrowLeft') {
      const row = rows[cursor]
      if (!row || row.kind !== 'topic' || row.topic.side_chats.length === 0) return
      event.preventDefault()
      setExpanded((prev) => ({ ...prev, [row.id]: event.key === 'ArrowRight' }))
    }
  }

  if (collapsed) {
    return (
      <div className="mc-topic-rail mc-topic-rail-collapsed">
        <button
          type="button"
          className="mc-topic-rail-collapse"
          onClick={onToggleCollapsed}
          aria-expanded={false}
          aria-label="Show topics"
          title="Show topics"
        >
          <IconListTree />
        </button>
      </div>
    )
  }

  return (
    <div className="mc-topic-rail">
      <div className="mc-topic-rail-head">
        <span className="mc-topic-rail-title">Topics</span>
        <button type="button" className="mc-topic-rail-textbtn" onClick={onRefresh}>
          Refresh
        </button>
        {showCollapse ? (
          <button
            type="button"
            className="mc-topic-rail-textbtn"
            onClick={onToggleCollapsed}
            aria-expanded
            aria-label="Hide topics"
          >
            Hide
          </button>
        ) : null}
      </div>
      <input
        className="mc-topic-rail-filter"
        value={filter}
        onChange={(event) => onFilterChange(event.target.value)}
        placeholder="Filter titles"
        aria-label="Filter topic titles"
        type="search"
      />
      {loading ? <p className="mc-topic-rail-note">Loading topics…</p> : null}
      {error ? <p className="mc-topic-rail-note">{error}</p> : null}
      {segmenting ? <p className="mc-topic-rail-note">Updating topics…</p> : null}
      {!segmenting && unsegmentedCount > 0 ? (
        <p className="mc-topic-rail-note">{unsegmentedCount} new messages not titled yet</p>
      ) : null}
      {!loading && visible.length === 0 ? (
        <p className="mc-topic-rail-note">
          {topics.length === 0 ? 'Topics appear after a few messages.' : 'No matching titles.'}
        </p>
      ) : (
        <div
          className="mc-topic-rail-tree"
          role="tree"
          aria-label="Conversation topics"
          onKeyDown={onKeyDown}
        >
          {visible.map((topic) => {
            const open = expanded[topic.id] !== false && topic.side_chats.length > 0
            const rowIndex = rows.findIndex((row) => row.kind === 'topic' && row.id === topic.id)
            const selected = rowIndex === cursor
            return (
              <div key={topic.id} role="group">
                <div
                  role="treeitem"
                  aria-expanded={topic.side_chats.length > 0 ? open : undefined}
                  aria-current={activeTopicId === topic.id ? 'true' : undefined}
                  aria-selected={selected}
                  tabIndex={selected ? 0 : -1}
                  data-topic-id={topic.id}
                  className={
                    activeTopicId === topic.id
                      ? 'mc-topic-rail-item mc-topic-rail-item-active'
                      : 'mc-topic-rail-item'
                  }
                  onClick={() => {
                    setCursor(rowIndex)
                    onJump(topic.start_message_id)
                  }}
                >
                  {editingId === topic.id ? (
                    <input
                      className="mc-topic-rail-rename"
                      value={draft}
                      aria-label={`Rename ${topic.title}`}
                      onChange={(event) => setDraft(event.target.value)}
                      onClick={(event) => event.stopPropagation()}
                      onKeyDown={(event) => {
                        if (event.key === 'Enter') {
                          event.preventDefault()
                          event.stopPropagation()
                          const next = draft.trim()
                          if (next) onRename(topic.id, next)
                          setEditingId(null)
                        } else if (event.key === 'Escape') {
                          event.preventDefault()
                          event.stopPropagation()
                          setEditingId(null)
                        }
                      }}
                      autoFocus
                    />
                  ) : (
                    <>
                      <span className="mc-topic-rail-label">
                        {topic.pinned ? <span className="mc-topic-rail-pin">Pinned</span> : null}
                        <span className="mc-topic-rail-name">{topic.title}</span>
                        <span className="mc-topic-rail-meta">
                          {topic.message_count} · {formatTopicWhen(topic.start_ts)}
                        </span>
                      </span>
                      <TopicActionsMenu
                        title={topic.title}
                        pinned={topic.pinned}
                        canMerge={topicHasEarlierNeighbor(topics, topic.id)}
                        onRename={() => {
                          setEditingId(topic.id)
                          setDraft(topic.title)
                        }}
                        onTogglePin={() => onTogglePin(topic.id, !topic.pinned)}
                        onMerge={() => onMerge(topic.id)}
                        onDelete={() => onDelete(topic.id)}
                        onCreateSession={() => {
                          setSessionTopicId(topic.id)
                          setSessionTitle(topic.title)
                          setSessionError('')
                        }}
                      />
                    </>
                  )}
                </div>
                {open
                  ? topic.side_chats.map((side) => {
                      const sideIndex = rows.findIndex((row) => row.kind === 'side' && row.id === side.id)
                      return (
                        <div
                          key={side.id}
                          role="treeitem"
                          aria-selected={sideIndex === cursor}
                          tabIndex={sideIndex === cursor ? 0 : -1}
                          className="mc-topic-rail-side"
                          onClick={() => {
                            setCursor(sideIndex)
                            onSelectSideChat(side.id)
                          }}
                        >
                          {sideLabel(side)}
                          <span className="mc-topic-rail-meta">{side.turn_count}</span>
                        </div>
                      )
                    })
                  : null}
              </div>
            )
          })}
        </div>
      )}
      <NewTopicSessionDialog
        open={sessionTopicId != null}
        title={sessionTitle}
        busy={sessionBusy}
        error={sessionError}
        onTitle={setSessionTitle}
        onOpenChange={(open) => {
          if (!open && !sessionBusy) setSessionTopicId(null)
        }}
        onSubmit={() => {
          const nextTitle = sessionTitle.trim()
          if (!sessionTopicId || !nextTitle || nextTitle.length > 500 || sessionBusy) return
          const topicId = sessionTopicId
          setSessionBusy(true)
          setSessionError('')
          void onCreateSession(topicId, nextTitle)
            .then(() => setSessionTopicId(null))
            .catch((err: unknown) => {
              setSessionError(err instanceof Error ? err.message : 'Could not create a session')
            })
            .finally(() => setSessionBusy(false))
        }}
      />
    </div>
  )
}

function TopicActionsMenu({
  title,
  pinned,
  canMerge,
  onRename,
  onTogglePin,
  onMerge,
  onDelete,
  onCreateSession,
}: {
  title: string
  pinned: boolean
  canMerge: boolean
  onRename: () => void
  onTogglePin: () => void
  onMerge: () => void
  onDelete: () => void
  onCreateSession: () => void
}) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        className="mc-topic-rail-menu"
        aria-label={`Topic actions for ${title}`}
        title="Topic actions"
        onClick={(event) => event.stopPropagation()}
        onPointerDown={(event) => event.stopPropagation()}
      >
        <IconMoreVertical />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" onClick={(event) => event.stopPropagation()}>
        <DropdownMenuItem
          onSelect={(event) => {
            event.stopPropagation()
            onRename()
          }}
        >
          Rename
        </DropdownMenuItem>
        <DropdownMenuItem
          onSelect={(event) => {
            event.stopPropagation()
            onTogglePin()
          }}
        >
          {pinned ? 'Unpin' : 'Pin'}
        </DropdownMenuItem>
        <DropdownMenuItem
          disabled={!canMerge}
          onSelect={(event) => {
            event.stopPropagation()
            onMerge()
          }}
        >
          Merge with previous
        </DropdownMenuItem>
        <DropdownMenuItem
          title="Start a fresh session from this topic"
          onSelect={(event) => {
            event.stopPropagation()
            onCreateSession()
          }}
        >
          Create new session
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem
          variant="destructive"
          onSelect={(event) => {
            event.stopPropagation()
            onDelete()
          }}
        >
          Delete topic and messages…
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

function NewTopicSessionDialog({
  open,
  title,
  busy,
  error,
  onTitle,
  onOpenChange,
  onSubmit,
}: {
  open: boolean
  title: string
  busy: boolean
  error: string
  onTitle: (value: string) => void
  onOpenChange: (open: boolean) => void
  onSubmit: () => void
}) {
  const trimmed = title.trim()
  const tooLong = trimmed.length > 500
  const canCreate = trimmed.length > 0 && !tooLong && !busy
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-[420px]">
        <DialogHeader>
          <DialogTitle>New session</DialogTitle>
          <DialogDescription>
            Starts a fresh session from this topic, on the persona you already have open.
          </DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-2">
          <Input
            placeholder="Session focus"
            value={title}
            onChange={(event) => onTitle(event.target.value)}
            autoFocus
            disabled={busy}
            aria-label="Session title"
            maxLength={500}
            onKeyDown={(event) => {
              if (event.key === 'Enter') {
                event.preventDefault()
                onSubmit()
              }
            }}
          />
          {tooLong ? <p className="text-xs text-destructive">Keep the title under 500 characters.</p> : null}
          {error ? <p className="text-xs text-destructive">{error}</p> : null}
        </div>
        <DialogFooter>
          <Button variant="outline" disabled={busy} onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button onClick={onSubmit} disabled={!canCreate}>
            {busy ? 'Creating…' : 'Create session'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function flattenRows(topics: ConversationTopic[], expanded: Record<string, boolean>): TopicRow[] {
  const rows: TopicRow[] = []
  topics.forEach((topic, index) => {
    rows.push({ kind: 'topic', id: topic.id, topic, index })
    if (expanded[topic.id] === false) return
    for (const side of topic.side_chats) {
      rows.push({
        kind: 'side',
        id: side.id,
        topicId: topic.id,
        sideChatId: side.id,
        anchorMessageId: side.anchor_message_id,
      })
    }
  })
  return rows
}

export type TopicOutlineRailProps = {
  topics: ConversationTopic[]
  segmenting: boolean
  unsegmentedCount: number
  loading?: boolean
  error?: string
  focusTopicId: string | null
  onClearFocus: () => void
  onJump: (messageId: string) => void
  onSelectSideChat: (sideChatId: string) => void
  onRename: (topicId: string, title: string) => void
  onTogglePin: (topicId: string, pinned: boolean) => void
  onMerge: (topicId: string) => void
  onDelete: (topicId: string) => void
  onCreateSession: (topicId: string, title: string) => Promise<void>
  onRefresh: () => void
  enableSpy?: boolean
  className?: string
  /** Sheet copy stays expanded; it does not share the desktop collapse toggle. */
  variant?: 'rail' | 'sheet'
}

export function TopicOutlineRail({
  topics,
  segmenting,
  unsegmentedCount,
  loading,
  error,
  focusTopicId,
  onClearFocus,
  onJump,
  onSelectSideChat,
  onRename,
  onTogglePin,
  onMerge,
  onDelete,
  onCreateSession,
  onRefresh,
  enableSpy = false,
  className,
  variant = 'rail',
}: TopicOutlineRailProps) {
  const [filter, setFilter] = React.useState('')
  const [collapsed, setCollapsed] = React.useState(() => {
    try {
      return window.localStorage.getItem('mc-topic-rail-collapsed') === '1'
    } catch {
      return false
    }
  })
  const [spyId, setSpyId] = React.useState<string | null>(null)
  const holdUntil = React.useRef(0)
  const rootRef = React.useRef<HTMLDivElement>(null)

  React.useEffect(() => {
    if (focusTopicId) holdUntil.current = Date.now() + 800
  }, [focusTopicId])

  React.useEffect(() => {
    if (!enableSpy) return
    const viewport = document.querySelector('.mc-thread-viewport')
    if (!(viewport instanceof HTMLElement)) return
    let frame = 0
    const onScroll = () => {
      cancelAnimationFrame(frame)
      frame = window.requestAnimationFrame(() => {
        if (Date.now() < holdUntil.current) return
        const rootTop = viewport.getBoundingClientRect().top
        const nodes = viewport.querySelectorAll('[data-message-ts]')
        let chosen: string | null = null
        for (const node of nodes) {
          if (!(node instanceof HTMLElement)) continue
          if (node.getBoundingClientRect().bottom < rootTop + 24) continue
          const ts = node.getAttribute('data-message-ts')
          if (!ts) continue
          chosen = topicIdForTimestamp(topics, ts)
          break
        }
        if (!chosen) return
        if (focusTopicId && chosen !== focusTopicId) onClearFocus()
        setSpyId(chosen)
      })
    }
    viewport.addEventListener('scroll', onScroll, { passive: true })
    onScroll()
    return () => {
      cancelAnimationFrame(frame)
      viewport.removeEventListener('scroll', onScroll)
    }
  }, [enableSpy, focusTopicId, onClearFocus, topics])

  const activeTopicId = focusTopicId ?? spyId

  React.useEffect(() => {
    if (!activeTopicId || !rootRef.current) return
    const tree = rootRef.current.querySelector('.mc-topic-rail-tree')
    const el = rootRef.current.querySelector(`[data-topic-id="${CSS.escape(activeTopicId)}"]`)
    if (tree instanceof HTMLElement && el instanceof HTMLElement) {
      const treeRect = tree.getBoundingClientRect()
      const elRect = el.getBoundingClientRect()
      if (elRect.top < treeRect.top) {
        tree.scrollTop += elRect.top - treeRect.top
      } else if (elRect.bottom > treeRect.bottom) {
        tree.scrollTop += elRect.bottom - treeRect.bottom
      }
    }
  }, [activeTopicId])

  const toggleCollapsed = () => {
    setCollapsed((value) => {
      const next = !value
      try {
        window.localStorage.setItem('mc-topic-rail-collapsed', next ? '1' : '0')
      } catch {
        // Ignore storage failures; the toggle still applies for this view.
      }
      return next
    })
  }

  return (
    <div ref={rootRef} className={className}>
      <TopicOutlineTree
        topics={topics}
        activeTopicId={activeTopicId}
        segmenting={segmenting}
        unsegmentedCount={unsegmentedCount}
        loading={loading}
        error={error}
        filter={filter}
        onFilterChange={setFilter}
        onJump={onJump}
        onSelectSideChat={onSelectSideChat}
        onRename={onRename}
        onTogglePin={onTogglePin}
        onMerge={onMerge}
        onDelete={onDelete}
        onCreateSession={onCreateSession}
        onRefresh={onRefresh}
        collapsed={variant === 'sheet' ? false : collapsed}
        onToggleCollapsed={toggleCollapsed}
        showCollapse={variant === 'rail'}
      />
    </div>
  )
}
