import { useEffect, useState, type ReactNode } from 'react'
import { IconListTree, IconOps, IconSettings } from './icons'

export type MobileTab = 'chat' | 'topics' | 'ops' | 'settings'

type MobileTabBarProps = {
  active: MobileTab
  onChat: () => void
  onTopics: () => void
  onOps: () => void
  onSettings: () => void
}

function ChatIcon() {
  return (
    <svg className="size-5 shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
    </svg>
  )
}

const TABS: { id: MobileTab; label: string; icon: ReactNode }[] = [
  { id: 'chat', label: 'Chat', icon: <ChatIcon /> },
  { id: 'topics', label: 'Topics', icon: <IconListTree /> },
  { id: 'ops', label: 'Ops', icon: <IconOps /> },
  { id: 'settings', label: 'Settings', icon: <IconSettings /> },
]

export function MobileTabBar({ active, onChat, onTopics, onOps, onSettings }: MobileTabBarProps) {
  const [keyboardOpen, setKeyboardOpen] = useState(false)

  useEffect(() => {
    const viewport = window.visualViewport
    if (!viewport) return
    const sync = () => {
      setKeyboardOpen(viewport.height < window.innerHeight * 0.75)
    }
    sync()
    viewport.addEventListener('resize', sync)
    return () => viewport.removeEventListener('resize', sync)
  }, [])

  const handlers: Record<MobileTab, () => void> = {
    chat: onChat,
    topics: onTopics,
    ops: onOps,
    settings: onSettings,
  }

  return (
    <nav
      aria-label="Primary"
      className={
        keyboardOpen
          ? 'hidden'
          : 'fixed inset-x-0 bottom-0 z-40 flex border-t border-[color:var(--mc-border-soft)] bg-[color:var(--mc-bg-sidebar)] md:hidden'
      }
      style={{
        height: 'calc(56px + env(safe-area-inset-bottom))',
        paddingBottom: 'env(safe-area-inset-bottom)',
      }}
    >
      {TABS.map((tab) => {
        const selected = active === tab.id
        return (
          <button
            key={tab.id}
            type="button"
            className="flex min-h-11 min-w-11 flex-1 flex-col items-center justify-center gap-0.5 text-xs"
            style={{ color: selected ? 'var(--mc-accent)' : 'var(--mc-text-muted)' }}
            aria-current={selected ? 'page' : undefined}
            onClick={handlers[tab.id]}
          >
            {tab.icon}
            <span>{tab.label}</span>
          </button>
        )
      })}
    </nav>
  )
}
