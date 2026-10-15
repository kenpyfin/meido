import { useEffect } from 'react'
import type { PendingReplyQuote } from '../lib/reply-quote'

export type UseKeyboardShortcutsOptions = {
  activePendingReply: PendingReplyQuote | null
  onDismissPendingReply: () => void
  settingsDialogOpen: boolean
  mobileOpsOpen: boolean
  shortcutsOpen: boolean
  onOpenShortcuts: () => void
}

export function useKeyboardShortcuts({
  activePendingReply,
  onDismissPendingReply,
  settingsDialogOpen,
  mobileOpsOpen,
  shortcutsOpen,
  onOpenShortcuts,
}: UseKeyboardShortcutsOptions): void {
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      const target = e.target
      if (!(target instanceof HTMLElement)) return
      const tag = target.tagName
      const typing =
        tag === 'INPUT' ||
        tag === 'TEXTAREA' ||
        target.isContentEditable ||
        settingsDialogOpen ||
        mobileOpsOpen ||
        shortcutsOpen
      if (e.key === '?' && !typing && !e.metaKey && !e.ctrlKey && !e.altKey) {
        e.preventDefault()
        onOpenShortcuts()
      }
      if (e.key === '/' && !typing && !e.metaKey && !e.ctrlKey && !e.altKey) {
        e.preventDefault()
        document.querySelector<HTMLElement>('.aui-composer-input')?.focus()
      }
      if (e.key === 'Escape' && activePendingReply) {
        onDismissPendingReply()
      }
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [
    activePendingReply,
    mobileOpsOpen,
    onDismissPendingReply,
    onOpenShortcuts,
    settingsDialogOpen,
    shortcutsOpen,
  ])
}
