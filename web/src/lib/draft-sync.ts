/** Composer text the parent has not acknowledged yet stays local. */

export type DraftSyncState = {
  composer: string
  localGeneration: number
  ackedGeneration: number
  /** Ignore composer echoes until the store matches this applied value. */
  suppressEcho: string | null
}

export type DraftSyncEvent =
  | { type: 'composer'; text: string }
  | { type: 'parent'; draft: string; scopeChanged: boolean; composing: boolean }

export type DraftSyncEffect = {
  state: DraftSyncState
  setComposer: string | null
  publish: string | null
}

export function createDraftSyncState(): DraftSyncState {
  return {
    composer: '',
    localGeneration: 0,
    ackedGeneration: 0,
    suppressEcho: null,
  }
}

function applyExternal(state: DraftSyncState, draft: string): DraftSyncEffect {
  if (draft === state.composer) {
    return {
      state: { ...state, ackedGeneration: state.localGeneration },
      setComposer: null,
      publish: null,
    }
  }
  const generation = state.localGeneration + 1
  return {
    state: {
      composer: draft,
      localGeneration: generation,
      ackedGeneration: generation,
      suppressEcho: draft,
    },
    setComposer: draft,
    publish: null,
  }
}

/**
 * Local keystrokes win until the parent catches up. Parent text is written
 * into the composer only for a scope change or when no local edit is waiting.
 */
export function reduceDraftSync(state: DraftSyncState, event: DraftSyncEvent): DraftSyncEffect {
  if (event.type === 'composer') {
    if (state.suppressEcho != null) {
      if (event.text !== state.suppressEcho) {
        return { state, setComposer: null, publish: null }
      }
      return {
        state: { ...state, composer: event.text, suppressEcho: null },
        setComposer: null,
        publish: null,
      }
    }
    if (event.text === state.composer) {
      return { state, setComposer: null, publish: null }
    }
    return {
      state: {
        ...state,
        composer: event.text,
        localGeneration: state.localGeneration + 1,
        suppressEcho: null,
      },
      setComposer: null,
      publish: event.text,
    }
  }

  if (event.composing && !event.scopeChanged) {
    return { state, setComposer: null, publish: null }
  }
  if (event.scopeChanged) {
    return applyExternal(state, event.draft)
  }
  if (event.draft === state.composer) {
    return {
      state: { ...state, ackedGeneration: state.localGeneration },
      setComposer: null,
      publish: null,
    }
  }
  if (state.localGeneration > state.ackedGeneration) {
    return { state, setComposer: null, publish: null }
  }
  return applyExternal(state, event.draft)
}
