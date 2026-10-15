import { describe, expect, it } from 'vitest'
import { createDraftSyncState, reduceDraftSync, type DraftSyncState } from './draft-sync'

function parent(
  state: DraftSyncState,
  draft: string,
  extras: { scopeChanged?: boolean; composing?: boolean } = {},
) {
  return reduceDraftSync(state, {
    type: 'parent',
    draft,
    scopeChanged: extras.scopeChanged ?? false,
    composing: extras.composing ?? false,
  })
}

function typed(state: DraftSyncState, text: string) {
  return reduceDraftSync(state, { type: 'composer', text })
}

describe('reduceDraftSync', () => {
  it('keeps rapid keystrokes when the parent draft is a render behind', () => {
    let state = createDraftSyncState()
    let effect = typed(state, 'h')
    expect(effect.publish).toBe('h')
    expect(effect.setComposer).toBeNull()
    state = effect.state

    effect = parent(state, '')
    expect(effect.setComposer).toBeNull()
    expect(effect.state.composer).toBe('h')
    state = effect.state

    effect = typed(state, 'he')
    expect(effect.publish).toBe('he')
    state = effect.state

    effect = parent(state, 'h')
    expect(effect.setComposer).toBeNull()
    expect(effect.publish).toBeNull()
    expect(effect.state.composer).toBe('he')
    state = effect.state

    effect = parent(state, 'he')
    expect(effect.setComposer).toBeNull()
    expect(effect.state.composer).toBe('he')
    expect(effect.state.localGeneration).toBe(effect.state.ackedGeneration)
  })

  it('does not republish when a poll rerenders the same draft', () => {
    let state = typed(createDraftSyncState(), 'kept').state
    state = parent(state, 'kept').state
    const again = parent(state, 'kept')
    expect(again.setComposer).toBeNull()
    expect(again.publish).toBeNull()
    expect(again.state.composer).toBe('kept')
  })

  it('applies a send clear only after local edits are acknowledged', () => {
    let state = typed(createDraftSyncState(), 'hello').state
    state = parent(state, 'hello').state
    const cleared = parent(state, '')
    expect(cleared.setComposer).toBe('')
    expect(cleared.publish).toBeNull()

    const staleEcho = typed(cleared.state, 'hello')
    expect(staleEcho.publish).toBeNull()
    expect(staleEcho.state.composer).toBe('')

    const echoed = typed(staleEcho.state, '')
    expect(echoed.publish).toBeNull()
    expect(echoed.state.suppressEcho).toBeNull()
  })

  it('does not clear unacknowledged text typed after a send', () => {
    let state = typed(createDraftSyncState(), 'hello').state
    state = parent(state, 'hello').state
    state = typed(state, 'next').state
    const clear = parent(state, '')
    expect(clear.setComposer).toBeNull()
    expect(clear.state.composer).toBe('next')
  })

  it('replaces the composer when the persona scope changes', () => {
    let state = typed(createDraftSyncState(), 'persona-a').state
    const switched = parent(state, 'persona-b draft', { scopeChanged: true })
    expect(switched.setComposer).toBe('persona-b draft')
    expect(switched.publish).toBeNull()
    const echo = typed(switched.state, 'persona-a')
    expect(echo.publish).toBeNull()
    expect(echo.state.composer).toBe('persona-b draft')
  })

  it('treats a paste as one local edit and ignores the previous parent value', () => {
    let state = typed(createDraftSyncState(), 'a').state
    state = parent(state, 'a').state
    const pasted = typed(state, 'a pasted sentence')
    expect(pasted.publish).toBe('a pasted sentence')
    const stale = parent(pasted.state, 'a')
    expect(stale.setComposer).toBeNull()
    expect(stale.state.composer).toBe('a pasted sentence')
  })

  it('does not apply an external draft during IME composition', () => {
    let state = typed(createDraftSyncState(), 'ni').state
    const during = parent(state, '', { composing: true })
    expect(during.setComposer).toBeNull()
    expect(during.state.composer).toBe('ni')
    state = typed(during.state, '你').state
    const after = parent(state, '')
    expect(after.setComposer).toBeNull()
    expect(after.state.composer).toBe('你')
  })

  it('restores a saved draft without echoing the empty composer', () => {
    const restored = parent(createDraftSyncState(), 'saved draft')
    expect(restored.setComposer).toBe('saved draft')
    const premature = typed(restored.state, '')
    expect(premature.publish).toBeNull()
    expect(premature.state.composer).toBe('saved draft')
    const caughtUp = typed(premature.state, 'saved draft')
    expect(caughtUp.publish).toBeNull()
    expect(caughtUp.state.suppressEcho).toBeNull()
  })
})
