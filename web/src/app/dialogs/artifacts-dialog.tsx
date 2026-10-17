import { useEffect, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogTitle, DialogDescription } from '@/components/ui/dialog'
import { api } from '@/api/client'
import { ensureArtifactSession } from '@/api/artifacts'
import type { ArtifactItem } from '@/types'
import { useIsMobile } from '@/lib/use-is-mobile'
import type { ArtifactsDialogProps } from './types'
import { formatBytes } from './shared'
import { MarkdownDocument, usePreviewText } from '@/components/markdown-document'

type Page = { artifacts: ArtifactItem[]; next_cursor: string | null; indexing: boolean; indexing_error?: string | null; trash_retention_days?: number }
function trashNote(item: ArtifactItem): string | null {
  if (item.status !== 'trash') return null
  if (item.pinned) return 'Kept while pinned'
  if (!item.expires_at) return null
  const when = new Date(item.expires_at)
  if (Number.isNaN(when.getTime())) return null
  return `Removed ${when.toLocaleString()}`
}
type Review = { artifact: ArtifactItem; managed_reclaimable_bytes: number; original_deletable: boolean; original_reclaimable_bytes: number }
const empty: Page = { artifacts: [], next_cursor: null, indexing: false }

export function ArtifactsDialog({ chatId, activePersonaId, artifacts }: ArtifactsDialogProps) {
  const [page, setPage] = useState<Page>(empty)
  const [q, setQ] = useState('')
  const [kind, setKind] = useState('all')
  const [status, setStatus] = useState('active')
  const [before, setBefore] = useState('')
  const [job, setJob] = useState('')
  const [cursors, setCursors] = useState<string[]>([''])
  const [selected, setSelected] = useState<ArtifactItem | null>(null)
  const [checked, setChecked] = useState<Set<string>>(new Set())
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [revision, setRevision] = useState(0)
  const [reviews, setReviews] = useState<Review[] | null>(null)
  const [deleteOriginal, setDeleteOriginal] = useState<Set<string>>(new Set())
  const [mutation, setMutation] = useState(false)
  const [mobileDetail, setMobileDetail] = useState(false)
  const mobile = useIsMobile()
  const cursor = cursors[cursors.length - 1]
  const refresh = () => setRevision(v => v + 1)
  useEffect(() => {
    setCursors(['']); setSelected(null); setChecked(new Set()); setPage(empty); setReviews(null); setMobileDetail(false)
  }, [chatId, activePersonaId, q, kind, status, before, job])
  useEffect(() => {
    if (!artifacts.open) setMobileDetail(false)
  }, [artifacts.open])
  useEffect(() => {
    if (!artifacts.open || chatId == null || activePersonaId == null) return
    const controller = new AbortController()
    const timer = setTimeout(async () => {
      setBusy(true); setError('')
      try {
        await ensureArtifactSession()
        const query = new URLSearchParams({ chat_id: String(chatId), persona_id: String(activePersonaId), q, kind, status, limit: '50' })
        if (cursor) query.set('cursor', cursor)
        if (before) query.set('before', `${before}T00:00:00Z`)
        if (job) query.set('job_id', job)
        const next = await api<Page>(`/api/artifacts?${query}`, { signal: controller.signal })
        if (controller.signal.aborted) return
        setPage(next); setChecked(old => new Set([...old].filter(id => next.artifacts.some(a => a.id === id))))
        setSelected(old => next.artifacts.find(a => a.id === old?.id) ?? next.artifacts[0] ?? null)
      } catch (e) { if (!controller.signal.aborted) setError(String(e)) }
      finally { if (!controller.signal.aborted) setBusy(false) }
    }, 200)
    return () => { clearTimeout(timer); controller.abort() }
  }, [artifacts.open, chatId, activePersonaId, q, kind, status, before, job, cursor, revision])
  useEffect(() => {
    if (!artifacts.open || reviews || mutation) return
    const timer = setInterval(refresh, page.indexing ? 2500 : 15000)
    return () => clearInterval(timer)
  }, [artifacts.open, page.indexing, reviews, mutation])
  const act = async (ids: string[], change: object) => {
    setMutation(true); setError('')
    try {
      for (const id of ids) await api(`/api/artifacts/${id}`, { method: 'PATCH', body: JSON.stringify(change) })
    } catch (e) { setError(String(e)) }
    finally { setMutation(false); refresh() }
  }
  const review = async () => {
    setMutation(true); setError('')
    try {
      const results: Review[] = []
      for (const id of checked) results.push(await api<Review>(`/api/artifacts/${id}/deletion-review`))
      setDeleteOriginal(new Set()); setReviews(results)
    } catch (e) { setError(String(e)) }
    finally { setMutation(false) }
  }
  const purge = async () => {
    if (!reviews) return
    setMutation(true); setError('')
    try {
      for (const r of reviews) await api(`/api/artifacts/${r.artifact.id}`, { method: 'DELETE', body: JSON.stringify({ confirmed: true, delete_original: deleteOriginal.has(r.artifact.id) }) })
      setReviews(null)
    } catch (e) { setError(String(e)); setReviews(null) }
    finally { setMutation(false); refresh() }
  }
  const toggle = (id: string) => setChecked(old => { const next = new Set(old); if (next.has(id)) next.delete(id); else next.add(id); return next })
  const openFile = (item: ArtifactItem) => {
    setSelected(item)
    if (mobile) setMobileDetail(true)
  }
  const phoneDetail = mobile && mobileDetail && selected != null
  const detail = selected ? (
    <>
      <h3 className="font-semibold break-all">{selected.name}</h3>
      <p className="text-sm text-muted-foreground break-all">{selected.source} · {selected.job_id ? `Job ${selected.job_id}` : 'No recorded job'} · {selected.status}{trashNote(selected) ? ` · ${trashNote(selected)}` : ''}</p>
      <div className="flex flex-wrap gap-2 my-3">
        {selected.status === 'active' && <>
          <Button size="sm" variant="secondary" onClick={() => window.open(selected.url, '_blank', 'noopener,noreferrer')}>Open</Button>
          <Button size="sm" variant="secondary" onClick={() => window.open(`${selected.url}?download=true`, '_blank', 'noopener,noreferrer')}>Download</Button>
          <Button size="sm" variant="secondary" onClick={() => void navigator.clipboard.writeText(new URL(selected.url, location.origin).href).catch(e => setError(String(e)))}>Copy link</Button>
        </>}
        <Button size="sm" variant="secondary" disabled={mutation} onClick={() => void act([selected.id], { pinned: !selected.pinned })}>{selected.pinned ? 'Unpin' : 'Pin'}</Button>
        {selected.message_id && <Button size="sm" variant="secondary" onClick={() => { artifacts.onOpenChange(false); artifacts.onOpenMessage(selected.message_id!) }}>Open originating message</Button>}
      </div>
      <ArtifactPreview key={`${selected.id}:${selected.status}`} item={selected} />
    </>
  ) : <p className="text-muted-foreground">Select a file to preview.</p>
  return <Dialog open={artifacts.open} onOpenChange={artifacts.onOpenChange}>
    <DialogContent className="w-[96vw] max-w-[1400px] h-[90vh] flex flex-col overflow-hidden">
      <DialogTitle>{phoneDetail ? (selected?.name ?? 'File') : 'Artifact library'}</DialogTitle>
      {phoneDetail ? (
        <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-auto">
          <Button variant="outline" size="sm" className="self-start" onClick={() => setMobileDetail(false)}>Back</Button>
          {detail}
        </div>
      ) : null}
      {phoneDetail ? null : <>
      <DialogDescription>Outputs and attachments for this persona. Each link preserves the exact published file.</DialogDescription>
      <div className="flex flex-wrap gap-2 items-center">
        <input aria-label="Search files" placeholder="Search filenames…" value={q} onChange={e => setQ(e.target.value)} className="rounded border bg-transparent px-3 py-2 flex-1 min-w-40" />
        <select aria-label="File type" value={kind} onChange={e => setKind(e.target.value)} className="rounded border bg-[var(--mc-bg-panel)] p-2">
          {['all','video','image','audio','pdf','markdown','html','text','other'].map(k => <option key={k} value={k}>{k === 'all' ? 'All types' : k}</option>)}
        </select>
        <select aria-label="Library view" value={status} onChange={e => setStatus(e.target.value)} className="rounded border bg-[var(--mc-bg-panel)] p-2"><option value="active">Active</option><option value="trash">Trash</option></select>
        <label className="text-sm">Before <input aria-label="Created before" type="date" value={before} onChange={e => setBefore(e.target.value)} className="rounded border bg-transparent p-2" /></label>
        <input aria-label="Job ID" placeholder="Filter by job ID" value={job} onChange={e => setJob(e.target.value)} className="rounded border bg-transparent p-2 w-44" />
        <Button variant="secondary" disabled={busy} onClick={refresh}>Refresh</Button>
      </div>
      {error && <p role="alert" className="text-destructive">{error}</p>}
      {page.indexing_error && <p role="alert" className="text-destructive text-sm">Some existing files could not be indexed: {page.indexing_error}. Refresh retries safely.</p>}
      {page.indexing && <p role="status" className="text-sm text-muted-foreground">Indexing existing files in the background. Results appear as they become available.</p>}
      {status === 'trash' && <p className="text-sm text-muted-foreground">Unpinned files stay in Trash for {page.trash_retention_days ?? 30} days, then the published copy is removed. Original files stay on disk unless you delete them in the review. Pinned files are kept.</p>}
      <div className="flex gap-3 min-h-0 flex-1 flex-col md:flex-row">
        <div className="flex min-h-0 flex-1 flex-col border rounded md:w-[45%] md:flex-none">
          <div className="flex flex-wrap gap-2 p-2 border-b items-center">
            <label className="text-sm"><input type="checkbox" aria-label="Select this page" checked={page.artifacts.length > 0 && checked.size === page.artifacts.length} onChange={e => setChecked(new Set(e.target.checked ? page.artifacts.map(a => a.id) : []))} /> Select page</label>
            <span className="text-sm">{checked.size} selected</span>
            <Button size="sm" variant="secondary" disabled={!checked.size || mutation} onClick={() => void act([...checked], { status: status === 'trash' ? 'active' : 'trash' })}>{status === 'trash' ? 'Restore' : 'Move to Trash'}</Button>
            {status === 'trash' && <Button size="sm" variant="destructive" disabled={!checked.size || mutation} onClick={() => void review()}>Review deletion</Button>}
            {status === 'active' && <Button size="sm" variant="secondary" title="Select explicitly superseded files, plus unpinned files older than the chosen date, for your review" onClick={() => setChecked(new Set(page.artifacts.filter(a => !a.pinned && (a.superseded_by || before)).map(a => a.id)))}>Select candidates</Button>}
          </div>
          <div className="overflow-auto flex-1" aria-busy={busy}>
            {!page.artifacts.length && <p className="p-4 text-muted-foreground">{busy ? 'Loading files…' : 'No matching files.'}</p>}
            {page.artifacts.map(a => <div key={a.id} className={`flex border-b items-start gap-2 p-2 ${selected?.id === a.id ? 'bg-accent' : ''}`}>
              <input type="checkbox" aria-label={`Select ${a.name}`} className="mt-2" checked={checked.has(a.id)} onChange={() => toggle(a.id)} />
              <button className="text-left min-w-0 flex-1" onClick={() => openFile(a)}>
                <span className="block truncate font-medium">{a.pinned ? '★ ' : ''}{a.name}</span>
                <span className="text-xs text-muted-foreground">{a.kind} · {formatBytes(a.size_bytes ?? null)} · {a.created_at ? new Date(a.created_at).toLocaleString() : 'Unknown date'}</span>
                {a.job_id && <span className="block truncate text-xs text-muted-foreground">Job {a.job_id}</span>}
                {a.availability === 'missing' && <span className="block text-xs text-destructive">File missing</span>}
                {a.superseded_by && <span className="block text-xs">Superseded</span>}
                {trashNote(a) && <span className="block text-xs">{trashNote(a)}</span>}
              </button>
            </div>)}
          </div>
          <div className="flex justify-between p-2 border-t items-center">
            <Button size="sm" variant="secondary" disabled={cursors.length === 1 || busy} onClick={() => setCursors(old => old.slice(0, -1))}>Previous</Button>
            <span className="text-sm">Page {cursors.length} · {page.artifacts.length} files</span>
            <Button size="sm" variant="secondary" disabled={!page.next_cursor || busy} onClick={() => setCursors(old => [...old, page.next_cursor!])}>Next</Button>
          </div>
        </div>
        {mobile ? null : (
          <section className="flex-1 min-w-0 min-h-0 overflow-auto border rounded p-3">
            {detail}
          </section>
        )}
      </div>
      </>}
      {!phoneDetail && reviews && <div className="border rounded p-3 max-h-[40vh] overflow-auto bg-[var(--mc-bg-panel)]" role="region" aria-label="Permanent deletion review">
        <h3 className="font-semibold">Permanently delete {reviews.length} files?</h3>
        <p className="text-sm">Managed storage reclaimable: {formatBytes(reviews.reduce((sum, r) => sum + r.managed_reclaimable_bytes, 0))}. Shared data is retained. Old links will report deletion.</p>
        {reviews.map(r => <div key={r.artifact.id} className="py-2 text-sm break-all">
          <p>{r.artifact.name}</p>
          {r.original_deletable ? <label><input type="checkbox" checked={deleteOriginal.has(r.artifact.id)} onChange={e => setDeleteOriginal(old => { const next = new Set(old); if (e.target.checked) next.add(r.artifact.id); else next.delete(r.artifact.id); return next })} /> Also delete original ({formatBytes(r.original_reclaimable_bytes)}): {r.artifact.source_path}</label> : <p className="text-muted-foreground">Original retained.</p>}
        </div>)}
        <div className="flex gap-2"><Button variant="destructive" disabled={mutation} onClick={() => void purge()}>Confirm permanent deletion</Button><Button variant="secondary" disabled={mutation} onClick={() => setReviews(null)}>Cancel</Button></div>
      </div>}
    </DialogContent>
  </Dialog>
}

function ArtifactPreview({ item }: { item: ArtifactItem }) {
  const textKind = item.status === 'active' && (item.kind === 'text' || item.kind === 'markdown')
  const preview = usePreviewText(textKind ? item.preview_url : null)
  const [mediaError, setMediaError] = useState('')
  const error = preview.error || mediaError
  if (item.availability === 'missing' && item.status === 'active') return <p>Published bytes are missing. Restore from backup or publish the source again.</p>
  if (item.status !== 'active') return <p>Restore this file from Trash to preview it.</p>
  if (error) return <p role="alert" className="text-destructive">{error}</p>
  const fail = () => setMediaError('Preview unavailable. The file may be missing or this browser may not support its format. Try Open or Download.')
  if (item.kind === 'video') return <video controls preload="metadata" src={item.url} onError={fail} className="max-h-[60vh] w-full" />
  if (item.kind === 'audio') return <audio controls preload="metadata" src={item.url} onError={fail} className="w-full" />
  if (item.kind === 'image') return <img src={item.preview_url} alt={item.name} onError={fail} className="max-h-[60vh] w-full object-contain" />
  if (['html', 'pdf'].includes(item.kind)) return <iframe title={item.name} sandbox="" src={item.preview_url} className="h-[60vh] w-full" />
  if (item.kind === 'markdown' || item.kind === 'text') {
    const note = <p className="text-xs text-muted-foreground">Preview limited to 256 KiB. Download for the complete file.</p>
    if (preview.loading) return <>{note}<p className="text-sm text-muted-foreground">Loading preview…</p></>
    if (item.kind === 'markdown') return <>{note}<MarkdownDocument text={preview.text} /></>
    return <>{note}<pre className="whitespace-pre-wrap break-words text-sm">{preview.text}</pre></>
  }
  return <p>No preview for this format. Use Open or Download.</p>
}
