import { useState } from 'react'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'

/** In-app viewer for a published image. The caller starts the media session first. */
export function ImageFileDialog({
  name,
  url,
  open,
  onOpenChange,
}: {
  name: string
  url: string
  open: boolean
  onOpenChange: (open: boolean) => void
}) {
  const [failed, setFailed] = useState(false)
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-[min(64rem,calc(100%-2rem))]">
        <DialogTitle className="break-all pr-8">{name}</DialogTitle>
        <DialogDescription>Image</DialogDescription>
        {failed ? (
          <p role="alert" className="text-destructive">
            This image could not be shown. It may be missing, or this browser may not support its format.
          </p>
        ) : (
          <img
            src={url}
            alt={name}
            className="max-h-[70vh] w-full object-contain"
            onError={() => setFailed(true)}
          />
        )}
      </DialogContent>
    </Dialog>
  )
}
