import React, { act } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ArtifactLink } from './artifact-link'

const { apiMock } = vi.hoisted(() => ({ apiMock: vi.fn() }))
vi.mock('@/api/client', () => ({ api: apiMock }))
vi.mock('@/api/artifacts', () => ({ ensureArtifactSession: () => Promise.resolve() }))

const id = '11111111-1111-4111-8111-111111111111'
const item = (kind: string, name: string) => ({
  id,
  name,
  kind,
  size_bytes: 128,
  status: 'active' as const,
  pinned: false,
  source: 'generated',
  source_path: '/source',
  sha256: 'hash',
  url: `/api/artifacts/${id}/content`,
  preview_url: `/api/artifacts/${id}/content?preview=true`,
})

let container: HTMLDivElement
let root: Root

beforeEach(() => {
  apiMock.mockReset()
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  container = document.createElement('div')
  document.body.append(container)
  root = createRoot(container)
})

afterEach(async () => {
  await act(async () => root.unmount())
  container.remove()
})

describe('ArtifactLink', () => {
  it('opens a markdown file in the reader', async () => {
    apiMock.mockResolvedValue(item('markdown', 'notes.md'))
    vi.stubGlobal('fetch', vi.fn(async () => ({ ok: true, text: async () => '# Notes\n\nBody' })))
    await act(async () => root.render(<ArtifactLink id={id} />))
    await act(async () => { await Promise.resolve() })
    const open = container.querySelector('button')
    expect(open?.textContent).toContain('notes.md')
    expect(container.querySelector('a')).toBeNull()
    await act(async () => open!.click())
    await act(async () => { await import('./markdown-file-dialog') })
    expect(document.body.textContent).toContain('Open raw')
    expect(document.body.querySelector('.aui-md-root')).not.toBeNull()
    vi.unstubAllGlobals()
  })

  it('keeps other files as a new-tab link', async () => {
    apiMock.mockResolvedValue(item('image', 'shot.png'))
    await act(async () => root.render(<ArtifactLink id={id} />))
    await act(async () => { await Promise.resolve() })
    const link = container.querySelector('a')
    expect(link?.getAttribute('href')).toBe(`/api/artifacts/${id}/content`)
    expect(link?.textContent).toContain('shot.png')
    expect(container.querySelector('button')).toBeNull()
  })
})
