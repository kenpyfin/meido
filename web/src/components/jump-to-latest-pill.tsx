import React from 'react'

export type JumpToLatestPillProps = {
  newMessagesPending?: boolean
  onJump: () => void | Promise<void>
}

/** Floating control to leave an anchored history window and return to the live tip. */
export function JumpToLatestPill({
  newMessagesPending = false,
  onJump,
}: JumpToLatestPillProps) {
  return (
    <div className="mc-jump-to-latest" role="region" aria-label="Jump to latest">
      <button
        type="button"
        className="mc-jump-to-latest-btn cursor-pointer"
        onClick={() => {
          void onJump()
        }}
      >
        {newMessagesPending ? 'New messages — jump to latest' : 'Jump to latest'}
      </button>
    </div>
  )
}
