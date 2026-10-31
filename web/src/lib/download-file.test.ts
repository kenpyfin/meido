import { afterEach, describe, expect, it, vi } from 'vitest'
import { codeFileExtension, downloadTextFile } from './download-file'

describe('codeFileExtension', () => {
  it('maps common language tags', () => {
    expect(codeFileExtension('python')).toBe('py')
    expect(codeFileExtension('TypeScript')).toBe('ts')
    expect(codeFileExtension('bash')).toBe('sh')
  })

  it('falls back to txt', () => {
    expect(codeFileExtension('')).toBe('txt')
    expect(codeFileExtension(undefined)).toBe('txt')
    expect(codeFileExtension('brainfuck')).toBe('txt')
  })
})

describe('downloadTextFile', () => {
  afterEach(() => {
    vi.useRealTimers()
    vi.restoreAllMocks()
  })

  it('clicks a download link and revokes the URL only after a delay', () => {
    vi.useFakeTimers()
    const createObjectURL = vi.fn(() => 'blob:test')
    const revokeObjectURL = vi.fn()
    Object.assign(URL, { createObjectURL, revokeObjectURL })
    const click = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {})

    downloadTextFile('file.py', 'print(1)\n')

    expect(click).toHaveBeenCalledTimes(1)
    const anchor = click.mock.contexts[0] as HTMLAnchorElement
    expect(anchor.download).toBe('file.py')
    expect(anchor.href).toBe('blob:test')
    expect(anchor.isConnected).toBe(false)
    expect(revokeObjectURL).not.toHaveBeenCalled()

    vi.advanceTimersByTime(1000)
    expect(revokeObjectURL).toHaveBeenCalledWith('blob:test')
  })
})
