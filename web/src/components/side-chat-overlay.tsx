import { useEffect, useState, type ReactNode } from 'react'

export function useNarrowViewport() {
  const [narrow, setNarrow] = useState(false)
  useEffect(() => {
    const media = window.matchMedia('(max-width: 767px)')
    const apply = () => setNarrow(media.matches)
    apply()
    media.addEventListener('change', apply)
    return () => media.removeEventListener('change', apply)
  }, [])
  return narrow
}

export function SideChatOverlay({
  onClose,
  children,
}: {
  onClose: () => void
  children: ReactNode
}) {
  const narrow = useNarrowViewport()
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])

  return (
    <div
      className={
        narrow
          ? 'fixed inset-0 z-40 flex flex-col bg-background'
          : 'fixed inset-y-0 right-0 z-40 flex w-[min(28rem,100vw)] flex-col border-l border-border bg-background shadow-lg'
      }
      role="dialog"
      aria-modal="false"
      aria-label="Side chat"
      data-side-chat-host="overlay"
      data-layout="overlay"
    >
      {children}
    </div>
  )
}
