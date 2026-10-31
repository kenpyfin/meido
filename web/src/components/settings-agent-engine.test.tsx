import React, { act } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { SettingsAgentEnginePanel } from './settings-agent-engine'

vi.mock('./settings-llm', () => ({ SettingsLlmPanel: () => <div>Model fields</div> }))
vi.mock('./settings-cursor', () => ({ SettingsCursorPanel: () => <div>Cursor fields</div> }))
vi.mock('./settings-gemini-adk', () => ({ SettingsGeminiAdkPanel: () => <div>Gemini fields</div> }))
vi.mock('./settings-run-context', () => ({ SettingsRunContextPanel: () => <div>Run context</div> }))
vi.mock('./llm-model-fields', () => ({ LocalModelFields: () => null }))
vi.mock('./confirm-dialog', () => ({ ConfirmDialog: () => null }))
vi.mock('@/components/ui/dialog', () => ({
  Dialog: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
  DialogContent: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
  DialogTitle: ({ children }: { children: React.ReactNode }) => <h2>{children}</h2>,
  DialogDescription: ({ children }: { children: React.ReactNode }) => <p>{children}</p>,
}))
vi.mock('@/components/ui/button', () => ({
  Button: ({ children, ...props }: React.ButtonHTMLAttributes<HTMLButtonElement>) => (
    <button {...props}>{children}</button>
  ),
}))
vi.mock('@/components/ui/input', () => ({
  Input: (props: React.InputHTMLAttributes<HTMLInputElement>) => <input {...props} />,
}))

function api(path: string, init?: RequestInit) {
  if (init?.method === 'PATCH' && path.includes('/bulletin')) return Promise.resolve({})
  if (path === '/api/runtime') {
    return Promise.resolve({ agent_engine: 'classic', max_tool_iterations: 100 })
  }
  if (path.includes('/bulletin')) {
    return Promise.resolve({
      agent_engine: { override: 'classic', global: 'classic', effective: 'classic', uses_default: false },
    })
  }
  if (path.includes('/api/llm')) return Promise.resolve({ model: 'gpt', provider: { id: 'openai' } })
  if (path.includes('local-delegate')) {
    return Promise.resolve({ base_url: 'http://127.0.0.1:8080/v1', model: 'local', tools_ok: true })
  }
  if (path.includes('cursor-engine')) return Promise.resolve({ sdk_model: 'composer', engine_ready: true })
  if (path.includes('topology')) return Promise.resolve({ profile: { default_model: 'gemini' } })
  if (path.includes('gemini-adk')) return Promise.resolve({ engine_ready: true, api_key_configured: true })
  return Promise.resolve({})
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

describe('agent engine ledger', () => {
  it('shows a 2×2 engine grid with live scope and a service note', async () => {
    const onError = vi.fn()
    await act(async () => {
      root.render(
        <SettingsAgentEnginePanel
          api={api as <T>(path: string, init?: RequestInit) => Promise<T>}
          onError={onError}
          activePersonaId={2}
          personas={[{ id: 2, name: 'Kuro', is_active: true }]}
        />,
      )
    })
    await act(async () => {
      await Promise.resolve()
    })
    expect(container.querySelector('.mc-engine-grid')).not.toBeNull()
    expect(container.querySelectorAll('.mc-engine-option')).toHaveLength(4)
    expect(container.textContent).toContain('Single turn')
    expect(container.textContent).toContain('Classic · Cost routing')
    expect(container.textContent).toContain('Gemini ADK')
    expect(container.textContent).toContain('Cursor (SDK)')
    expect(container.textContent).toContain('Applies live')
    expect(container.textContent).toContain('Service note')
    expect(container.textContent).toContain('Ready')
    expect(container.textContent).toContain('One cloud model')
    const active = container.querySelector('.mc-engine-option--active')
    expect(active?.textContent).toContain('Single turn')
    expect(active?.textContent).toContain('Ready')
    expect(container.querySelector('.mc-engine-config')?.textContent).toContain('Single turn')
    expect(container.textContent).toContain('Model fields')
    expect(onError).not.toHaveBeenCalled()
  })

  it('saves only a changed max tokens from Advanced', async () => {
    const onError = vi.fn()
    const spy = vi.fn((path: string, init?: RequestInit) => {
      if (path === '/api/runtime' && init?.method === 'PATCH') {
        return Promise.resolve({ max_tool_iterations: 100, max_tokens: 65536, message: 'ok' })
      }
      if (path === '/api/runtime') {
        return Promise.resolve({
          agent_engine: 'classic',
          max_tool_iterations: 100,
          max_tokens: 32768,
          max_tokens_min: 1024,
          max_tokens_max: 131072,
        })
      }
      return api(path, init)
    })
    await act(async () => {
      root.render(
        <SettingsAgentEnginePanel
          api={spy as <T>(path: string, init?: RequestInit) => Promise<T>}
          onError={onError}
          activePersonaId={2}
          personas={[{ id: 2, name: 'Kuro', is_active: true }]}
        />,
      )
    })
    const advanced = [...container.querySelectorAll('button')].find((b) => b.textContent === 'Advanced')
    await act(async () => advanced?.click())
    await act(async () => {
      await Promise.resolve()
    })
    const field = [...container.querySelectorAll('label')].find((l) =>
      l.textContent?.includes('Max tokens'),
    )
    const input = field?.querySelector('input') as HTMLInputElement
    expect(input.value).toBe('32768')
    await act(async () => {
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')?.set
      setter?.call(input, '65536')
      input.dispatchEvent(new Event('input', { bubbles: true }))
    })
    const save = [...container.querySelectorAll('button')].find((b) => b.textContent === 'Save')
    await act(async () => save?.click())
    const patch = spy.mock.calls.find(([p, i]) => p === '/api/runtime' && i?.method === 'PATCH')
    expect(JSON.parse(String(patch?.[1]?.body))).toEqual({ max_tokens: 65536 })
    expect(onError).not.toHaveBeenCalled()
  })
})
