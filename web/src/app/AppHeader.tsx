import { Button } from '@/components/ui/button'
import { ChevronDown, Menu } from 'lucide-react'
import React from 'react'
import { SessionPicker } from '../components/session-picker'
import { SideChatsPicker } from '../components/side-chats-picker'
import { IconCockpit, IconOps, IconRibbonBow, IconServiceBell, IconSidebar } from '../components/icons'
import type { ChatSession, InstallationStatus, QueueLane, SideChatSummary } from '../types'

type Appearance = 'dark' | 'light'

export type AppHeaderNavProps = {
  mobileNavOpen: boolean
  onOpenMobileNav: () => void
  desktopSidebarOpen: boolean
  onToggleDesktopSidebar: () => void
}

export type AppHeaderSessionProps = {
  activePersonaId: number | null
  activePersonaName: string | null
  chatSessions: ChatSession[]
  activeSessionId: string | null
  historyLoading: boolean
  onSelectSession: (sessionId: string | null) => void
  onCreateSession: (intent: string, mirrorMainChat: boolean) => Promise<boolean>
  onDeleteSession: (sessionId: string) => Promise<void>
  sideChats: SideChatSummary[]
  activeSideChatId: string | null
  sideChatsLoading?: boolean
  onSelectSideChat: (sideChatId: string) => void
  onDeleteSideChat: (sideChatId: string) => Promise<boolean>
}

export type AppHeaderToolbarProps = {
  queueLane: QueueLane | null
  backgroundActiveCount: number
  installationStatus: InstallationStatus | null
  statusText: string
  cockpitExpanded?: boolean
  onToggleCockpit: () => void
  onOpenPersona: () => void
  onOpenSettings: () => void
  onOpenInbox: () => void
  inboxBadgeCount?: number
  schedulesBadgeCount?: number
  onOpenSchedules: () => void
  onOpenPrinciples: () => void
  onOpenArtifacts: () => void
  onOpenMemory: () => void
  onOpenAgentHistory: () => void
  onOpenTopics: () => void
  onOpenTerminal?: () => void
  terminalAvailable?: boolean
  agentHistoryDisabled?: boolean
}

export type AppHeaderProps = {
  appearance: Appearance
  mobileContextOpen: boolean
  onOpenMobileContext: () => void
  nav: AppHeaderNavProps
  session: AppHeaderSessionProps
  toolbar: AppHeaderToolbarProps
  onOpenMobileOps: () => void
  /** Set outside Personas mode: the header shows the module name and hides persona-only controls. */
  moduleTitle?: string | null
}

export const AppHeader = React.memo(function AppHeader({
  appearance,
  mobileContextOpen,
  onOpenMobileContext,
  nav,
  session,
  toolbar,
  onOpenMobileOps,
  moduleTitle = null,
}: AppHeaderProps) {
  const personaModule = moduleTitle == null
  const {
    mobileNavOpen,
    onOpenMobileNav,
    desktopSidebarOpen,
    onToggleDesktopSidebar,
  } = nav
  const {
    activePersonaId,
    activePersonaName,
    chatSessions,
    activeSessionId,
    historyLoading,
    onSelectSession,
    onCreateSession,
    onDeleteSession,
    sideChats,
    activeSideChatId,
    sideChatsLoading,
    onSelectSideChat,
    onDeleteSideChat,
  } = session
  const {
    queueLane,
    backgroundActiveCount,
    cockpitExpanded = false,
    onToggleCockpit,
    onOpenPersona,
    onOpenSettings,
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
    agentHistoryDisabled = false,
  } = toolbar

  const cockpitBusyCount = (queueLane?.pending ?? 0) + backgroundActiveCount
  const sessionCaption =
    activePersonaId == null || !personaModule
      ? null
      : activeSessionId
        ? chatSessions.find((item) => item.id === activeSessionId)?.title || 'Session'
        : 'Main chat'
  const mobileTitle = moduleTitle ?? activePersonaName ?? 'Chat'

  const renderSessionControls = (className?: string) =>
    activePersonaId == null || !personaModule ? null : (
      <div className={`flex items-center mc-session-controls min-w-0 ${className ?? ''}`}>
        <SessionPicker
          compact
          sessions={chatSessions}
          activeSessionId={activeSessionId}
          onSelectSession={onSelectSession}
          onCreateSession={onCreateSession}
          onDeleteSession={onDeleteSession}
          loading={historyLoading}
        />
        <span className="mc-toolbar-divider" aria-hidden />
        <SideChatsPicker
          sideChats={sideChats}
          sessions={chatSessions}
          activeSideChatId={activeSideChatId}
          loading={sideChatsLoading}
          onSelect={onSelectSideChat}
          onDelete={onDeleteSideChat}
        />
      </div>
    )

  return (
    <header
      className={
        appearance === 'dark'
          ? 'mc-katyusha-lace sticky top-0 z-30 bg-[color:var(--mc-bg-panel)]/95 backdrop-blur-sm md:top-0'
          : 'mc-katyusha-lace sticky top-0 z-30 bg-[color:var(--mc-surface-elevated)]/92 backdrop-blur-sm md:top-0'
      }
    >
      <div className="mc-app-header-bar px-3 max-md:py-2 md:px-4 md:py-3">
        <div className="flex justify-between items-center gap-2 flex-wrap w-full flex-col md:flex-row md:flex-wrap">
          <div className="flex items-center gap-2 min-w-0 w-full min-h-[44px] md:flex-1">
            <Button
              size="icon-lg"
              variant="secondary"
              className="shrink-0 min-h-10 min-w-10 md:!hidden"
              type="button"
              aria-expanded={mobileNavOpen}
              aria-haspopup="dialog"
              aria-controls="mobile-session-sidebar-panel"
              aria-label="Open personas and theme"
              title="Personas & theme"
              onClick={onOpenMobileNav}
            >
              <Menu className="size-5 shrink-0" aria-hidden />
            </Button>
            <Button
              size="icon-lg"
              variant="secondary"
              className="!hidden shrink-0 md:!inline-flex"
              type="button"
              aria-expanded={desktopSidebarOpen}
              aria-label={desktopSidebarOpen ? 'Hide personas sidebar' : 'Show personas sidebar'}
              title={desktopSidebarOpen ? 'Hide personas' : 'Show personas'}
              onClick={onToggleDesktopSidebar}
            >
              <IconSidebar
                className={`size-5 shrink-0 transition-transform duration-150 ${
                  desktopSidebarOpen ? '' : 'rotate-180'
                }`}
              />
            </Button>
            {personaModule ? (
              <button
                type="button"
                className="mc-display-title flex min-h-10 min-w-0 flex-1 items-center gap-1 text-left text-[1.35rem] leading-none md:!hidden"
                aria-expanded={mobileContextOpen}
                aria-haspopup="dialog"
                title="Session, topics, and view"
                onClick={onOpenMobileContext}
              >
                {activePersonaName ? (
                  <IconRibbonBow className="size-4 shrink-0 text-[color:var(--mc-accent)]" />
                ) : null}
                <span className="truncate">{mobileTitle}</span>
                {sessionCaption ? (
                  <span className="truncate font-sans text-xs font-normal text-[color:var(--mc-text-muted)]">
                    {sessionCaption}
                  </span>
                ) : null}
                <ChevronDown
                  className={`size-4 shrink-0 text-[color:var(--mc-text-muted)] transition-transform duration-150 ${
                    mobileContextOpen ? 'rotate-180' : ''
                  }`}
                  aria-hidden
                />
              </button>
            ) : (
              <h1 className="mc-display-title min-w-0 flex-1 truncate text-[1.35rem] md:!hidden">{mobileTitle}</h1>
            )}
            <h1 className="mc-display-title hidden min-w-0 shrink items-center gap-1.5 text-2xl md:!flex">
              {personaModule && activePersonaName ? (
                <IconRibbonBow className="size-4 shrink-0 text-[color:var(--mc-accent)]" />
              ) : null}
              <span className="truncate">{mobileTitle}</span>
            </h1>
            {renderSessionControls('max-md:!hidden')}
            {personaModule ? (
              <button
                type="button"
                className="mc-cockpit-launch relative shrink-0 md:!hidden"
                data-active={cockpitExpanded ? 'true' : 'false'}
                data-busy={cockpitBusyCount > 0 ? 'true' : 'false'}
                aria-pressed={cockpitExpanded}
                aria-label={
                  cockpitBusyCount > 0 ? `Cockpit, ${cockpitBusyCount} active` : 'Cockpit'
                }
                title="Cockpit"
                onClick={onToggleCockpit}
              >
                <IconCockpit className="size-4 shrink-0" />
                {cockpitBusyCount > 0 ? (
                  <span className="mc-badge-counter absolute -right-1 -top-1" aria-hidden>
                    {cockpitBusyCount > 99 ? '99+' : cockpitBusyCount}
                  </span>
                ) : null}
              </button>
            ) : null}
            <button
              type="button"
              className="mc-cockpit-launch relative shrink-0 md:!hidden"
              aria-label={inboxBadgeCount > 0 ? `Tools, ${inboxBadgeCount} in inbox` : 'Tools'}
              title="Tools"
              onClick={onOpenMobileOps}
            >
              <IconOps className="size-4 shrink-0" />
              {inboxBadgeCount > 0 ? (
                <span className="mc-badge-counter absolute -right-1 -top-1" aria-hidden>
                  {inboxBadgeCount > 99 ? '99+' : inboxBadgeCount}
                </span>
              ) : null}
            </button>
          </div>
          <div className="flex items-center gap-2 flex-wrap justify-end w-full max-md:!hidden md:!flex">
            {personaModule ? (
            <button
              type="button"
              className="mc-cockpit-launch cursor-pointer"
              data-active={cockpitExpanded ? 'true' : 'false'}
              data-busy={cockpitBusyCount > 0 ? 'true' : 'false'}
              aria-pressed={cockpitExpanded}
              aria-label={
                cockpitBusyCount > 0 ? `Cockpit, ${cockpitBusyCount} active` : 'Cockpit'
              }
              title="Cockpit"
              onClick={onToggleCockpit}
            >
              <IconCockpit className="size-4 shrink-0" />
              <span className="mc-cockpit-launch-label">Cockpit</span>
              {cockpitBusyCount > 0 ? (
                <span className="mc-badge-counter" aria-hidden>
                  {cockpitBusyCount > 99 ? '99+' : cockpitBusyCount}
                </span>
              ) : null}
            </button>
            ) : null}
            <button
              type="button"
              className="mc-inbox-launch !hidden md:!inline-flex"
              data-has-items={inboxBadgeCount > 0 ? 'true' : 'false'}
              aria-label={
                inboxBadgeCount > 0 ? `Inbox, ${inboxBadgeCount} items` : 'Open Inbox'
              }
              title="Inbox — new messages and todos"
              onClick={onOpenInbox}
            >
              <IconServiceBell className="size-3.5 shrink-0" />
              Inbox
              {inboxBadgeCount > 0 ? (
                <span className="mc-inbox-launch__badge" aria-hidden>
                  {inboxBadgeCount > 99 ? '99+' : inboxBadgeCount}
                </span>
              ) : null}
            </button>
            <span className="mc-toolbar-divider !hidden md:!inline-block" aria-hidden />
            <Button size="sm" variant="secondary" className="!hidden md:!inline-flex" onClick={onOpenPersona}>
              Persona
            </Button>
            <Button size="sm" variant="secondary" className="!hidden md:!inline-flex" onClick={onOpenSettings}>
              Settings
            </Button>
            <Button
              size="sm"
              variant="secondary"
              className="!hidden md:!inline-flex"
              aria-label={
                schedulesBadgeCount > 0
                  ? `Schedules, ${schedulesBadgeCount} unread runs`
                  : 'Open Schedules'
              }
              onClick={onOpenSchedules}
            >
              Schedules
              {schedulesBadgeCount > 0 ? (
                <span className="mc-inbox-launch__badge" aria-hidden>
                  {schedulesBadgeCount > 99 ? '99+' : schedulesBadgeCount}
                </span>
              ) : null}
            </Button>
            {personaModule ? (
            <Button size="sm" variant="secondary" className="!hidden md:!inline-flex" onClick={onOpenPrinciples}>
              Principles
            </Button>
            ) : null}
            <Button size="sm" variant="secondary" className="!hidden md:!inline-flex" onClick={onOpenArtifacts}>
              Artifacts
            </Button>
            {terminalAvailable && onOpenTerminal ? (
              <Button size="sm" variant="secondary" className="!hidden md:!inline-flex" onClick={onOpenTerminal}>
                Terminal
              </Button>
            ) : null}
            {personaModule ? (
              <>
                <Button size="sm" variant="secondary" className="!hidden md:!inline-flex" onClick={onOpenMemory}>
                  Memory
                </Button>
                <Button
                  size="sm"
                  variant="secondary"
                  className="!hidden md:!inline-flex"
                  disabled={agentHistoryDisabled}
                  onClick={onOpenAgentHistory}
                >
                  Last agent run
                </Button>
              </>
            ) : null}
          </div>
        </div>
      </div>
    </header>
  )
})
