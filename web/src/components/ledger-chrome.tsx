import { DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { cn } from '@/lib/utils'
import type { ReactNode } from 'react'
import { IconBrassBrooch } from './icons'

export type LedgerState = 'Ready' | 'Missing' | 'Restart needed' | 'No restart owed'

const SETTLED: ReadonlySet<LedgerState> = new Set(['Ready', 'No restart owed'])

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

export function LedgerSection({
  title,
  owed = false,
  children,
  className,
}: {
  title?: string
  owed?: boolean
  children: ReactNode
  className?: string
}) {
  return (
    <section className={cn('mc-ledger-section', className)} data-owed={owed ? 'true' : 'false'}>
      {title ? <h3 className="mc-ledger-section-title">{title}</h3> : null}
      {children}
    </section>
  )
}

export function LedgerStatusRow({
  label,
  state,
  detail,
}: {
  label: string
  state: LedgerState
  detail?: string
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
