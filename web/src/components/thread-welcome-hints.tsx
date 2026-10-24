import React from 'react'
import { Button } from '@/components/ui/button'
import { IconMeidoCrest } from './icons'

/** Empty thread welcome with keyboard shortcut hints. */
export function ThreadWelcomeHints({
  onShowShortcuts,
}: {
  onShowShortcuts?: () => void
}) {
  return (
    <div className="mc-thread-welcome-hints px-3 py-8 text-center">
      <IconMeidoCrest className="mc-crest-medallion" />
      <p className="mc-display-title text-2xl text-[color:var(--mc-text-primary)]">
        Okaerinasaimase — Welcome back.
      </p>
      <p className="mx-auto mt-2 max-w-md text-sm text-[color:var(--mc-text-muted)]">
        Send a request below, or drop files onto the tray.
      </p>
      <p className="mt-3 text-xs text-[color:var(--mc-text-faint)]">
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
