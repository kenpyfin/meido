import React from 'react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { IconMeidoCrest, IconTeacup } from './icons'

/** Empty thread welcome with keyboard shortcut hints. */
export function ThreadWelcomeHints({
  onShowShortcuts,
}: {
  onShowShortcuts?: () => void
}) {
  return (
    <div className="mc-thread-welcome-hints px-3 py-8 text-center">
      <div className="mc-crest-medallion" aria-hidden>
        <IconMeidoCrest className="size-8" />
      </div>
      <p className="mc-display-title text-2xl text-[color:var(--mc-text-primary)]">
        Okaerinasaimase — Welcome back.
      </p>
      <p className="mx-auto mt-2 max-w-md text-sm text-[color:var(--mc-text-muted)]">
        How may I attend to your tasks today? Send a request below, or drop files onto the tray to begin.
      </p>
      <div className="mt-3 flex flex-wrap items-center justify-center gap-2">
        <Badge variant="outline" className="font-normal text-[color:var(--mc-text-faint)]">
          <IconTeacup className="mr-1 size-3.5" />
          The tray accepts files
        </Badge>
      </div>
      <p className="mt-2 text-xs text-[color:var(--mc-text-faint)]">
        Press <kbd className="mc-shortcuts-kbd">/</kbd> to focus the composer
        {onShowShortcuts ? (
          <>
            {' '}
            · Press{' '}
            <Button
              type="button"
              variant="link"
              size="sm"
              className="mc-thread-welcome-shortcuts-link h-auto p-0 text-xs"
              onClick={onShowShortcuts}
            >
              <kbd className="mc-shortcuts-kbd">?</kbd>
            </Button>{' '}
            for shortcuts
          </>
        ) : (
          <>
            {' '}
            · Press <kbd className="mc-shortcuts-kbd">?</kbd> for shortcuts
          </>
        )}
      </p>
    </div>
  )
}
