import { describe, expect, it } from 'vitest'
import { normalizeMathDelimiters } from './math-delimiters'

describe('normalizeMathDelimiters', () => {
  it('returns text without LaTeX delimiters unchanged', () => {
    const text = 'Costs $5 and *more*.'
    expect(normalizeMathDelimiters(text)).toBe(text)
  })

  it('converts inline \\( \\) to single dollars', () => {
    expect(normalizeMathDelimiters('Euler: \\( e^{i\\pi} + 1 = 0 \\) holds.')).toBe(
      'Euler: $e^{i\\pi} + 1 = 0$ holds.',
    )
  })

  it('converts a standalone \\[ \\] line into a display block', () => {
    expect(normalizeMathDelimiters('Sum:\n\\[ \\sum_n \\frac{1}{n^2} \\]\nDone')).toBe(
      'Sum:\n$$\n\\sum_n \\frac{1}{n^2}\n$$\nDone',
    )
  })

  it('converts multiline display math', () => {
    expect(normalizeMathDelimiters('\\[\na = b\n\\]')).toBe('$$\na = b\n$$')
  })

  it('keeps mid-sentence \\[ \\] inline', () => {
    expect(normalizeMathDelimiters('so \\[x\\] here')).toBe('so $$x$$ here')
  })

  it('leaves fenced code blocks alone', () => {
    const text = '```latex\n\\( x \\) and \\[ y \\]\n```\nthen \\(z\\)'
    expect(normalizeMathDelimiters(text)).toBe('```latex\n\\( x \\) and \\[ y \\]\n```\nthen $z$')
  })

  it('leaves inline code spans alone', () => {
    expect(normalizeMathDelimiters('use `\\(x\\)` for \\(y\\)')).toBe('use `\\(x\\)` for $y$')
  })
})
