import React, { act } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { Persona, PersonaTodo } from '../../types'
import { BoardView } from './board-view'
import { InboxPanel } from '../../components/inbox-panel'

const api = vi.fn()

vi.mock('../../api/client', () => ({
  api: (...args: unknown[]) => api(...args),
}))

vi.mock('@/components/markdown-stream', () => ({
  MarkdownStream: ({ children }: { children?: string }) => <div>{children}</div>,
}))

vi.mock('@/components/ui/button', () => ({
  Button: ({ children, ...props }: React.ButtonHTMLAttributes<HTMLButtonElement>) => (
    <button {...props}>{children}</button>
  ),
}))

vi.mock('@/components/ui/dialog', () => ({
  Dialog: ({ open, children }: { open?: boolean; children: React.ReactNode }) =>
    open ? <div>{children}</div> : null,
  DialogContent: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
  DialogTitle: ({ children }: { children: React.ReactNode }) => <h2>{children}</h2>,
  DialogDescription: ({ children }: { children: React.ReactNode }) => <p>{children}</p>,
  DialogClose: ({ children }: { children: React.ReactNode }) => <>{children}</>,
}))

const personas: Persona[] = [
  {
    id: 2,
    name: 'Latch',
    chat_id: 7,
  } as Persona,
]

function ticket(overrides: Partial<PersonaTodo> = {}): PersonaTodo {
  return {
    id: 12,
    chat_id: 7,
    persona_id: 2,
    title: 'Fix the latch',
    status: 'todo',
    created_at: '2026-10-21T00:00:00Z',
    updated_at: '2026-10-21T00:00:00Z',
    description: 'Plan: replace the strike plate.\n\nBuild: filed the edge.',
    deploy_prompt: 'Keep the brass hinge.',
    owner_persona_id: 2,
    auto_advance: false,
    ...overrides,
  }
}

describe('BoardView', () => {
  let host: HTMLDivElement
  let root: Root

  beforeEach(() => {
    api.mockReset()
    host = document.createElement('div')
    document.body.appendChild(host)
    root = createRoot(host)
  })

  afterEach(() => {
    act(() => root.unmount())
    host.remove()
  })

  it('shows four columns, deploy and complete on every card, and history only in the detail view', async () => {
    const todos = [
      ticket(),
      ticket({ id: 13, title: 'Hang the door', status: 'verify', description: 'Checked the swing.', deploy_prompt: '' }),
    ]
    api.mockImplementation(async (path: string) => {
      if (String(path).includes('/api/todos?')) return { todos }
      if (String(path).includes('/events')) {
        return {
          events: [
            {
              id: 1,
              from_status: 'todo',
              to_status: 'plan',
              actor_kind: 'operator',
              at: '2026-10-21T01:00:00Z',
            },
          ],
        }
      }
      return { proposals: [] }
    })

    await act(async () => {
      root.render(<BoardView chatId={7} personas={personas} />)
    })

    const text = host.textContent ?? ''
    expect(text).toContain('TODO')
    expect(text).toContain('PLAN')
    expect(text).toContain('BUILD')
    expect(text).toContain('VERIFY')
    expect(text).not.toContain('DESIGN')
    expect(text).not.toContain('IMPLEMENT')
    expect(host.querySelectorAll('button').length).toBeGreaterThan(0)
    const buttons = [...host.querySelectorAll('button')].map((button) => button.textContent)
    expect(buttons.filter((label) => label === 'Deploy')).toHaveLength(2)
    expect(buttons.filter((label) => label === 'Complete')).toHaveLength(2)
    expect(text).not.toContain('Plan: replace the strike plate.')

    const card = host.querySelector('#board-card-12')
    expect(card).toBeTruthy()
    await act(async () => {
      card?.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    })

    const dialogText = document.body.textContent ?? ''
    expect(dialogText).toContain('Ticket #12')
    expect(dialogText).toContain('Plan: replace the strike plate.')
    expect(dialogText).toContain('TODO → PLAN')
    const prompt = document.body.querySelector('textarea[aria-label="Deploy prompt"]') as HTMLTextAreaElement | null
    expect(prompt?.value).toBe('Keep the brass hinge.')
    expect(dialogText).toContain('Deploy')
    expect(dialogText).toContain('Complete')
  })

  it('opens the focused ticket from the inbox', async () => {
    api.mockImplementation(async (path: string) => {
      if (String(path).includes('/api/todos?')) return { todos: [ticket({ status: 'build' })] }
      if (String(path).includes('/events')) return { events: [] }
      return { proposals: [] }
    })
    const onFocusTodoHandled = vi.fn()

    await act(async () => {
      root.render(
        <BoardView chatId={7} personas={personas} focusTodoId={12} onFocusTodoHandled={onFocusTodoHandled} />,
      )
    })

    expect(document.body.textContent).toContain('Ticket #12')
    expect(onFocusTodoHandled).toHaveBeenCalled()
  })
})

describe('InboxPanel todos', () => {
  let host: HTMLDivElement
  let root: Root

  beforeEach(() => {
    host = document.createElement('div')
    document.body.appendChild(host)
    root = createRoot(host)
  })

  afterEach(() => {
    act(() => root.unmount())
    host.remove()
  })

  it('opens the board ticket instead of the persona chat', async () => {
    const onOpenTodo = vi.fn()
    const onOpenTarget = vi.fn()
    const onOpenChange = vi.fn()
    await act(async () => {
      root.render(
        <InboxPanel
          appearance="dark"
          open
          onOpenChange={onOpenChange}
          unread={[]}
          todos={[ticket()]}
          personas={personas}
          loading={false}
          busyTodoId={null}
          onRefresh={() => undefined}
          onOpenTarget={onOpenTarget}
          onOpenTodo={onOpenTodo}
          onCompleteTodo={() => undefined}
        />,
      )
    })

    const open = [...host.querySelectorAll('button')].find((button) => button.textContent === 'Open')
    await act(async () => {
      open?.click()
    })
    expect(onOpenTodo).toHaveBeenCalledWith(12)
    expect(onOpenTarget).not.toHaveBeenCalled()
    expect(onOpenChange).toHaveBeenCalledWith(false)
  })
})
