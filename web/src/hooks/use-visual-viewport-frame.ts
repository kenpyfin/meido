import { useEffect } from 'react'

const NARROW_SCREEN = '(max-width: 767px)'

/**
 * Publish the visible viewport size so the app shell can shrink with the
 * keyboard instead of letting the browser zoom the page to reveal the composer.
 */
export function useVisualViewportFrame(): void {
  useEffect(() => {
    const root = document.documentElement
    const viewport = window.visualViewport
    const narrow = window.matchMedia(NARROW_SCREEN)

    const apply = () => {
      if (!narrow.matches || viewport == null) {
        root.style.removeProperty('--mc-visual-viewport-height')
        root.style.removeProperty('--mc-vv-offset-top')
        // iPad and other non-phone widths keep the desktop shell. A leftover
        // window scroll from the phone pin would otherwise open the page at the bottom.
        if (window.scrollX !== 0 || window.scrollY !== 0) {
          window.scrollTo(0, 0)
        }
        return
      }
      root.style.setProperty('--mc-visual-viewport-height', `${viewport.height}px`)
      root.style.setProperty('--mc-vv-offset-top', `${viewport.offsetTop}px`)
      if ((viewport.offsetTop > 0 || viewport.offsetLeft > 0) && (window.scrollX !== 0 || window.scrollY !== 0)) {
        window.scrollTo(0, 0)
      }
    }

    apply()
    narrow.addEventListener('change', apply)
    viewport?.addEventListener('resize', apply)
    viewport?.addEventListener('scroll', apply)
    window.addEventListener('orientationchange', apply)
    return () => {
      narrow.removeEventListener('change', apply)
      viewport?.removeEventListener('resize', apply)
      viewport?.removeEventListener('scroll', apply)
      window.removeEventListener('orientationchange', apply)
      root.style.removeProperty('--mc-visual-viewport-height')
      root.style.removeProperty('--mc-vv-offset-top')
    }
  }, [])
}
