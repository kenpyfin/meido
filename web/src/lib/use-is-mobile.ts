import { useEffect, useState } from 'react'

const QUERY = '(max-width: 767px)'

function mobileNow(): boolean {
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return false
  return window.matchMedia(QUERY).matches
}

/** Phone width. Matches the cockpit breakpoint in `web/src/styles.css`. */
export function useIsMobile(): boolean {
  const [mobile, setMobile] = useState(mobileNow)
  useEffect(() => {
    if (typeof window.matchMedia !== 'function') return
    const mq = window.matchMedia(QUERY)
    const sync = () => setMobile(mq.matches)
    sync()
    mq.addEventListener('change', sync)
    return () => mq.removeEventListener('change', sync)
  }, [])
  return mobile
}
