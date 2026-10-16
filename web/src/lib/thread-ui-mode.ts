import { useSyncExternalStore } from 'react'

export type ThreadUiMode = 'legacy' | 'next'

export const THREAD_UI_STORAGE_KEY = 'finally-a-value-bot_thread_ui'

export function getThreadUiMode(): ThreadUiMode {
  if (typeof window === 'undefined') return 'next'
  try {
    const v = window.localStorage.getItem(THREAD_UI_STORAGE_KEY)
    return v === 'legacy' ? 'legacy' : 'next'
  } catch {
    return 'next'
  }
}

function subscribeThreadUiMode(onStoreChange: () => void): () => void {
  const onStorage = (e: StorageEvent) => {
    if (e.key === THREAD_UI_STORAGE_KEY || e.key === null) onStoreChange()
  }
  window.addEventListener('storage', onStorage)
  return () => window.removeEventListener('storage', onStorage)
}

export function useThreadUiMode(): ThreadUiMode {
  return useSyncExternalStore(subscribeThreadUiMode, getThreadUiMode, () => 'next')
}
