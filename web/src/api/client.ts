const WEB_AUTH_STORAGE_KEY = 'web_auth_token'

export { WEB_AUTH_STORAGE_KEY }

export const AUTH_REQUIRED_EVENT = 'web-auth-required'

function sanitizeHttpHeaderValue(value: string): string | null {
  const trimmed = value.trim()
  if (!trimmed) return null
  if (trimmed.includes('\r') || trimmed.includes('\n')) return null
  for (let i = 0; i < trimmed.length; i += 1) {
    const code = trimmed.charCodeAt(i)
    if (code > 0xff) return null
  }
  return trimmed
}

/** Persist auth across browser restarts (migrates legacy sessionStorage once). */
export function getStoredAuthToken(): string | null {
  if (typeof localStorage === 'undefined') return null
  try {
    let t = localStorage.getItem(WEB_AUTH_STORAGE_KEY)
    if (!t && typeof sessionStorage !== 'undefined') {
      try {
        const legacy = sessionStorage.getItem(WEB_AUTH_STORAGE_KEY)
        if (legacy) {
          localStorage.setItem(WEB_AUTH_STORAGE_KEY, legacy)
          sessionStorage.removeItem(WEB_AUTH_STORAGE_KEY)
          t = legacy
        }
      } catch {
        // ignore migration failures
      }
    }
    if (!t) return null
    const sanitized = sanitizeHttpHeaderValue(t)
    if (!sanitized) {
      localStorage.removeItem(WEB_AUTH_STORAGE_KEY)
      return null
    }
    return sanitized
  } catch {
    return null
  }
}

export function setStoredAuthToken(token: string): void {
  if (typeof localStorage === 'undefined') return
  try {
    localStorage.setItem(WEB_AUTH_STORAGE_KEY, token)
    if (typeof sessionStorage !== 'undefined') {
      try {
        sessionStorage.removeItem(WEB_AUTH_STORAGE_KEY)
      } catch {
        // ignore
      }
    }
  } catch {
    // ignore
  }
}

export function makeHeaders(options: RequestInit = {}): HeadersInit {
  const headers: Record<string, string> = {
    ...(options.headers as Record<string, string> | undefined),
  }
  for (const [key, value] of Object.entries(headers)) {
    if (typeof value !== 'string') {
      delete headers[key]
      continue
    }
    const sanitized = sanitizeHttpHeaderValue(value)
    if (!sanitized) {
      delete headers[key]
      continue
    }
    headers[key] = sanitized
  }
  const token = getStoredAuthToken()
  if (token) {
    headers['Authorization'] = `Bearer ${token}`
  }
  if (options.body && !headers['Content-Type']) {
    headers['Content-Type'] = 'application/json'
  }
  return headers
}

function messageForFailedResponse(status: number, data: Record<string, unknown>, bodyText?: string): string {
  if (status === 401) {
    return 'Unauthorized. Enter the API token (WEB_AUTH_TOKEN from .env).'
  }
  if (status === 413) {
    const serverMsg = String(data.error || data.message || bodyText || '').trim()
    return serverMsg
      ? `Upload too large: ${serverMsg}`
      : 'Upload too large. Choose a smaller file or increase MAX_DOCUMENT_SIZE_MB in .env.'
  }
  if (status === 429) {
    const serverMsg = String(data.error || data.message || bodyText || '').trim()
    return serverMsg
      ? `Too many requests: ${serverMsg} Please wait a moment before sending again.`
      : 'Too many requests. Please wait a moment before sending again.'
  }
  return String(data.error || data.message || bodyText || `HTTP ${status}`)
}

export async function api<T>(path: string, options: RequestInit = {}): Promise<T> {
  if (path.startsWith('/api/history?') && getStoredAuthToken()) {
    await (await import('./artifacts')).ensureArtifactSession()
  }
  const res = await fetch(path, { ...options, headers: makeHeaders(options) })
  const bodyText = await res.text()
  let data: Record<string, unknown> = {}
  try {
    data = bodyText ? (JSON.parse(bodyText) as Record<string, unknown>) : {}
  } catch {
    data = { message: bodyText || undefined }
  }
  if (res.status === 401) {
    window.dispatchEvent(new CustomEvent(AUTH_REQUIRED_EVENT))
    throw new Error(messageForFailedResponse(401, data, bodyText))
  }
  if (!res.ok) {
    throw new Error(messageForFailedResponse(res.status, data, bodyText))
  }
  return data as T
}

/** Multipart/form-data requests (do not set Content-Type; browser adds boundary). */
export async function apiForm<T>(path: string, options: RequestInit = {}): Promise<T> {
  const headers = makeHeaders(options) as Record<string, string>
  delete headers['Content-Type']
  const res = await fetch(path, { ...options, headers })
  const bodyText = await res.text()
  let data: Record<string, unknown> = {}
  try {
    data = bodyText ? (JSON.parse(bodyText) as Record<string, unknown>) : {}
  } catch {
    data = { message: bodyText || undefined }
  }
  if (res.status === 401) {
    window.dispatchEvent(new CustomEvent(AUTH_REQUIRED_EVENT))
    throw new Error(messageForFailedResponse(401, data, bodyText))
  }
  if (!res.ok) {
    throw new Error(messageForFailedResponse(res.status, data, bodyText))
  }
  return data as T
}

export { sanitizeHttpHeaderValue }
