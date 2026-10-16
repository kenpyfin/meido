import React from 'react'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { cn } from '@/lib/utils'

export type MobileSheetProps = {
  open: boolean
  onOpenChange: (open: boolean) => void
  title: string
  /** Optional short description for screen readers. */
  description?: string
  children: React.ReactNode
  /** Extra class on the sheet panel. */
  className?: string
}

/**
 * Mobile bottom sheet (focus trap, Escape, backdrop).
 * Desktop callers should keep their own anchored dropdown instead of this.
 */
export function MobileSheet({
  open,
  onOpenChange,
  title,
  description,
  children,
  className,
}: MobileSheetProps) {
  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent
        side="bottom"
        className={cn('mc-sheet max-h-[85dvh] gap-0 rounded-t-xl p-0', className)}
      >
        <div className="mc-sheet-handle" aria-hidden />
        <SheetHeader className="mc-sheet-header px-4 pt-2">
          <SheetTitle className="mc-sheet-title">{title}</SheetTitle>
          {description ? (
            <SheetDescription className="mc-sheet-description">{description}</SheetDescription>
          ) : (
            <SheetDescription className="sr-only">{title}</SheetDescription>
          )}
        </SheetHeader>
        <div className="mc-sheet-body px-4 pb-4">{children}</div>
      </SheetContent>
    </Sheet>
  )
}
