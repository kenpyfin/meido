import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { MarkdownDocument, usePreviewText } from '@/components/markdown-document'

const PREVIEW_NOTE = 'Preview limited to 256 KiB. Download for the complete file.'

/** Fields the reader needs. A full artifact record satisfies this. */
export type ReadableFile = {
  name: string
  url: string
  preview_url: string
}

/** In-app reader for a published markdown artifact. Loaded only after the card is opened. */
export function MarkdownFileDialog({
  item,
  open,
  onOpenChange,
}: {
  item: ReadableFile
  open: boolean
  onOpenChange: (open: boolean) => void
}) {
  const { text, error, loading } = usePreviewText(open ? item.preview_url : null)
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-[min(48rem,calc(100%-2rem))]">
        <DialogTitle className="break-all pr-8">{item.name}</DialogTitle>
        <DialogDescription>{PREVIEW_NOTE}</DialogDescription>
        <div className="flex flex-wrap gap-2">
          <Button
            size="sm"
            variant="secondary"
            onClick={() => window.open(item.url, '_blank', 'noopener,noreferrer')}
          >
            Open raw
          </Button>
          <Button
            size="sm"
            variant="secondary"
            onClick={() => window.open(`${item.url}?download=true`, '_blank', 'noopener,noreferrer')}
          >
            Download
          </Button>
        </div>
        {error ? (
          <p role="alert" className="text-destructive">{error}</p>
        ) : loading ? (
          <p className="text-muted-foreground">Loading preview…</p>
        ) : (
          <MarkdownDocument text={text} />
        )}
      </DialogContent>
    </Dialog>
  )
}
