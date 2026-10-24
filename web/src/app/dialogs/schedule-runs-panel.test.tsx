import React, { act } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ScheduleRunsPanel, type ScheduledRunItem } from './schedule-runs-panel'

type Api = React.ComponentProps<typeof ScheduleRunsPanel>['api']

function asApi(fn: (path: string, init?: RequestInit) => Promise<unknown>): Api {
  return fn as Api
}

vi.mock('@/lib/use-is-mobile', () => ({
  useIsMobile: () => false,
}))

vi.mock('@/components/ui/button', () => ({
  Button: ({ children, ...props }: React.ButtonHTMLAttributes<HTMLButtonElement>) => (
    <button {...props}>{children}</button>
  ),
}))

describe('ScheduleRunsPanel', () => {
  const runs: ScheduledRunItem[] = [
    {
      id: 1,
      task_id: 4,
      persona_id: 2,
      started_at: '2026-10-18T00:00:00Z',
      finished_at: '2026-10-18T00:01:00Z',
      status: 'ok',
      preview: 'first',
      read: false,
    },
    {
      id: 2,
      task_id: 4,
      persona_id: 2,
      started_at: '2026-10-18T01:00:00Z',
      finished_at: '2026-10-18T01:01:00Z',
      status: 'ok',
      preview: 'second',
      read: false,
    },
  ]
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

  it('marks every unread run read once when All runs is open', async () => {
    const calls: string[] = []
    const onAcknowledged = vi.fn()
    const api = asApi(async (path, init) => {
      calls.push(`${init?.method ?? 'GET'} ${path}`)
      if (path.startsWith('/api/schedules/runs?')) {
        return { runs: runs.map((run) => ({ ...run, read: true })) }
      }
      return { ok: true, cleared: 2 }
    })

    await act(async () => {
      root.render(
        <ScheduleRunsPanel
          api={api}
          chatId={7}
          acknowledgeUnread
          onAcknowledged={onAcknowledged}
        />,
      )
    })

    expect(calls).toEqual([
      'POST /api/schedules/runs/read?chat_id=7',
      'GET /api/schedules/runs?chat_id=7&limit=20',
    ])
    expect(onAcknowledged).toHaveBeenCalledTimes(1)
    expect(host.textContent).not.toContain('unread')
    expect(host.textContent).toContain('first')
    expect(host.textContent).toContain('second')
  })

  it('leaves a single task list unread until each run is opened', async () => {
    const calls: string[] = []
    const api = asApi(async (path) => {
      calls.push(path)
      return { runs }
    })

    await act(async () => {
      root.render(<ScheduleRunsPanel api={api} chatId={7} taskId={4} />)
    })

    expect(calls).toEqual(['/api/schedules/runs?chat_id=7&limit=20&task_id=4'])
    expect(host.textContent).toContain('unread')
    expect(host.textContent).toContain('Owner Persona #2')
  })

  it('opens run output in a separate dialog on a desk and names the owner', async () => {
    const api = asApi(async (path) => {
      if (path === '/api/schedules/runs/1') {
        return { run: { id: 1, persona_id: 2, status: 'ok', output: 'full output', error: null } }
      }
      return { runs }
    })

    await act(async () => {
      root.render(
        <ScheduleRunsPanel
          api={api}
          chatId={7}
          personas={[{ id: 2, name: 'Ledger' }]}
        />,
      )
    })

    expect(host.textContent).toContain('Owner Ledger')
    const row = Array.from(host.querySelectorAll('button')).find((button) =>
      button.textContent?.includes('#1'),
    )
    expect(row).toBeTruthy()
    await act(async () => {
      row?.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    })

    const dialog = document.body.querySelector('[data-slot="dialog-content"]')
    expect(dialog).toBeTruthy()
    expect(dialog?.classList.contains('mc-run-sheet')).toBe(true)
    const header = dialog?.querySelector('.mc-run-sheet-header')
    expect(header).toBeTruthy()
    expect(header?.textContent).toContain('Run #1')
    expect(header?.textContent).toContain('Owner Ledger')
    expect(dialog?.textContent).toContain('full output')
    expect(host.querySelector('[data-slot="dialog-content"]')).toBeNull()
    expect(document.body.textContent).not.toContain('Hide')
  })
})
