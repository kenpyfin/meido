import React, { act } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { MarkdownStream } from './markdown-stream'

const copyTextToClipboard = vi.fn((_text: string, _opts?: { trim?: boolean }) => Promise.resolve(true))
const downloadTextFile = vi.fn()

vi.mock('@/lib/copy-to-clipboard', () => ({
  copyTextToClipboard: (text: string, opts?: { trim?: boolean }) => copyTextToClipboard(text, opts),
}))
vi.mock('@/lib/download-file', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/lib/download-file')>()),
  downloadTextFile: (name: string, text: string) => downloadTextFile(name, text),
}))

async function waitFor(check: () => boolean) {
  for (let i = 0; i < 100; i += 1) {
    if (check()) return
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 20))
    })
  }
  throw new Error('timed out waiting for render')
}

describe('MarkdownStream', () => {
  let container: HTMLDivElement
  let root: Root

  beforeEach(() => {
    container = document.createElement('div')
    document.body.appendChild(container)
    root = createRoot(container)
  })

  afterEach(() => {
    act(() => root.unmount())
    container.remove()
    vi.clearAllMocks()
  })

  it('renders inline and display LaTeX with KaTeX', async () => {
    act(() => {
      root.render(<MarkdownStream>{'Inline $x^2$ and \\(y\\).\n\n\\[\n\\frac{1}{2}\n\\]'}</MarkdownStream>)
    })
    await waitFor(() => container.querySelectorAll('.katex').length >= 3)
    expect(container.querySelector('.katex-display')).not.toBeNull()
  })

  it('copies and downloads the untrimmed code block', async () => {
    act(() => {
      root.render(<MarkdownStream>{'```python\n    indented()\n```'}</MarkdownStream>)
    })
    await waitFor(() => !!container.querySelector('[data-streamdown="code-block-copy-button"]'))

    await act(async () => {
      container.querySelector<HTMLButtonElement>('[data-streamdown="code-block-copy-button"]')!.click()
    })
    expect(copyTextToClipboard).toHaveBeenCalledWith('    indented()\n', { trim: false })

    act(() => {
      container.querySelector<HTMLButtonElement>('[data-streamdown="code-block-download-button"]')!.click()
    })
    expect(downloadTextFile).toHaveBeenCalledWith('file.py', '    indented()\n')
  })
})
