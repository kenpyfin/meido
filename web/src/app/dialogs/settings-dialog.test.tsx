import React, { act } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { SettingsDialog } from './settings-dialog'
import type { SettingsDialogProps } from './types'

vi.mock('../../components/settings-runtime', () => ({
  SettingsRuntimePanel: () => <div>Runtime toggles body</div>,
}))
vi.mock('../../components/settings-integrations', () => ({ SettingsIntegrationsPanel: () => null }))
vi.mock('../../components/settings-connections', () => ({ SettingsConnectionsPanel: () => null }))
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

function props(): SettingsDialogProps {
  return {
    appearance: 'dark',
    api: vi.fn(),
    chatId: 1,
    activePersonaId: 2,
    personas: [],
    onRunSetupWizard: vi.fn(),
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
    },
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

describe('household ledger', () => {
  it('states readiness in words under the parlor header', async () => {
    await act(async () => {
      root.render(<SettingsDialog {...props()} />)
    })
    expect(container.textContent).toContain('Household ledger')
    expect(container.textContent).toContain('Web UI configuration')
    expect(container.querySelector('.mc-ledger-header.mc-katyusha-lace')).not.toBeNull()
    const owed = container.querySelector('.mc-ledger-section[data-owed="true"]')
    expect(owed?.textContent).toContain('Missing')
    expect(owed?.textContent).toContain('Open Connections.')
    expect(owed?.textContent).toContain('Ready')
    expect(owed?.textContent).toContain('Restart needed')
    expect(owed?.textContent).toContain('Run Setup Wizard to finish setup.')
    expect(container.textContent).not.toContain('LLM ready')
    expect(container.textContent).not.toContain('Cursor not ready')
  })
})
