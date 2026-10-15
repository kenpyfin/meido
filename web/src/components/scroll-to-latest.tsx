import React, { useCallback } from 'react'
import { ThreadPrimitive } from '@assistant-ui/react'
import { IconChevronDown } from './icons'

/** Jump to the latest messages when the thread is scrolled up. */
export function ScrollToLatest() {
  const suppressKeyboard = useCallback((e: React.PointerEvent | React.MouseEvent) => {
    // Do not let the tap focus this control or transfer focus into the composer.
    e.preventDefault()
    e.stopPropagation()
    const active = document.activeElement
    if (
      active instanceof HTMLElement &&
      (active.tagName === 'TEXTAREA' || active.tagName === 'INPUT' || active.isContentEditable)
    ) {
      active.blur()
    }
  }, [])

  return (
    <ThreadPrimitive.ScrollToBottom asChild>
      <button
        type="button"
        className="mc-scroll-to-latest-btn cursor-pointer"
        aria-label="Jump to latest messages"
        title="Jump to latest messages"
        tabIndex={-1}
        onPointerDown={suppressKeyboard}
        onMouseDown={suppressKeyboard}
        onClick={(e) => {
          // Click still runs (ScrollToBottom handler); keep focus out of the composer.
          e.stopPropagation()
          const active = document.activeElement
          if (
            active instanceof HTMLElement &&
            (active.tagName === 'TEXTAREA' || active.tagName === 'INPUT' || active.isContentEditable)
          ) {
            active.blur()
          }
        }}
      >
        <IconChevronDown className="size-4 shrink-0" />
      </button>
    </ThreadPrimitive.ScrollToBottom>
  )
}
