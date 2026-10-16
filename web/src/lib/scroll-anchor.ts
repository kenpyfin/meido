/** Keep the viewport visually still when older history is prepended. */

export type HeightScrollAnchor = {
  scrollTop: number
  scrollHeight: number
  messageId?: string
  offsetFromViewport?: number
}

export function captureHeightScrollAnchor(el: HTMLElement): HeightScrollAnchor {
  const anchor: HeightScrollAnchor = {
    scrollTop: el.scrollTop,
    scrollHeight: el.scrollHeight,
  }
  const vpRect = el.getBoundingClientRect()
  const nodes = el.querySelectorAll('[data-message-id]')
  for (const node of nodes) {
    if (!(node instanceof HTMLElement)) continue
    const messageId = node.getAttribute('data-message-id')
    if (!messageId) continue
    const rect = node.getBoundingClientRect()
    if (rect.bottom > vpRect.top + 1) {
      anchor.messageId = messageId
      anchor.offsetFromViewport = rect.top - vpRect.top
      break
    }
  }
  return anchor
}

/**
 * Re-apply a capture taken *before* prepended rows landed.
 * Prefers pinning the same message; falls back to height delta.
 */
export function applyHeightScrollAnchor(
  el: HTMLElement,
  anchor: HeightScrollAnchor,
): boolean {
  if (anchor.messageId != null && anchor.offsetFromViewport != null) {
    const msg = el.querySelector(
      `[data-message-id="${CSS.escape(anchor.messageId)}"]`,
    )
    if (msg instanceof HTMLElement) {
      const vpRect = el.getBoundingClientRect()
      const elRect = msg.getBoundingClientRect()
      const delta = elRect.top - vpRect.top - anchor.offsetFromViewport
      if (Math.abs(delta) >= 0.5) {
        el.scrollTop += delta
      }
      return true
    }
  }
  const heightDelta = el.scrollHeight - anchor.scrollHeight
  if (heightDelta <= 0) return false
  const next = anchor.scrollTop + heightDelta
  if (Math.abs(el.scrollTop - next) >= 0.5) {
    el.scrollTop = next
  }
  return true
}

type FrozenViewport = HTMLElement & {
  __mcScrollFrozen?: boolean
  __mcAllowScrollWrite?: boolean
}

/**
 * Ignore assistant-ui `scrollTo` / `scrollTop` writes while a prepend is in
 * flight. Our restore sets `__mcAllowScrollWrite` around its own assignment.
 */
export function installViewportScrollFreeze(el: HTMLElement): void {
  const frozen = el as FrozenViewport
  if (frozen.__mcScrollFrozen) return
  frozen.__mcScrollFrozen = true
  frozen.__mcAllowScrollWrite = false

  const originalScrollTo = el.scrollTo.bind(el)
  el.scrollTo = ((...args: Parameters<HTMLElement['scrollTo']>) => {
    if (el.dataset.mcHoldScroll === 'true' && !frozen.__mcAllowScrollWrite) return
    originalScrollTo(...args)
  }) as HTMLElement['scrollTo']

  const desc = Object.getOwnPropertyDescriptor(Element.prototype, 'scrollTop')
  if (!desc?.get || !desc?.set) return
  Object.defineProperty(el, 'scrollTop', {
    configurable: true,
    get() {
      return desc.get!.call(el) as number
    },
    set(value: number) {
      if (el.dataset.mcHoldScroll === 'true' && !frozen.__mcAllowScrollWrite) return
      desc.set!.call(el, value)
    },
  })
}

export function withAllowedScrollWrite(el: HTMLElement, write: () => void): void {
  const frozen = el as FrozenViewport
  frozen.__mcAllowScrollWrite = true
  try {
    write()
  } finally {
    frozen.__mcAllowScrollWrite = false
  }
}

export function setViewportHoldScroll(el: HTMLElement, hold: boolean): void {
  if (hold) el.dataset.mcHoldScroll = 'true'
  else delete el.dataset.mcHoldScroll
}

/**
 * Paint-freeze the current viewport with a clone so remounting history cannot
 * flash the top of the thread. Call the returned function to lift the cover.
 */
export function coverViewport(el: HTMLElement): () => void {
  const parent = el.parentElement
  if (!parent) return () => {}
  const computedPos = getComputedStyle(parent).position
  const prevPos = parent.style.position
  if (computedPos === 'static') {
    parent.style.position = 'relative'
  }
  const cover = document.createElement('div')
  cover.className = 'mc-thread-scroll-cover'
  cover.setAttribute('aria-hidden', 'true')
  cover.style.top = `${el.offsetTop}px`
  cover.style.left = `${el.offsetLeft}px`
  cover.style.width = `${el.offsetWidth}px`
  cover.style.height = `${el.offsetHeight}px`
  const inner = el.cloneNode(true) as HTMLElement
  inner.setAttribute('aria-hidden', 'true')
  inner.removeAttribute('id')
  inner.style.transform = `translateY(${-el.scrollTop}px)`
  inner.style.overflow = 'hidden'
  inner.style.height = `${el.scrollHeight}px`
  inner.style.width = `${el.clientWidth}px`
  inner.style.maxHeight = 'none'
  inner.style.flex = 'none'
  cover.appendChild(inner)
  parent.appendChild(cover)
  return () => {
    cover.remove()
    if (computedPos === 'static') {
      parent.style.position = prevPos
    }
  }
}

/** After the restored viewport has painted, lift the freeze cover. */
export function uncoverViewportAfterPaint(uncover: () => void): void {
  requestAnimationFrame(() => {
    requestAnimationFrame(uncover)
  })
}
