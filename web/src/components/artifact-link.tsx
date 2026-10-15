import { lazy, Suspense, useEffect, useState } from 'react'
import { api } from '@/api/client'
import { ensureArtifactSession } from '@/api/artifacts'
import type { ArtifactItem } from '@/types'

const MarkdownFileDialog = lazy(() =>
  import('./markdown-file-dialog').then((m) => ({ default: m.MarkdownFileDialog })),
)

/** Metadata and availability come from the server, not the assistant's label. */
export function ArtifactLink({ id }: { id: string }) {
  const [item, setItem] = useState<ArtifactItem | null>(null)
  const [error, setError] = useState('')
  const [readerOpen, setReaderOpen] = useState(false)
  useEffect(() => {
    const controller = new AbortController()
    ensureArtifactSession().then(() => api<ArtifactItem>(`/api/artifacts/${id}`, { signal: controller.signal })).then(a => {
      if (!controller.signal.aborted) setItem(a)
    }).catch(e => { if (!controller.signal.aborted) setError(String(e)) })
    return () => controller.abort()
  }, [id])
  if (error) return <span role="status">File unavailable: {error}</span>
  if (!item) return <span>Loading file…</span>
  if (item.availability === 'missing' && item.status === 'active') return <span>{item.name} — file missing</span>
  if (item.status !== 'active') return <span>{item.name} — {item.status === 'trash' ? 'in Trash; restore in the artifact library' : 'permanently deleted'}</span>
  const label = (
    <>
      <span className="font-medium break-all">{item.name}</span>
      <span className="text-xs text-muted-foreground">{item.kind} · {Math.ceil((item.size_bytes ?? 0) / 1024)} KiB{item.job_id ? ` · Job ${item.job_id.slice(0, 8)}` : ''}</span>
    </>
  )
  if (item.kind === 'markdown') {
    return (
      <>
        <button
          type="button"
          className="inline-flex flex-col border rounded px-3 py-2 my-1 max-w-full bg-transparent text-left"
          onClick={() => setReaderOpen(true)}
        >
          {label}
        </button>
        {readerOpen ? (
          <Suspense fallback={null}>
            <MarkdownFileDialog item={item} open={readerOpen} onOpenChange={setReaderOpen} />
          </Suspense>
        ) : null}
      </>
    )
  }
  return <a href={item.url} target="_blank" rel="noopener noreferrer" className="inline-flex flex-col border rounded px-3 py-2 my-1 no-underline max-w-full">
    {label}
  </a>
}
export function artifactId(url: string | undefined): string | null {
  return url?.match(/^\/api\/artifacts\/([0-9a-f-]{36})\/content(?:\?|$)/)?.[1] ?? null
}
