import React from 'react'
import { Button } from '@/components/ui/button'

export type EmptyStateProps = {
  title: string
  description?: string
  actionLabel?: string
  onAction?: () => void
  className?: string
}

export function EmptyState({
  title,
  description,
  actionLabel,
  onAction,
  className,
}: EmptyStateProps) {
  return (
    <div className={`mc-empty-state ${className ?? ''}`.trim()}>
      <span className="block text-sm font-medium text-[color:var(--mc-text-primary)]">{title}</span>
      {description ? (
        <span className="mt-1 block text-xs text-muted-foreground">{description}</span>
      ) : null}
      {actionLabel && onAction ? (
        <Button variant="secondary" size="sm" className="mt-3 cursor-pointer" onClick={onAction}>
          {actionLabel}
        </Button>
      ) : null}
    </div>
  )
}
