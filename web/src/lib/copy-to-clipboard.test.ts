import { describe, expect, it, vi } from 'vitest'
import { copyTextToClipboard } from './copy-to-clipboard'

describe('copyTextToClipboard', () => {
  it('returns false for empty text', async () => {
    await expect(copyTextToClipboard('   ')).resolves.toBe(false)
  })

  it('uses navigator.clipboard when available', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined)
    vi.stubGlobal('navigator', { clipboard: { writeText } })

    await expect(copyTextToClipboard('hello')).resolves.toBe(true)
    expect(writeText).toHaveBeenCalledWith('hello')

    vi.unstubAllGlobals()
  })
})
