import React, { act } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { AppHeader, type AppHeaderProps } from './AppHeader'

vi.mock('../components/session-picker', () => ({
  SessionPicker: () => <div data-testid="session-picker" />,
}))

vi.mock('../components/side-chats-picker', () => ({
  SideChatsPicker: () => <div data-testid="side-chats-picker" />,
}))

vi.mock('@/components/ui/button', () => ({
  Button: ({ children, ...props }: React.ButtonHTMLAttributes<HTMLButtonElement>) => (
    <button {...props}>{children}</button>
  ),
}))

vi.mock('@/components/ui/dropdown-menu', () => ({
  DropdownMenu: ({ children }: { children: React.ReactNode }) => (
    <div data-testid="records-dropdown">{children}</div>
  ),
  DropdownMenuTrigger: ({ children }: { children: React.ReactNode }) => (
    <div data-testid="records-trigger">{children}</div>
  ),
  DropdownMenuContent: ({
    children,
    className,
  }: {
    children: React.ReactNode
    className?: string
  }) => <div className={className}>{children}</div>,
  DropdownMenuItem: ({
    children,
    disabled,
    onSelect,
  }: {
    children: React.ReactNode
    disabled?: boolean
    onSelect?: () => void
  }) => (
    <button
      type="button"
      data-slot="dropdown-menu-item"
      disabled={disabled}
      onClick={() => {
        if (!disabled) onSelect?.()
      }}
    >
      {children}
    </button>
  ),
  DropdownMenuSeparator: () => <hr data-slot="dropdown-menu-separator" />,
}))

function makeProps(overrides?: Partial<AppHeaderProps>): AppHeaderProps {
  return {
    appearance: 'dark',
    mobileContextOpen: false,
    onOpenMobileContext: vi.fn(),
    onOpenMobileOps: vi.fn(),
    nav: {
      mobileNavOpen: false,
      onOpenMobileNav: vi.fn(),
      desktopSidebarOpen: true,
      onToggleDesktopSidebar: vi.fn(),
    },
    session: {
      activePersonaId: 2,
      activePersonaName: 'Kuro',
      chatSessions: [],
      activeSessionId: null,
      historyLoading: false,
      onSelectSession: vi.fn(),
      onCreateSession: vi.fn(async () => true),
      onDeleteSession: vi.fn(async () => {}),
      sideChats: [],
      activeSideChatId: null,
      onSelectSideChat: vi.fn(),
      onDeleteSideChat: vi.fn(async () => true),
    },
    toolbar: {
      queueLane: { pending: 1, active: true },
      backgroundActiveCount: 1,
      installationStatus: null,
      statusText: 'Ready',
      cockpitExpanded: false,
      onToggleCockpit: vi.fn(),
      onOpenPersona: vi.fn(),
      onOpenSettings: vi.fn(),
      onOpenInbox: vi.fn(),
      inboxBadgeCount: 2,
      schedulesBadgeCount: 0,
      onOpenSchedules: vi.fn(),
      onOpenPrinciples: vi.fn(),
      onOpenArtifacts: vi.fn(),
      onOpenMemory: vi.fn(),
      onOpenAgentHistory: vi.fn(),
      onOpenTopics: vi.fn(),
      onOpenTerminal: vi.fn(),
      terminalAvailable: true,
      agentHistoryDisabled: false,
    },
    ...overrides,
  }
}

let container: HTMLDivElement
let root: Root

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  container = document.createElement('div')
  document.body.append(container)
  root = createRoot(container)
})

afterEach(async () => {
  await act(async () => root.unmount())
  container.remove()
})

describe('AppHeader three-cluster desktop toolbar and Records menu', () => {
  it('groups desktop tools into attention, configuration, and records when schedules are idle', async () => {
    const props = makeProps()
    await act(async () => {
      root.render(<AppHeader {...props} />)
    })

    const attention = container.querySelector('[data-cluster="attention"]')
    const configuration = container.querySelector('[data-cluster="configuration"]')
    const records = container.querySelector('[data-cluster="records"]')

    expect(attention).not.toBeNull()
    expect(configuration).not.toBeNull()
    expect(records).not.toBeNull()

    expect(attention?.textContent).toContain('Cockpit')
    expect(attention?.textContent).toContain('Inbox')
    expect(attention?.textContent).not.toContain('Schedules')

    expect(configuration?.textContent).toContain('Persona')
    expect(configuration?.textContent).toContain('Settings')

    const recordsMenu = records?.querySelector('.mc-records-menu')
    expect(recordsMenu).not.toBeNull()
    const itemLabels = Array.from(
      recordsMenu?.querySelectorAll('[data-slot="dropdown-menu-item"]') ?? []
    ).map((el) => el.textContent)
    expect(itemLabels).toEqual([
      'Schedules',
      'Artifacts',
      'Terminal',
      'Principles',
      'Memory',
      'Last agent run',
    ])

    const memoryBtn = Array.from(
      recordsMenu?.querySelectorAll('[data-slot="dropdown-menu-item"]') ?? []
    ).find((el) => el.textContent === 'Memory') as HTMLButtonElement | undefined
    await act(async () => {
      memoryBtn?.click()
    })
    expect(props.toolbar.onOpenMemory).toHaveBeenCalledTimes(1)
  })

  it('surfaces Schedules in the attention cluster when unread runs exist and disables Last agent run when empty', async () => {
    const props = makeProps({
      toolbar: {
        ...makeProps().toolbar,
        schedulesBadgeCount: 3,
        agentHistoryDisabled: true,
      },
    })
    await act(async () => {
      root.render(<AppHeader {...props} />)
    })

    const attention = container.querySelector('[data-cluster="attention"]')
    expect(attention?.textContent).toContain('Schedules')
    expect(attention?.textContent).toContain('3')

    const recordsMenu = container.querySelector('[data-cluster="records"] .mc-records-menu')
    const itemLabels = Array.from(
      recordsMenu?.querySelectorAll('[data-slot="dropdown-menu-item"]') ?? []
    ).map((el) => el.textContent)
    expect(itemLabels).not.toContain('Schedules')

    const historyItem = Array.from(
      recordsMenu?.querySelectorAll('[data-slot="dropdown-menu-item"]') ?? []
    ).find((el) => el.textContent === 'Last agent run') as HTMLButtonElement | undefined
    expect(historyItem?.disabled).toBe(true)
  })

  it('keeps the persona name and session caption on one header row', async () => {
    const props = makeProps({
      session: {
        ...makeProps().session,
        activeSessionId: 's1',
        chatSessions: [
          {
            id: 's1',
            chat_id: 1,
            persona_id: 2,
            title: 'A very long chat name that used to cover the persona',
            intent: '',
            status: 'active',
            created_at: '2026-10-20T00:00:00Z',
            last_active_at: '2026-10-20T00:00:00Z',
            ttl_hours: 0,
          },
        ],
      },
    })
    await act(async () => {
      root.render(<AppHeader {...props} />)
    })

    const row = container.querySelector('.mc-app-header-row')
    expect(row).not.toBeNull()
    expect(row?.className).not.toContain('flex-wrap')
    expect(row?.className).not.toContain('flex-col')

    const persona = container.querySelector('.mc-header-persona')
    const session = container.querySelector('.mc-header-session')
    expect(persona?.textContent).toBe('Kuro')
    expect(session?.textContent).toContain('A very long chat name')
    const cockpitIcon = container.querySelector('.mc-cockpit-launch svg')
    expect(cockpitIcon?.getAttribute('stroke-width')).toBe('2')
    expect(cockpitIcon?.getAttribute('stroke-linecap')).toBe('round')
    const inboxIcon = container.querySelector('[data-cluster="attention"] button[title^="Inbox"] svg')
    expect(inboxIcon?.getAttribute('stroke-width')).toBe('2')
    expect(inboxIcon?.getAttribute('stroke-linecap')).toBe('round')
    expect(container.querySelector('[data-cluster="attention"] button[title^="Inbox"]')?.className).toContain(
      'mc-cockpit-launch',
    )
  })

  it('hides persona-only controls outside the Personas module', async () => {
    const props = makeProps({ moduleTitle: 'Workspace' })
    await act(async () => {
      root.render(<AppHeader {...props} />)
    })

    const attention = container.querySelector('[data-cluster="attention"]')
    expect(attention?.textContent).not.toContain('Cockpit')
    expect(attention?.textContent).toContain('Inbox')

    const recordsMenu = container.querySelector('[data-cluster="records"] .mc-records-menu')
    const itemLabels = Array.from(
      recordsMenu?.querySelectorAll('[data-slot="dropdown-menu-item"]') ?? []
    ).map((el) => el.textContent)
    expect(itemLabels).toEqual(['Schedules', 'Artifacts', 'Terminal'])
  })
})
