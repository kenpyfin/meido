import React, { act, useState } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { PersonaDialog } from './persona-dialog'
import type { PersonaDialogProps, PersonaLedgerPage } from './types'

vi.mock('../../components/settings-persona', () => ({
  SettingsPersonaPanel: () => <div>Profile body</div>,
}))
vi.mock('../../components/settings-agent-engine', () => ({
  SettingsAgentEnginePanel: ({ onOpenConnections }: { onOpenConnections?: () => void }) => (
    <button type="button" onClick={onOpenConnections}>Open Connections</button>
  ),
}))
vi.mock('../../components/settings-hooks-skills', () => ({
  SettingsHooksPanel: () => null,
  SettingsSkillsPanel: () => null,
}))
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

const openHousehold = vi.fn()
const openSettingsPage = vi.fn()

function props(): PersonaDialogProps {
  return {
    api: vi.fn(async () => ({ secrets: [] })) as PersonaDialogProps['api'],
    activePersonaId: 2,
    personas: [{ id: 2, name: 'Kuro', is_active: true }],
    persona: {
      open: true,
      onOpenChange: vi.fn(),
      error: '',
      setError: vi.fn(),
      onCreatePersona: vi.fn(),
      onRenamePersona: vi.fn(async () => {}),
      onDeletePersona: vi.fn(),
      page: 'profile',
      onPageChange: vi.fn(),
      revealPage: 0,
      onOpenHousehold: openHousehold,
      onOpenSettingsPage: openSettingsPage,
      engineMark: 'Missing',
    },
  }
}

function Harness({ initial = 'profile' as PersonaLedgerPage }) {
  const base = props()
  const [page, setPage] = useState<PersonaLedgerPage>(initial)
  const [revealPage, setRevealPage] = useState(0)
  return (
    <PersonaDialog
      {...base}
      persona={{
        ...base.persona,
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

let container: HTMLDivElement
let root: Root

beforeEach(() => {
  openHousehold.mockClear()
  openSettingsPage.mockClear()
  window.matchMedia = (() => ({
    matches: false,
    media: '',
    addEventListener: () => {},
    removeEventListener: () => {},
    dispatchEvent: () => false,
    onchange: null,
    addListener: () => {},
    removeListener: () => {},
  })) as typeof window.matchMedia
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  container = document.createElement('div')
  document.body.append(container)
  root = createRoot(container)
})

afterEach(async () => {
  await act(async () => root.unmount())
  container.remove()
})

describe('attendant dossier', () => {
  it('indexes the persona pages and marks a missing engine', async () => {
    await act(async () => {
      root.render(<Harness />)
    })
    expect(container.textContent).toContain('Attendant dossier')
    expect(container.textContent).toContain('Kuro')
    expect(container.querySelector('[data-page="engine"]')?.textContent).toContain('Missing')
    expect(container.textContent).toContain('Open household settings')
    expect(container.textContent).toContain('Profile body')
    expect(container.textContent).toContain('Applies live')
    expect(container.textContent).toContain('Forget removes a name stored for this persona.')
    expect(container.querySelector('.mc-service-note')).not.toBeNull()
  })

  it('opens household Connections from the engine page', async () => {
    await act(async () => {
      root.render(<Harness initial="engine" />)
    })
    const jump = Array.from(container.querySelectorAll('button')).find((button) => button.textContent === 'Open Connections')
    await act(async () => {
      jump?.click()
    })
    expect(openSettingsPage).toHaveBeenCalledWith('connections')
  })
})
