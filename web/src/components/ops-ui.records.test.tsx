import React, { act } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { RecordsMenu } from './ops-ui'

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
  document.body.querySelectorAll('[data-radix-popper-content-wrapper]').forEach((node) => node.remove())
})

describe('RecordsMenu', () => {
  it('opens the menu on the Records button and runs the selected action', async () => {
    const onOpenArtifacts = vi.fn()
    await act(async () => {
      root.render(
        <RecordsMenu
          personaModule
          terminalAvailable={false}
          schedulesBadgeCount={0}
          agentHistoryDisabled={false}
          onOpenSchedules={vi.fn()}
          onOpenPrinciples={vi.fn()}
          onOpenArtifacts={onOpenArtifacts}
          onOpenMemory={vi.fn()}
          onOpenAgentHistory={vi.fn()}
        />,
      )
    })

    const trigger = document.querySelector('[aria-label="Open Records menu"]')
    expect(trigger).toBeInstanceOf(HTMLButtonElement)

    await act(async () => {
      trigger?.dispatchEvent(
        new PointerEvent('pointerdown', { bubbles: true, cancelable: true, button: 0 }),
      )
    })

    expect(trigger?.getAttribute('data-state')).toBe('open')
    const wrapper = document.querySelector('[data-radix-popper-content-wrapper]') as HTMLElement | null
    expect(wrapper).not.toBeNull()
    expect(wrapper?.style.transform ?? '').not.toContain('-200%')

    const artifacts = Array.from(document.querySelectorAll('[data-slot="dropdown-menu-item"]')).find(
      (el) => el.textContent === 'Artifacts',
    )
    expect(artifacts).toBeInstanceOf(HTMLElement)
    await act(async () => {
      artifacts?.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, cancelable: true, button: 0 }))
      artifacts?.dispatchEvent(new PointerEvent('pointerup', { bubbles: true, cancelable: true, button: 0 }))
      artifacts?.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true, button: 0 }))
    })
    expect(onOpenArtifacts).toHaveBeenCalledTimes(1)
  })
})
