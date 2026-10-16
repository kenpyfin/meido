import React from 'react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'

/** Empty thread welcome with keyboard shortcut hints. */
export function ThreadWelcomeHints({
  onShowShortcuts,
}: {
  onShowShortcuts?: () => void
}) {
  return (
    <div className="mc-thread-welcome-hints px-3 py-8 text-center">
      <p className="text-sm text-[color:var(--mc-text-muted)]">
        Send a message to start. Attach files by dragging them into the composer.
      </p>
      <div className="mt-3 flex flex-wrap items-center justify-center gap-2">
        <Badge variant="outline" className="font-normal text-[color:var(--mc-text-faint)]">
          Tip: drag files into the composer
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
