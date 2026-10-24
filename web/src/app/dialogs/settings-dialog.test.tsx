import React, { act, useState } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { SettingsDialog } from './settings-dialog'
import type { SettingsDialogProps, SettingsLedgerPage } from './types'

vi.mock('../../components/settings-runtime', () => ({
  SettingsRuntimePanel: () => <div>Runtime toggles body</div>,
}))
vi.mock('../../components/settings-integrations', () => ({ SettingsIntegrationsPanel: () => null }))
vi.mock('../../components/settings-connections', () => ({ SettingsConnectionsPanel: () => <div>Connections body</div> }))
vi.mock('../../components/settings-topics-model', () => ({ SettingsTopicsModelPanel: () => null }))
vi.mock('../../components/settings-environment', () => ({ SettingsEnvironmentPanel: () => null }))
vi.mock('@/components/ui/dialog', () => ({
  Dialog: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
  DialogContent: ({ children, className }: { children: React.ReactNode; className?: string }) => (
    <div className={className}>{children}</div>
  ),
  DialogTitle: ({ children, className }: { children: React.ReactNode; className?: string }) => (
    <h2 className={className}>{children}</h2>
  ),
  DialogDescription: ({ children }: { children: React.ReactNode }) => <p>{children}</p>,
  DialogClose: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
}))
vi.mock('@/components/ui/button', () => ({
  Button: ({ children, ...props }: React.ButtonHTMLAttributes<HTMLButtonElement>) => (
    <button {...props}>{children}</button>
  ),
}))

const configurePersona = vi.fn()

function props(): SettingsDialogProps {
  return {
    appearance: 'dark',
    api: vi.fn(),
    chatId: 1,
    activePersonaId: 2,
    personas: [{ id: 2, name: 'Kuro', is_active: true }],
    onRunSetupWizard: vi.fn(),
    onConfigurePersona: configurePersona,
    settings: {
      open: true,
      onOpenChange: vi.fn(),
      error: '',
      setError: vi.fn(),
      restartNotice: null,
      installationStatus: {
        llm_ready: false,
        channel_ready: true,
        cursor_engine_ready: false,
        web_enabled: true,
        requires_restart_for_env_changes: true,
        needs_initialization: true,
      },
      restartBusy: false,
      requestRestart: vi.fn(async () => {}),
      bindings: [],
      updateChannelPersonaPolicy: vi.fn(async () => {}),
      reloadInstallationStatus: vi.fn(async () => {}),
      page: 'overview',
      onPageChange: vi.fn(),
      revealPage: 0,
    },
  }
}

function Harness() {
  const base = props()
  const [page, setPage] = useState<SettingsLedgerPage>('overview')
  const [revealPage, setRevealPage] = useState(0)
  return (
    <SettingsDialog
      {...base}
      settings={{
        ...base.settings,
        page,
        revealPage,
        onPageChange: (next) => {
          setPage(next)
          setRevealPage((n) => n + 1)
        },
      }}
    />
  )
}

function installMatchMedia(mobile: boolean) {
  window.matchMedia = ((query: string) => ({
    matches: mobile && query.includes('767'),
    media: query,
    addEventListener: () => {},
    removeEventListener: () => {},
    dispatchEvent: () => false,
    onchange: null,
    addListener: () => {},
    removeListener: () => {},
  })) as typeof window.matchMedia
}

let container: HTMLDivElement
let root: Root

beforeEach(() => {
  configurePersona.mockClear()
  installMatchMedia(false)
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  container = document.createElement('div')
  document.body.append(container)
  root = createRoot(container)
})

afterEach(async () => {
  await act(async () => root.unmount())
  container.remove()
})

describe('household ledger', () => {
  it('states readiness in words under the parlor header', async () => {
    await act(async () => {
      root.render(<Harness />)
    })
    expect(container.textContent).toContain('Household ledger')
    expect(container.textContent).toContain('Web UI configuration')
    expect(container.querySelector('.mc-ledger-header.mc-katyusha-lace')).not.toBeNull()
    const owed = container.querySelector('.mc-ledger-section[data-owed="true"]')
    expect(owed?.textContent).toContain('Missing')
    expect(owed?.textContent).toContain('Open Connections')
    expect(owed?.textContent).toContain('Ready')
    expect(owed?.textContent).toContain('Restart needed')
    expect(owed?.textContent).toContain('Run Setup Wizard')
    expect(container.textContent).not.toContain('LLM ready')
    expect(container.textContent).not.toContain('Cursor not ready')
    const connections = container.querySelector('[data-page="connections"]')
    expect(connections?.textContent).toContain('Missing')
    expect(connections?.getAttribute('data-active')).toBe('false')
    expect(container.textContent).toContain('Configure Kuro')
  })

  it('opens Connections from the readiness row and the index', async () => {
    await act(async () => {
      root.render(<Harness />)
    })
    const jump = Array.from(container.querySelectorAll('button')).find((button) => button.textContent === 'Open Connections')
    await act(async () => {
      jump?.click()
    })
    expect(container.querySelector('.mc-ledger-page-title')?.textContent).toBe('Connections')
    expect(container.querySelector('[data-page="connections"]')?.getAttribute('data-active')).toBe('true')
    expect(container.textContent).toContain('Connections body')
  })

  it('opens a ledger page from the phone index and returns to the list', async () => {
    installMatchMedia(true)
    await act(async () => {
      root.render(<Harness />)
    })
    expect(container.querySelector('.mc-ledger-page-title')).toBeNull()
    const environment = container.querySelector('[data-page="environment"]') as HTMLButtonElement
    await act(async () => {
      environment.click()
    })
    expect(container.querySelector('.mc-ledger-page-title')?.textContent).toBe('Environment')
    const back = Array.from(container.querySelectorAll('button')).find((button) => button.textContent === 'Back')
    await act(async () => {
      back?.click()
    })
    expect(container.querySelector('.mc-ledger-page-title')).toBeNull()
    expect(container.querySelector('[data-page="environment"]')).not.toBeNull()
  })

  it('opens the attendant dossier from the footer', async () => {
    await act(async () => {
      root.render(<Harness />)
    })
    const jump = Array.from(container.querySelectorAll('button')).find((button) => button.textContent === 'Configure Kuro')
    await act(async () => {
      jump?.click()
    })
    expect(configurePersona).toHaveBeenCalledWith()
  })
})
