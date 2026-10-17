import React, { act } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { api } from '../api/client'
import { CockpitBar } from './cockpit-bar'

vi.mock('../api/client', () => ({
  api: vi.fn(),
}))

let container: HTMLDivElement
let root: Root

function buttonByText(rootNode: ParentNode, label: string): HTMLButtonElement | undefined {
  return [...rootNode.querySelectorAll('button')].find((node) => node.textContent?.trim() === label)
}

async function renderBar(memo: string | null, pending = 0) {
  await act(async () => {
    root.render(
      <CockpitBar
        appearance="dark"
        statusText="Idle"
        queueLane={{
          chat_id: 1,
          persona_id: 2,
          pending,
          active_for_ms: 0,
          oldest_wait_ms: 0,
        }}
        backgroundActiveCount={pending > 0 ? 1 : 0}
        installationStatus={null}
        onQueueClick={() => {}}
        bookmarks={[]}
        activePersonaId={2}
        operatorMemoServer={memo}
        reloadBulletin={async () => {}}
        expanded
      />,
    )
  })
}

describe('CockpitBar memo clear', () => {
  beforeEach(() => {
    vi.mocked(api).mockReset()
    vi.mocked(api).mockResolvedValue({})
    container = document.createElement('div')
    document.body.appendChild(container)
    root = createRoot(container)
  })

  afterEach(() => {
    act(() => {
      root.unmount()
    })
    container.remove()
  })

  it('asks before clearing a saved steering note', async () => {
    await renderBar('Watch the queue')
    const clear = buttonByText(container, 'Clear')
    expect(clear).toBeTruthy()
    expect(clear?.disabled).toBe(false)

    await act(async () => {
      clear?.click()
    })

    expect(document.body.textContent).toContain('This removes the steering note from the system prompt.')
    expect(api).not.toHaveBeenCalled()

    await act(async () => {
      buttonByText(document.body, 'Clear memo')?.click()
    })

    expect(api).toHaveBeenCalledWith(
      '/api/personas/2/bulletin',
      expect.objectContaining({
        method: 'PATCH',
        body: JSON.stringify({ operator_memo: '' }),
      }),
    )
  })

  it('keeps Clear disabled when the saved memo is empty', async () => {
    await renderBar('')
    expect(buttonByText(container, 'Clear')?.disabled).toBe(true)
  })

  it('paints a live queue in brass', async () => {
    await renderBar(null, 2)
    const queue = container.querySelector('button[title="Open run queue"] span:last-child')
    expect(queue?.className).toContain('--mc-accent-brass')
    expect(queue?.textContent).toContain('2 pending')
  })
})
