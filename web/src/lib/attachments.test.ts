import { afterEach, describe, expect, it, vi } from 'vitest'
import { formatUploadBytes, splitDataUrl, uploadAttachmentFile } from './attachments'

describe('formatUploadBytes', () => {
  it('formats byte sizes', () => {
    expect(formatUploadBytes(512)).toBe('512 B')
    expect(formatUploadBytes(2048)).toBe('2.0 KB')
    expect(formatUploadBytes(10 * 1024 * 1024)).toBe('10.0 MB')
  })
})

describe('splitDataUrl', () => {
  it('parses data URLs', () => {
    const parsed = splitDataUrl('data:image/png;base64,abc123')
    expect(parsed?.mimeType).toBe('image/png')
    expect(parsed?.base64).toBe('abc123')
  })
})

describe('uploadAttachmentFile', () => {
  afterEach(() => {
    vi.restoreAllMocks()
    vi.unstubAllGlobals()
  })

  it('rejects with a timeout message when the request never settles', async () => {
    // fetch that respects the abort signal but otherwise never resolves.
    const fetchMock = vi.fn(
      (_url: string, init?: RequestInit) =>
        new Promise((_resolve, reject) => {
          const signal = init?.signal
          signal?.addEventListener('abort', () =>
            reject(signal.reason ?? new DOMException('aborted', 'AbortError')),
          )
        }) as Promise<Response>,
    )
    vi.stubGlobal('fetch', fetchMock)

    const file = new File(['x'], 'stuck.bin', { type: 'application/octet-stream' })
    await expect(uploadAttachmentFile(file, 1, { timeoutMs: 20 })).rejects.toThrow(/timed out/i)
    expect(fetchMock).toHaveBeenCalledTimes(1)
  })

  it('propagates caller aborts without the timeout message', async () => {
    const fetchMock = vi.fn(
      (_url: string, init?: RequestInit) =>
        new Promise((_resolve, reject) => {
          const signal = init?.signal
          signal?.addEventListener('abort', () =>
            reject(signal.reason ?? new DOMException('aborted', 'AbortError')),
          )
        }) as Promise<Response>,
    )
    vi.stubGlobal('fetch', fetchMock)

    const controller = new AbortController()
    const file = new File(['x'], 'cancel.bin', { type: 'application/octet-stream' })
    const promise = uploadAttachmentFile(file, 1, { signal: controller.signal, timeoutMs: 5_000 })
    controller.abort(new DOMException('user cancelled', 'AbortError'))
    await expect(promise).rejects.not.toThrow(/timed out/i)
  })
})
