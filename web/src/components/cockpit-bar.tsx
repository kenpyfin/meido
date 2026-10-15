import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import { Select } from '@/components/ui/select'
import React, { useCallback, useEffect, useId, useRef, useState } from 'react'
import { EmptyState } from './empty-state'
import { api } from '../api/client'
import { MobileSheet } from './mobile-sheet'
import { IconTrash } from './icons'
import {
  OPERATOR_MEMO_MAX_CHARS,
  type InstallationStatus,
  type PersonaBulletinFocus,
  type PersonaMessageBookmark,
  type QueueLane,
} from '../types'

function operatorMemoCharCount(s: string): number {
  return Array.from(s.trim()).length
}

function StatusSep() {
  return (
    <span
      className="mc-cockpit-status-sep mx-1.5 inline-block h-3 w-px shrink-0 bg-[color:var(--mc-border-soft)] max-md:hidden"
      aria-hidden
    />
  )
}

const cockpitLinkClass =
  'm-0 inline-flex cursor-pointer items-center gap-1.5 border-0 bg-transparent p-0 text-left font-inherit text-[13px] text-[color:var(--mc-text-primary)] underline-offset-2 hover:underline focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[color:var(--mc-accent)]'

const controlsPanelClass = 'rounded-md border border-[color:var(--mc-border-soft)] p-3'

/** Radix Select portals its listbox to `document.body`; clicks there are outside `expandedRootRef`. */
function isPointerOnRadixSelectOverlay(target: EventTarget | null): boolean {
  if (!target || !(target instanceof Element)) return false
  return (
    target.closest('[data-radix-popper-content-wrapper]') != null ||
    target.closest('[data-radix-select-viewport]') != null
  )
}

function useIsMobile(): boolean {
  const [isMobile, setIsMobile] = useState(false)
  useEffect(() => {
    const mq = window.matchMedia('(max-width: 767px)')
    const sync = () => setIsMobile(mq.matches)
    sync()
    mq.addEventListener('change', sync)
    return () => mq.removeEventListener('change', sync)
  }, [])
  return isMobile
}

export type CockpitBarProps = {
  appearance: 'dark' | 'light'
  statusText: string
  queueLane: QueueLane | null
  /** Pending agent runs on other personas in this chat (when > 0). */
  otherPersonasPending?: number
  backgroundActiveCount: number
  installationStatus: InstallationStatus | null
  onQueueClick: () => void
  /** Kept for API stability; bulletin now lives in BulletinStrip. */
  bulletinFocus?: PersonaBulletinFocus | null
  bookmarks: PersonaMessageBookmark[]
  /** Used when jumping to a bookmarked message. */
  activePersonaId: number | null
  /** Jump to message in the main thread (replaces bookmark reader dialog). */
  onJumpToBookmark?: (messageId: string) => void | Promise<void>
  /** Remove a bookmark without closing the cockpit. */
  onRemoveBookmark?: (messageId: string) => void | Promise<void | boolean>
  /** Server-stored operator memo (may be null). */
  operatorMemoServer: string | null
  /** Reload bulletin after PATCH (same persona). */
  reloadBulletin: () => Promise<void>
  /** Short status line updates after successful saves. */
  onBulletinStatus?: (message: string) => void
  /** Controlled expand state. */
  expanded?: boolean
  onExpandedChange?: (expanded: boolean) => void
}

/**
 * Session status panel: queue, background jobs, setup readiness, operator memo, bookmarks.
 * Bulletin focus and persona run-context knobs live elsewhere (BulletinStrip / Settings).
 */
export const CockpitBar = React.memo(function CockpitBar({
  statusText,
  queueLane,
  otherPersonasPending = 0,
  backgroundActiveCount,
  installationStatus,
  onQueueClick,
  bookmarks,
  activePersonaId,
  onJumpToBookmark,
  onRemoveBookmark,
  operatorMemoServer,
  reloadBulletin,
  onBulletinStatus,
  expanded: expandedControlled,
  onExpandedChange,
}: CockpitBarProps) {
  const [expandedInternal, setExpandedInternal] = useState(false)
  const expanded = expandedControlled ?? expandedInternal
  const setExpanded = useCallback(
    (value: boolean) => {
      if (expandedControlled === undefined) setExpandedInternal(value)
      onExpandedChange?.(value)
    },
    [expandedControlled, onExpandedChange],
  )
  const [memoDraft, setMemoDraft] = useState('')
  const [memoBusy, setMemoBusy] = useState(false)
  const [memoError, setMemoError] = useState('')
  const expandedRootRef = useRef<HTMLDivElement | null>(null)
  const panelId = useId()
  const isMobile = useIsMobile()
  const pending = queueLane?.pending ?? 0
  const oldestWaitMs = queueLane?.oldest_wait_ms ?? 0
  const queueError = queueLane?.last_error

  useEffect(() => {
    setMemoDraft(operatorMemoServer ?? '')
  }, [operatorMemoServer])

  const serverMemoTrimmed = (operatorMemoServer ?? '').trim()
  const memoDraftTrimmed = memoDraft.trim()
  const memoDirty = memoDraftTrimmed !== serverMemoTrimmed
  const memoCharCount = operatorMemoCharCount(memoDraft)
  const memoTooLong = memoCharCount > OPERATOR_MEMO_MAX_CHARS

  const saveMemo = useCallback(async () => {
    if (activePersonaId == null) return
    if (memoTooLong) {
      setMemoError(`Memo exceeds ${OPERATOR_MEMO_MAX_CHARS} characters (after trimming).`)
      return
    }
    setMemoBusy(true)
    setMemoError('')
    try {
      const payload = memoDraftTrimmed.length === 0 ? '' : memoDraft
      await api(`/api/personas/${activePersonaId}/bulletin`, {
        method: 'PATCH',
        body: JSON.stringify({ operator_memo: payload }),
      })
      await reloadBulletin()
      onBulletinStatus?.('Operator memo saved')
    } catch (e) {
      setMemoError(e instanceof Error ? e.message : String(e))
    } finally {
      setMemoBusy(false)
    }
  }, [
    activePersonaId,
    memoDraft,
    memoDraftTrimmed,
    memoTooLong,
    reloadBulletin,
    onBulletinStatus,
  ])

  const onMemoBlur = useCallback(() => {
    if (!memoDirty || memoBusy || memoTooLong) return
    void saveMemo()
  }, [memoBusy, memoDirty, memoTooLong, saveMemo])

  const otherHint =
    otherPersonasPending > 0
      ? ` (+${otherPersonasPending} other persona${otherPersonasPending === 1 ? '' : 's'})`
      : ''
  const queueLabel =
    pending > 0
      ? `${pending} pending${oldestWaitMs > 0 ? `, ${Math.round(oldestWaitMs / 1000)}s wait` : ''}${otherHint}${queueError ? ' (!)' : ''}`
      : otherPersonasPending > 0
        ? `idle${otherHint}${queueError ? ' (!)' : ''}`
        : `idle${queueError ? ' (!)' : ''}`

  const needsRestart =
    installationStatus != null &&
    (installationStatus.requires_restart_for_env_changes ??
      installationStatus.requires_restart_to_apply_runtime_settings) === true

  useEffect(() => {
    if (expandedControlled !== undefined && expandedControlled !== expandedInternal) {
      setExpandedInternal(expandedControlled)
    }
  }, [expandedControlled, expandedInternal])

  useEffect(() => {
    if (!expanded || isMobile) return
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target as Node | null
      if (!target) return
      if (expandedRootRef.current?.contains(target)) return
      if (isPointerOnRadixSelectOverlay(target)) return
      setExpanded(false)
    }
    window.addEventListener('pointerdown', onPointerDown)
    return () => {
      window.removeEventListener('pointerdown', onPointerDown)
    }
  }, [expanded, isMobile, setExpanded])

  const body = (
    <div id={panelId} className="space-y-3 px-3 pb-1 md:px-4">
      <div className="mc-cockpit-status-strip flex min-h-[32px] flex-wrap items-center gap-x-2 gap-y-1 text-[13px] leading-snug max-md:grid max-md:grid-cols-2 max-md:gap-2">
        <span className="font-medium max-md:col-span-2 shrink-0">
          {statusText}
        </span>
        <StatusSep />
        <button type="button" className={cockpitLinkClass} title={queueError ?? 'Open run queue'} onClick={onQueueClick}>
          <span className="text-[color:var(--mc-text-muted)]">Queue</span>
          <span
            className={
              pending > 0 || queueError
                ? 'font-medium text-amber-500'
                : 'font-medium text-[color:var(--mc-text-muted)]'
            }
          >
            {queueLabel}
          </span>
        </button>
        <StatusSep />
        <button
          type="button"
          className={cockpitLinkClass}
          title="Open run queue and background jobs"
          onClick={onQueueClick}
        >
          <span className="text-[color:var(--mc-text-muted)]">Background</span>
          <span
            className={
              backgroundActiveCount > 0
                ? 'font-medium text-blue-500'
                : 'font-medium text-[color:var(--mc-text-muted)]'
            }
          >
            {backgroundActiveCount > 0 ? `${backgroundActiveCount} active` : 'none'}
          </span>
        </button>
        {installationStatus ? (
          <>
            <StatusSep />
            <div className="flex min-w-0 flex-wrap items-center gap-2 max-md:col-span-2">
              <span className="font-medium">
                LLM {installationStatus.llm_ready ? 'ready' : 'missing'}
              </span>
              <span className="font-medium">
                Channels {installationStatus.channel_ready ? 'ready' : 'missing'}
              </span>
              {needsRestart ? (
                <span className="font-medium">
                  Restart needed
                </span>
              ) : null}
            </div>
          </>
        ) : (
          <>
            <StatusSep />
            <span className="max-md:col-span-2">
              Setup loading…
            </span>
          </>
        )}
      </div>
      {queueError ? (
        <span className="leading-snug">
          Queue error: {queueError}
        </span>
      ) : null}
      {needsRestart ? (
        <span className="leading-snug">
          Restart the process after changing API keys or runtime settings in .env.
        </span>
      ) : !queueError ? (
        <span className="leading-snug">
          Session signals update on poll. Open the queue for run details.
        </span>
      ) : null}

      <div className="space-y-3">
        <div className={`${controlsPanelClass} space-y-3`}>
          <div>
            <span className="font-medium">
              Operator memo
            </span>
            <span className="mt-1 block leading-snug">
              Short steering note for this persona (system prompt). Separate from tiered memory and the header
              Memory JSON editor.
            </span>
            <Textarea
              className="mc-cockpit-memo mt-2 min-h-[72px] font-mono text-xs"
              value={memoDraft}
              onChange={(e) => setMemoDraft(e.target.value)}
              onBlur={onMemoBlur}
              disabled={activePersonaId == null || memoBusy}
              placeholder="What the operator cares about for the next runs…"
            />
            <div className="flex mt-1 justify-between items-center flex-wrap gap-2 max-md:flex-col max-md:items-stretch">
              <span>
                {memoCharCount} / {OPERATOR_MEMO_MAX_CHARS}
              </span>
              <div className="flex gap-2 max-md:w-full max-md:justify-end">
                <Button
                  type="button"
                  size="sm"
                  variant="secondary"
                  disabled={activePersonaId == null || memoBusy || !memoDirty || memoTooLong}
                  onClick={() => void saveMemo()}
                >
                  {memoBusy ? 'Saving…' : 'Save memo'}
                </Button>
                <Button
                  type="button"
                  size="sm"
                  variant="ghost"
                  disabled={activePersonaId == null || memoBusy || serverMemoTrimmed.length === 0}
                  onClick={() => {
                    setMemoDraft('')
                    void (async () => {
                      if (activePersonaId == null) return
                      setMemoBusy(true)
                      setMemoError('')
                      try {
                        await api(`/api/personas/${activePersonaId}/bulletin`, {
                          method: 'PATCH',
                          body: JSON.stringify({ operator_memo: '' }),
                        })
                        await reloadBulletin()
                        onBulletinStatus?.('Operator memo cleared')
                      } catch (e) {
                        setMemoError(e instanceof Error ? e.message : String(e))
                      } finally {
                        setMemoBusy(false)
                      }
                    })()
                  }}
                >
                  Clear
                </Button>
              </div>
            </div>
            {memoError ? (
              <span className="mt-1">
                {memoError}
              </span>
            ) : null}
          </div>
        </div>

        <div className={controlsPanelClass}>
          <span className="font-medium">
            Bookmarks
          </span>
          {bookmarks.length > 0 ? (
            <div className="mc-cockpit-bookmark-list mt-2">
              {bookmarks.slice(0, 12).map((b) => {
                const missing = Boolean(b.missing)
                const when = b.message_timestamp
                  ? new Date(b.message_timestamp).toLocaleString(undefined, {
                      month: 'short',
                      day: 'numeric',
                      hour: '2-digit',
                      minute: '2-digit',
                    })
                  : null
                const scope = missing
                  ? 'deleted'
                  : b.session_id
                    ? 'session'
                    : 'main'
                return (
                  <div
                    key={b.message_id}
                    className={
                      missing
                        ? 'mc-cockpit-bookmark-card mc-cockpit-bookmark-card--missing'
                        : 'mc-cockpit-bookmark-card'
                    }
                  >
                    <button
                      type="button"
                      className="mc-cockpit-bookmark-jump"
                      disabled={missing}
                      onClick={() => {
                        if (missing) return
                        setExpanded(false)
                        void onJumpToBookmark?.(b.message_id)
                      }}
                      title={missing ? 'Message was deleted' : 'Jump to message in chat'}
                    >
                      <span className="mc-cockpit-bookmark-meta">
                        <span className="mc-cockpit-bookmark-role">{b.role}</span>
                        <span className="mc-cockpit-bookmark-scope">{scope}</span>
                        {when ? (
                          <span className="mc-cockpit-bookmark-when">{when}</span>
                        ) : null}
                      </span>
                      <span className="mc-cockpit-bookmark-preview">{b.content_preview}</span>
                      {b.note ? (
                        <span className="mc-cockpit-bookmark-note">{b.note}</span>
                      ) : null}
                      {missing ? (
                        <span className="mc-cockpit-bookmark-missing-hint">
                          Message no longer exists
                        </span>
                      ) : null}
                    </button>
                    {onRemoveBookmark ? (
                      <button
                        type="button"
                        className="mc-cockpit-bookmark-delete"
                        title="Remove bookmark"
                        aria-label="Remove bookmark"
                        onClick={(e) => {
                          e.stopPropagation()
                          void onRemoveBookmark(b.message_id)
                        }}
                      >
                        <IconTrash className="size-3.5" />
                      </button>
                    ) : null}
                  </div>
                )
              })}
            </div>
          ) : (
            <EmptyState
              title="No bookmarks yet"
              description="Bookmark messages from the thread to jump back to them here."
              className="mt-2"
            />
          )}
        </div>
      </div>
    </div>
  )

  if (isMobile) {
    return (
      <MobileSheet
        open={expanded}
        onOpenChange={setExpanded}
        title="Session status"
        description="Queue, background jobs, memo, and bookmarks."
      >
        {body}
      </MobileSheet>
    )
  }

  if (!expanded) return null

  return (
    <div
      ref={expandedRootRef}
      className="mc-cockpit mc-cockpit-dropdown absolute left-0 right-0 top-full z-30 mt-1 max-h-[min(70vh,560px)] overflow-y-auto overscroll-contain rounded-xl border border-[color:var(--mc-border-soft)] bg-[color:var(--mc-bg-main)]/95 py-2 shadow-lg backdrop-blur"
      role="region"
      aria-label="Session status"
    >
      {body}
    </div>
  )
})
