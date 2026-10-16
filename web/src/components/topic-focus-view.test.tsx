import React, { act } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { TopicDossier } from '../lib/topic-focus'
import { TopicFocusView } from './topic-focus-view'

vi.mock('../api/artifacts', () => ({ ensureArtifactSession: () => Promise.resolve() }))

const dossier: TopicDossier = {
  ok: true,
  segmenting: false,
  topic: {
    id: 'topic-1',
    session_id: null,
    parent_id: null,
    continues_topic_id: null,
    title: 'Sourdough',
    summary: 'A generated note',
    start_message_id: 'm-start',
    end_message_id: 'm-2',
    start_ts: '2026-04-01T00:00:00Z',
    end_ts: '2026-04-01T01:00:00Z',
    message_count: 4,
    status: 'closed',
    source: 'auto',
    pinned: false,
    updated_at: '2026-04-01T01:00:00Z',
    side_chats: [{ id: 'side-1', title: 'Crumb', anchor_message_id: 'm-1', turn_count: 2 }],
  },
  messages: [
    {
      id: 'm-1',
      content: 'Look **starter** ![pic](/api/uploads/a.png)',
      is_from_bot: false,
      sender_name: 'web-user',
      timestamp: '2026-04-01T00:10:00Z',
    },
    { id: 'm-2', content: 'Reply with **crumb**', is_from_bot: true, sender_name: 'bot', timestamp: '2026-04-01T00:20:00Z' },
  ],
  has_older: true,
  has_newer: true,
  bookmarks: [
    { message_id: 'm-1', role: 'user', content_preview: 'Look' },
    { message_id: 'm-old', role: 'assistant', content_preview: 'Earlier note' },
  ],
  artifacts: [
    {
      id: 'art-1',
      name: 'crumb.png',
      kind: 'image',
      size_bytes: 10,
      status: 'active',
      availability: 'available',
      url: '/api/artifacts/art-1/content',
      preview_url: '/api/artifacts/art-1/content?preview=true',
    },
  ],
  evidence: { start_message_id: 'm-start', older_before_id: 'm-1', newer_after_id: 'm-2' },
}

let container: HTMLDivElement
let root: Root

function renderView(overrides: Partial<React.ComponentProps<typeof TopicFocusView>> = {}) {
  return act(async () => {
    root.render(
      <TopicFocusView
        topicTitle="Sourdough"
        assistantLabel="Baker"
        loading={false}
        error=""
        segmenting={false}
        dossier={dossier}
        pageKey="topic-1:latest"
        highlightMessageId={null}
        paging={false}
        onSelectSideChat={vi.fn()}
        onRetry={vi.fn()}
        onEarlier={vi.fn()}
        onLater={vi.fn()}
        onOpenAround={vi.fn()}
        onHighlight={vi.fn()}
        {...overrides}
      />,
    )
  })
}

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

describe('TopicFocusView', () => {
  it('renders rich turns, keeps outputs last, and pages inside the topic', async () => {
    const onEarlier = vi.fn()
    const onLater = vi.fn()
    const onHighlight = vi.fn()
    const onOpenAround = vi.fn()
    await renderView({ onEarlier, onLater, onHighlight, onOpenAround })
    expect(container.textContent).toContain('Generated summary')
    expect(container.textContent).toContain('A generated note')
    expect(container.textContent).toContain('You')
    expect(container.textContent).toContain('Baker')
    expect(container.textContent).toContain('Look')
    expect(container.textContent).toContain('starter')
    expect(container.textContent).toContain('crumb')
    expect(container.textContent).not.toContain('/api/uploads/a.png')
    expect(container.querySelector('[data-message-role="user"]')).toBeTruthy()
    expect(container.querySelector('[data-message-role="assistant"]')).toBeTruthy()
    expect(container.querySelector('[data-focus-message-count="2"]')).toBeTruthy()
    expect(container.querySelector('ol img')).toBeNull()
    expect(container.querySelector('img')).toBeNull()
    const outputs = container.querySelector('#dossier-outputs')
    const turns = container.querySelector('#dossier-messages')
    expect(outputs && turns && Boolean(turns.compareDocumentPosition(outputs) & Node.DOCUMENT_POSITION_FOLLOWING)).toBe(true)
    const labels = [...container.querySelectorAll('button')].map((button) => button.textContent)
    expect(labels).not.toContain('View source')
    expect(labels).not.toContain('Transcript')
    expect(labels).not.toContain('View in transcript')
    expect(labels).not.toContain('Open transcript')
    expect(container.textContent).toContain('Middle of this topic')
    container.querySelectorAll('button').forEach((button) => {
      if (button.textContent === 'Earlier') button.click()
      if (button.textContent === 'Later') button.click()
      if (button.textContent === 'Look') button.click()
      if (button.textContent === 'Earlier note') button.click()
    })
    expect(onEarlier).toHaveBeenCalled()
    expect(onLater).toHaveBeenCalled()
    expect(onHighlight).toHaveBeenCalledWith('m-1')
    expect(onOpenAround).toHaveBeenCalledWith('m-old')
  })

  it('opens markdown outputs in the reader and images in a viewer', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => ({ ok: true, text: async () => '# Notes\n\nBody' })),
    )
    await renderView({
      dossier: {
        ...dossier,
        artifacts: [
          ...dossier.artifacts,
          {
            id: 'art-md',
            name: 'notes.md',
            kind: 'markdown',
            size_bytes: 20,
            status: 'active',
            availability: 'available',
            url: '/api/artifacts/art-md/content',
            preview_url: '/api/artifacts/art-md/content?preview=true',
          },
        ],
      },
    })
    const read = container.querySelector('button[aria-label="Read notes.md"]')
    const view = container.querySelector('button[aria-label="View crumb.png"]')
    expect(read).toBeTruthy()
    expect(view).toBeTruthy()
    expect(container.querySelector('a[href="/api/artifacts/art-md/content"]')).toBeNull()
    await act(async () => {
      read?.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    })
    await act(async () => {
      await import('./markdown-file-dialog')
    })
    await act(async () => {
      await Promise.resolve()
    })
    expect(document.body.textContent).toContain('Open raw')
    expect(document.body.querySelector('.aui-md-root')).not.toBeNull()

    await act(async () => {
      view?.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    })
    await act(async () => {
      await Promise.resolve()
    })
    const image = document.body.querySelector('img')
    expect(image?.getAttribute('src')).toBe('/api/artifacts/art-1/content')
    expect(image?.getAttribute('alt')).toBe('crumb.png')
    vi.unstubAllGlobals()
  })

  it('retries a failed dossier without offering the transcript', async () => {
    const onRetry = vi.fn()
    await renderView({
      topicTitle: '',
      error: 'Could not load topic',
      dossier: null,
      onRetry,
    })
    const labels = [...container.querySelectorAll('button')].map((button) => button.textContent)
    expect(labels).toContain('Try again')
    expect(labels).not.toContain('Open transcript')
    const retry = [...container.querySelectorAll('button')].find((button) => button.textContent === 'Try again')
    retry?.click()
    expect(onRetry).toHaveBeenCalled()
  })
})
