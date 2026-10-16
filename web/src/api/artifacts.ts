import { api, getStoredAuthToken } from './client'
let expires = 0
let token: string | null = null
let pending: Promise<void> | null = null
export function ensureArtifactSession(): Promise<void> {
  const current = getStoredAuthToken()
  if (current === token && Date.now() < expires) return Promise.resolve()
  if (pending) return pending
  pending = api('/api/artifacts/media-session', { method: 'POST' }).then(() => {
    token = current; expires = Date.now() + 30 * 60 * 1000
  }).finally(() => { pending = null })
  return pending
}
