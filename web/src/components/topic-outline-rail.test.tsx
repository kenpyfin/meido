import React, { act } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { TopicOutlineTree } from './topic-outline-rail'
import type { ConversationTopic } from '../lib/conversation-topics'

const topics: ConversationTopic[] = [
  {
    id: 't1',
    session_id: null,
    parent_id: null,
    continues_topic_id: null,
    title: 'Deploy logs',
    summary: '',
    start_message_id: 'm1',
    end_message_id: 'm2',
    start_ts: '2026-04-01T00:00:00Z',
    end_ts: '2026-04-01T01:00:00Z',
    message_count: 4,
    status: 'closed',
    source: 'auto',
    pinned: false,
    side_chats: [
      { id: 's1', title: 'Rollback', anchor_message_id: 'm2', turn_count: 2 },
    ],
  },
  {
    id: 't2',
    session_id: null,
    parent_id: null,
    continues_topic_id: null,
    title: 'Budget',
    summary: '',
    start_message_id: 'm3',
    end_message_id: 'm4',
    start_ts: '2026-04-02T00:00:00Z',
    end_ts: '2026-04-02T01:00:00Z',
    message_count: 2,
    status: 'open',
    source: 'auto',
    pinned: false,
    side_chats: [],
  },
]

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

function renderTree(onJump = vi.fn()) {
  return act(async () => {
    root.render(
      <TopicOutlineTree
        topics={topics}
        activeTopicId={null}
        segmenting={false}
        unsegmentedCount={0}
        filter=""
        onFilterChange={() => {}}
        onJump={onJump}
        onSelectSideChat={() => {}}
        onRename={() => {}}
        onTogglePin={() => {}}
        onMerge={() => {}}
        onDelete={() => {}}
        onCreateSession={() => Promise.resolve()}
        onRefresh={() => {}}
      />,
    )
  })
}

describe('TopicOutlineTree', () => {
  it('renders topic titles newest first', async () => {
    await renderTree()
    const names = [...container.querySelectorAll('.mc-topic-rail-name')].map((node) => node.textContent)
    expect(names).toEqual(['Budget', 'Deploy logs'])
    expect(container.textContent).toContain('Rollback')
  })

  it('keeps rename, pin, merge, and delete behind one menu per topic', async () => {
    await renderTree()
    expect(container.textContent).not.toContain('Rename')
    expect(container.textContent).not.toContain('Merge')
    expect(container.textContent).not.toContain('Delete topic and messages')
    expect(container.querySelectorAll('[aria-label^="Topic actions for "]')).toHaveLength(2)
  })

  it('offers delete in the topic actions menu', async () => {
    const onDelete = vi.fn()
    await act(async () => {
      root.render(
        <TopicOutlineTree
          topics={topics}
          activeTopicId={null}
          segmenting={false}
          unsegmentedCount={0}
          filter=""
          onFilterChange={() => {}}
          onJump={() => {}}
          onSelectSideChat={() => {}}
          onRename={() => {}}
          onTogglePin={() => {}}
          onMerge={() => {}}
          onDelete={onDelete}
          onCreateSession={() => Promise.resolve()}
          onRefresh={() => {}}
        />,
      )
    })
    const trigger = container.querySelector('[aria-label="Topic actions for Budget"]')
    expect(trigger).toBeTruthy()
    await act(async () => {
      trigger?.dispatchEvent(
        new PointerEvent('pointerdown', { bubbles: true, cancelable: true, button: 0 }),
      )
    })
    const item = [...document.body.querySelectorAll('[role="menuitem"]')].find((node) =>
      node.textContent?.includes('Delete topic and messages'),
    )
    expect(item).toBeTruthy()
    await act(async () => {
      item?.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    })
    expect(onDelete).toHaveBeenCalledWith('t2')
  })

  it('creates a session from any topic menu', async () => {
    const onCreateSession = vi.fn().mockResolvedValue(undefined)
    await act(async () => {
      root.render(
        <TopicOutlineTree
          topics={topics}
          activeTopicId={null}
          segmenting={false}
          unsegmentedCount={0}
          filter=""
          onFilterChange={() => {}}
          onJump={() => {}}
          onSelectSideChat={() => {}}
          onRename={() => {}}
          onTogglePin={() => {}}
          onMerge={() => {}}
          onDelete={() => {}}
          onCreateSession={onCreateSession}
          onRefresh={() => {}}
        />,
      )
    })
    const trigger = container.querySelector('[aria-label="Topic actions for Deploy logs"]')
    await act(async () => {
      trigger?.dispatchEvent(
        new PointerEvent('pointerdown', { bubbles: true, cancelable: true, button: 0 }),
      )
    })
    const item = [...document.body.querySelectorAll('[role="menuitem"]')].find((node) =>
      node.textContent?.includes('Create new session'),
    )
    expect(item).toBeTruthy()
    expect(item?.getAttribute('aria-disabled')).not.toBe('true')
    expect(document.body.textContent).not.toContain('Agent engine')
    await act(async () => {
      item?.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    })
    const input = document.body.querySelector('[aria-label="Session title"]')
    expect(input).toBeTruthy()
    await act(async () => {
      const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')?.set
      setter?.call(input, 'Packaging')
      input?.dispatchEvent(new Event('input', { bubbles: true }))
    })
    const create = [...document.body.querySelectorAll('button')].find((node) =>
      node.textContent?.includes('Create session'),
    )
    expect(create).toBeTruthy()
    await act(async () => {
      create?.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    })
    expect(onCreateSession).toHaveBeenCalledWith('t1', 'Packaging')
  })

  it('jumps to the topic start on Enter', async () => {
    const onJump = vi.fn()
    await renderTree(onJump)
    const tree = container.querySelector('[role="tree"]')
    expect(tree).toBeTruthy()
    await act(async () => {
      tree?.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }))
    })
    expect(onJump).toHaveBeenCalledWith('m3')
  })

  it('moves to the older topic with ArrowDown before Enter', async () => {
    const onJump = vi.fn()
    await renderTree(onJump)
    const tree = container.querySelector('[role="tree"]')
    await act(async () => {
      tree?.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }))
    })
    await act(async () => {
      tree?.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }))
    })
    expect(onJump).toHaveBeenCalledWith('m1')
  })
})
