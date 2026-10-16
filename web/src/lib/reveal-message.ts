/** Add the brief pulse/anchor highlight to a message bubble without moving the scroll position. */
export function flashMessageElement(el: HTMLElement, durationMs = 2200): void {
  el.classList.add('mc-msg-subthread-anchor', 'mc-msg-reveal-flash')
  window.setTimeout(() => {
    el.classList.remove('mc-msg-reveal-flash')
    // Keep subthread-anchor only if it was already an active side-chat anchor.
    if (el.getAttribute('data-subthread-anchor') !== 'true') {
      el.classList.remove('mc-msg-subthread-anchor')
    }
  }, durationMs)
}

/** Highlight a mounted message bubble briefly and scroll it into view (nearest scroll ancestor). */
export function highlightMessageElement(el: HTMLElement, durationMs = 2200): void {
  el.scrollIntoView({ block: 'center', behavior: 'smooth' })
  flashMessageElement(el, durationMs)
}

/**
 * Precisely center a message inside a specific scroll viewport. Returns false when the
 * element is not inside the viewport. Setting scrollTop away from the bottom also clears
 * assistant-ui's `isAtBottom` flag so subsequent resize observers do not snap to bottom.
 */
export function centerMessageInViewport(viewport: HTMLElement, el: HTMLElement): boolean {
  const vpRect = viewport.getBoundingClientRect()
  const elRect = el.getBoundingClientRect()
  const target =
    viewport.scrollTop + (elRect.top - vpRect.top) - vpRect.height / 2 + elRect.height / 2
  const max = viewport.scrollHeight - viewport.clientHeight
  const clamped = Math.max(0, Math.min(target, max))
  viewport.scrollTo({ top: clamped, behavior: 'smooth' })
  return true
}

export function findMessageElement(messageId: string): HTMLElement | null {
  if (!messageId) return null
  const el = document.querySelector(`[data-message-id="${CSS.escape(messageId)}"]`)
  return el instanceof HTMLElement ? el : null
}

export function scrollToMessageIfMounted(messageId: string): boolean {
  const el = findMessageElement(messageId)
  if (!el) return false
  highlightMessageElement(el)
  return true
}
