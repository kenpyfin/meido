import React, { act } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ArtifactsDialog } from './artifacts-dialog'
import type { ArtifactsDialogProps } from './types'
const { apiMock } = vi.hoisted(() => ({ apiMock: vi.fn() }))
vi.mock('@/api/client', () => ({ api: apiMock }))
vi.mock('@/api/artifacts', () => ({ ensureArtifactSession: () => Promise.resolve() }))
vi.mock('@/components/ui/dialog', () => ({
  Dialog: ({ children }: any) => <div>{children}</div>,
  DialogContent: ({ children }: any) => <div>{children}</div>,
  DialogTitle: ({ children }: any) => <h2>{children}</h2>,
  DialogDescription: ({ children }: any) => <p>{children}</p>,
}))
vi.mock('@/components/ui/button', () => ({ Button: ({ children, variant: _v, size: _s, ...props }: any) => <button {...props}>{children}</button> }))
const file = (id: string) => ({ id, name: `${id}.mp4`, kind: 'video', size_bytes: 128, status: 'active', pinned: false, source: 'generated', source_path: '/source', url: `/api/artifacts/${id}/content`, preview_url: `/api/artifacts/${id}/content?preview=true` })
const props = (pid = 2): ArtifactsDialogProps => ({ appearance: 'dark', chatId: 1, activePersonaId: pid, artifacts: { open: true, onOpenChange: vi.fn(), onOpenMessage: vi.fn() } })
let container: HTMLDivElement
let root: Root
async function tick() { await act(async () => { await vi.advanceTimersByTimeAsync(250) }) }
beforeEach(() => {
  vi.useFakeTimers(); apiMock.mockReset()
  ;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true
  container = document.createElement('div'); document.body.append(container); root = createRoot(container)
})
afterEach(async () => { await act(async () => root.unmount()); container.remove(); vi.useRealTimers() })
describe('artifact library', () => {
  it('replaces the page instead of growing the rendered list', async () => {
    apiMock.mockImplementation((path: string) => Promise.resolve(path.includes('cursor=next')
      ? { artifacts: [file('older')], next_cursor: null, indexing: false }
      : { artifacts: Array.from({ length: 50 }, (_, i) => file(`take-${i}`)), next_cursor: 'next', indexing: false }))
    await act(async () => root.render(<ArtifactsDialog {...props()} />)); await tick()
    expect(container.querySelectorAll('input[aria-label^="Select take-"]')).toHaveLength(50)
    const next = [...container.querySelectorAll('button')].find(b => b.textContent === 'Next')!
    await act(async () => next.click()); await tick()
    expect(container.querySelectorAll('input[aria-label^="Select take-"]')).toHaveLength(0)
    expect(container.querySelector('input[aria-label="Select older.mp4"]')).not.toBeNull()
  })
  it('preserves bulk selection across background refreshes', async () => {
    apiMock.mockResolvedValue({ artifacts: [file('selected')], next_cursor: null, indexing: false })
    await act(async () => root.render(<ArtifactsDialog {...props()} />)); await tick()
    const checkbox = () => container.querySelector<HTMLInputElement>('input[aria-label="Select selected.mp4"]')!
    await act(async () => checkbox().click())
    expect(checkbox().checked).toBe(true)
    await act(async () => { await vi.advanceTimersByTimeAsync(15000) }); await tick()
    expect(apiMock.mock.calls.length).toBeGreaterThan(1)
    expect(checkbox().checked).toBe(true)
  })
  it('shows when unpinned trash will be removed', async () => {
    apiMock.mockResolvedValue({ artifacts: [{ ...file('old'), status: 'trash', expires_at: '2026-11-06T19:00:00Z' }], next_cursor: null, indexing: false, trash_retention_days: 30 })
    await act(async () => root.render(<ArtifactsDialog {...props()} />)); await tick()
    const view = container.querySelector<HTMLSelectElement>('select[aria-label="Library view"]')!
    await act(async () => { view.value = 'trash'; view.dispatchEvent(new Event('change', { bubbles: true })) }); await tick()
    expect(container.textContent).toContain('Unpinned files stay in Trash for 30 days')
    expect(container.textContent).toContain('Removed')
  })
  it('renders markdown as a document and leaves plain text as source', async () => {
    vi.stubGlobal('fetch', vi.fn(async (url: string) => ({
      ok: true,
      text: async () => (String(url).includes('notes') ? '# Notes\n\nBody' : 'plain line'),
    })))
    apiMock.mockResolvedValue({
      artifacts: [
        { ...file('notes'), name: 'notes.md', kind: 'markdown' },
        { ...file('log'), name: 'log.txt', kind: 'text' },
      ],
      next_cursor: null,
      indexing: false,
    })
    await act(async () => root.render(<ArtifactsDialog {...props()} />)); await tick()
    const clickNamed = async (name: string) => {
      const button = [...container.querySelectorAll('button')].find((b) => b.textContent?.includes(name))
      expect(button).toBeTruthy()
      await act(async () => button!.click())
      await act(async () => { await Promise.resolve() })
      await tick()
    }
    await clickNamed('notes.md')
    expect(container.querySelector('.aui-md-root')).not.toBeNull()
    expect(container.querySelector('pre')).toBeNull()
    await clickNamed('log.txt')
    expect(container.querySelector('pre')?.textContent).toContain('plain line')
    vi.unstubAllGlobals()
  })
  it('opens a file on its own page on a phone', async () => {
    vi.stubGlobal('matchMedia', () => ({
      matches: true,
      media: '(max-width: 767px)',
      addEventListener: () => {},
      removeEventListener: () => {},
    }))
    apiMock.mockResolvedValue({ artifacts: [file('clip'), file('still')], next_cursor: null, indexing: false })
    await act(async () => root.render(<ArtifactsDialog {...props()} />)); await tick()
    expect(container.querySelector('video')).toBeNull()
    const open = [...container.querySelectorAll('button')].find((b) => b.textContent?.includes('clip.mp4'))
    expect(open).toBeTruthy()
    await act(async () => open!.click())
    expect(container.querySelector('video')).not.toBeNull()
    expect(container.querySelector('input[aria-label="Select still.mp4"]')).toBeNull()
    const back = [...container.querySelectorAll('button')].find((b) => b.textContent === 'Back')
    expect(back).toBeTruthy()
    await act(async () => back!.click())
    expect(container.querySelector('video')).toBeNull()
    expect(container.querySelector('input[aria-label="Select still.mp4"]')).not.toBeNull()
    vi.unstubAllGlobals()
  })
  it('ignores a late response from the previous persona', async () => {
    let finish: (value: unknown) => void = () => {}
    apiMock.mockImplementation((path: string) => path.includes('persona_id=2')
      ? new Promise(resolve => { finish = resolve })
      : Promise.resolve({ artifacts: [file('right-persona')], next_cursor: null, indexing: false }))
    await act(async () => root.render(<ArtifactsDialog {...props()} />)); await tick()
    await act(async () => root.render(<ArtifactsDialog {...props(3)} />)); await tick()
    await act(async () => finish({ artifacts: [file('wrong-persona')], next_cursor: null, indexing: false }))
    expect(container.textContent).toContain('right-persona.mp4')
    expect(container.textContent).not.toContain('wrong-persona.mp4')
  })
})
