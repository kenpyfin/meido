import { describe, expect, it } from 'vitest'
import {
  applyHeightScrollAnchor,
  captureHeightScrollAnchor,
} from './scroll-anchor'

describe('height scroll anchor', () => {
  it('captures scrollTop and scrollHeight', () => {
    const el = document.createElement('div')
    Object.defineProperty(el, 'scrollTop', { value: 80, writable: true, configurable: true })
    Object.defineProperty(el, 'scrollHeight', { value: 2000, configurable: true })
    expect(captureHeightScrollAnchor(el)).toEqual({ scrollTop: 80, scrollHeight: 2000 })
  })

  it('adds the height delta to the original scrollTop', () => {
    const el = document.createElement('div')
    let top = 80
    Object.defineProperty(el, 'scrollTop', {
      configurable: true,
      get: () => top,
      set: (v: number) => {
        top = v
      },
    })
    Object.defineProperty(el, 'scrollHeight', { value: 3500, configurable: true })
    const ok = applyHeightScrollAnchor(el, { scrollTop: 80, scrollHeight: 2000 })
    expect(ok).toBe(true)
    expect(top).toBe(1580)
  })

  it('returns false until the DOM has actually grown', () => {
    const el = document.createElement('div')
    Object.defineProperty(el, 'scrollTop', { value: 80, writable: true, configurable: true })
    Object.defineProperty(el, 'scrollHeight', { value: 2000, configurable: true })
    expect(applyHeightScrollAnchor(el, { scrollTop: 80, scrollHeight: 2000 })).toBe(false)
  })
})
