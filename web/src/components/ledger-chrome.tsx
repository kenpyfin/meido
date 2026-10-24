import { Button } from '@/components/ui/button'
import { DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { cn } from '@/lib/utils'
import { useEffect, useRef, useState, type ReactNode } from 'react'
import { IconBrassBrooch } from './icons'

export type LedgerState = 'Ready' | 'Missing' | 'Restart needed' | 'No restart owed'

const SETTLED: ReadonlySet<LedgerState> = new Set(['Ready', 'No restart owed'])

export type LedgerIndexEntry = {
  id: string
  label: string
  state?: LedgerState
}

export function LedgerDialogHeader({
  eyebrow,
  title,
  description,
}: {
  eyebrow: string
  title: string
  description: string
}) {
  return (
    <header className="mc-ledger-header mc-katyusha-lace">
      <p className="mc-ledger-eyebrow">
        <IconBrassBrooch />
        {eyebrow}
      </p>
      <DialogTitle className="mc-display-title mc-ledger-dialog-title">{title}</DialogTitle>
      <DialogDescription className="mc-ledger-dialog-description">{description}</DialogDescription>
    </header>
  )
}

export function LedgerBook({
  label,
  entries,
  page,
  onPageChange,
  revealPage,
  open,
  mobile,
  children,
}: {
  label: string
  entries: LedgerIndexEntry[]
  page: string
  onPageChange: (id: string) => void
  revealPage: number
  open: boolean
  mobile: boolean
  children: ReactNode
}) {
  const [phonePage, setPhonePage] = useState(false)
  const seenReveal = useRef(revealPage)
  const active = entries.find((entry) => entry.id === page) ?? entries[0]

  useEffect(() => {
    if (!open) setPhonePage(false)
  }, [open])

  useEffect(() => {
    if (revealPage === seenReveal.current) return
    seenReveal.current = revealPage
    setPhonePage(true)
  }, [revealPage])

  const showPage = !mobile || phonePage

  function choose(id: string) {
    onPageChange(id)
    if (mobile) setPhonePage(true)
  }

  return (
    <div className={showPage && mobile ? 'mc-ledger-book mc-ledger-book--page' : 'mc-ledger-book'}>
      {showPage && mobile ? (
        <Button variant="outline" size="sm" className="self-start" type="button" onClick={() => setPhonePage(false)}>
          Back
        </Button>
      ) : (
        <nav className="mc-ledger-index" aria-label={label}>
          {entries.map((entry) => {
            const owed = entry.state != null && !SETTLED.has(entry.state)
            return (
              <button
                key={entry.id}
                type="button"
                className="mc-ledger-index-item"
                data-active={entry.id === active?.id ? 'true' : 'false'}
                data-page={entry.id}
                onClick={() => choose(entry.id)}
              >
                <span>{entry.label}</span>
                {owed ? <span className="mc-ledger-state mc-ledger-state--owed">{entry.state}</span> : null}
              </button>
            )
          })}
        </nav>
      )}
      {showPage ? (
        <div className="mc-ledger-page">
          <h2 className="mc-ledger-page-title">{active?.label}</h2>
          {children}
        </div>
      ) : null}
    </div>
  )
}

export type LedgerScope = 'live' | 'restart'

export function LedgerSection({
  title,
  owed = false,
  scope,
  dirty = false,
  children,
  className,
}: {
  title?: string
  owed?: boolean
  scope?: LedgerScope
  dirty?: boolean
  children: ReactNode
  className?: string
}) {
  const scopeLabel = scope === 'restart' ? 'Requires restart' : scope === 'live' ? 'Applies live' : null
  const scopeClass =
    scope === 'restart' && dirty ? 'mc-ledger-scope mc-ledger-scope--dirty' : 'mc-ledger-scope'
  return (
    <section className={cn('mc-ledger-section', className)} data-owed={owed ? 'true' : 'false'}>
      {title || scopeLabel ? (
        <div className="mc-ledger-section-head">
          {title ? <h3 className="mc-ledger-section-title">{title}</h3> : <span />}
          {scopeLabel ? <span className={scopeClass}>{scopeLabel}</span> : null}
        </div>
      ) : null}
      {children}
    </section>
  )
}

export function ServiceNote({ summary, children }: { summary: string; children: ReactNode }) {
  return (
    <div className="mc-service-note-block">
      <p className="mc-service-note-summary">{summary}</p>
      <details className="mc-service-note">
        <summary>Service note</summary>
        <p>{children}</p>
      </details>
    </div>
  )
}

export function LedgerStatusRow({
  label,
  state,
  detail,
}: {
  label: string
  state: LedgerState
  detail?: ReactNode
}) {
  const settled = SETTLED.has(state)
  return (
    <div className="mc-ledger-row">
      <span className="mc-ledger-row-label">{label}</span>
      <span className={settled ? 'mc-ledger-state mc-ledger-state--settled' : 'mc-ledger-state mc-ledger-state--owed'}>
        {state}
      </span>
      {detail ? <span className="mc-ledger-row-detail">{detail}</span> : null}
    </div>
  )
}

export function LedgerJump({
  children,
  onClick,
}: {
  children: ReactNode
  onClick: () => void
}) {
  return (
    <button type="button" className="mc-ledger-jump" onClick={onClick}>
      {children}
    </button>
  )
}
