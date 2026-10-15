import React from 'react'
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'

export type ConfirmDialogProps = {
  open: boolean
  onOpenChange: (open: boolean) => void
  title: string
  description: string
  confirmLabel?: string
  cancelLabel?: string
  destructive?: boolean
  loading?: boolean
  onConfirm: () => void | Promise<void>
}

export function ConfirmDialog({
  open,
  onOpenChange,
  title,
  description,
  confirmLabel = 'Confirm',
  cancelLabel = 'Cancel',
  destructive = false,
  loading = false,
  onConfirm,
}: ConfirmDialogProps) {
  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent className="max-w-[420px]">
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription>{description}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel disabled={loading}>{cancelLabel}</AlertDialogCancel>
          <Button
            variant={destructive ? 'destructive' : 'default'}
            disabled={loading}
            onClick={(e) => {
              e.preventDefault()
              void onConfirm()
            }}
          >
            {loading ? 'Working…' : confirmLabel}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}

export type PendingConfirm = {
  title: string
  description: string
  confirmLabel?: string
  destructive?: boolean
  onConfirm: () => void | Promise<void>
}

export function useConfirmDialog() {
  const [pending, setPending] = React.useState<PendingConfirm | null>(null)
  const [loading, setLoading] = React.useState(false)

  const requestConfirm = React.useCallback((opts: PendingConfirm) => {
    setPending(opts)
  }, [])

  const close = React.useCallback(() => {
    if (loading) return
    setPending(null)
  }, [loading])

  const handleConfirm = React.useCallback(async () => {
    if (!pending) return
    setLoading(true)
    try {
      await pending.onConfirm()
      setPending(null)
    } finally {
      setLoading(false)
    }
  }, [pending])

  const dialog = (
    <ConfirmDialog
      open={pending != null}
      onOpenChange={(open) => {
        if (!open) close()
      }}
      title={pending?.title ?? ''}
      description={pending?.description ?? ''}
      confirmLabel={pending?.confirmLabel}
      destructive={pending?.destructive}
      loading={loading}
      onConfirm={handleConfirm}
    />
  )

  return { requestConfirm, confirmDialog: dialog }
}
