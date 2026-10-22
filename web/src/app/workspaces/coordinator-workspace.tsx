import { Button } from '@/components/ui/button'
import { MarkdownStream } from '@/components/markdown-stream'
import { useCallback, useEffect, useState } from 'react'
import { api } from '../../api/client'
import type { Persona } from '../../types'
import { BoardView } from './board-view'
import { parseDigest, type DigestItem } from './today-digest'

type Overview = {
  digest?: string
  brief?: string | null
  brief_at?: string | null
}

type Props = {
  chatId: number | null
  personas: Persona[]
  onOpenPersona: (personaId: number, sessionId: string | null) => void
  focusTodoId?: number | null
  onFocusTodoHandled?: () => void
}

function formatWhen(iso: string | null | undefined): string {
  if (!iso) return ''
  const date = new Date(iso)
  if (Number.isNaN(date.getTime())) return iso
  return date.toLocaleString(undefined, {
    month: 'short',
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit',
  })
}

function personaLabel(personas: Persona[], id: string): string {
  const match = personas.find((persona) => String(persona.id) === id)
  return match?.name ?? `Persona ${id}`
}

function Pill({ children }: { children: string }) {
  return (
    <span className="rounded bg-[color:var(--mc-bg-sidebar)] px-1.5 py-0.5 text-[10px] font-medium uppercase tracking-wide text-[color:var(--mc-text-muted)]">
      {children}
    </span>
  )
}

function Meta({ children }: { children: string }) {
  return <span className="text-xs text-[color:var(--mc-text-muted)]">{children}</span>
}

function DigestLine({ item, personas }: { item: DigestItem; personas: Persona[] }) {
  if (item.kind === 'card') {
    const due = formatWhen(item.due)
    return (
      <div>
        <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
          <span className="font-medium">#{item.id}</span>
          <Pill>{item.status}</Pill>
          <Meta>{personaLabel(personas, item.owner)}</Meta>
          {due ? <Meta>{`Due ${due}`}</Meta> : null}
        </div>
        {item.title ? <p className="mt-0.5">{item.title}</p> : null}
      </div>
    )
  }
  if (item.kind === 'change') {
    return (
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
        <span className="font-medium">#{item.id}</span>
        <span>
          {item.from} → {item.to}
        </span>
        <Meta>{item.actor}</Meta>
      </div>
    )
  }
  if (item.kind === 'run') {
    return (
      <div>
        <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
          <span className="font-medium">Run #{item.id}</span>
          <Pill>{item.status}</Pill>
          <Meta>{`Task #${item.taskId}`}</Meta>
        </div>
        {item.preview ? <p className="mt-0.5">{item.preview}</p> : null}
      </div>
    )
  }
  if (item.kind === 'schedule') {
    return (
      <div>
        <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
          <span className="font-medium">#{item.id}</span>
          <Meta>{personaLabel(personas, item.persona)}</Meta>
          <Meta>{formatWhen(item.next) || item.next}</Meta>
        </div>
        {item.prompt ? <p className="mt-0.5">{item.prompt}</p> : null}
      </div>
    )
  }
  if (item.kind === 'topic') {
    return (
      <div>
        <div className="flex flex-wrap items-baseline gap-x-2 gap-y-1">
          <span className="font-medium">{item.name}</span>
          {item.title ? <span>{item.title}</span> : null}
        </div>
        {item.summary ? <p className="mt-0.5 text-[color:var(--mc-text-muted)]">{item.summary}</p> : null}
      </div>
    )
  }
  if (item.text.includes('\n')) {
    return <pre className="overflow-auto whitespace-pre text-xs leading-relaxed">{item.text}</pre>
  }
  return <p className="whitespace-pre-wrap">{item.text}</p>
}

function DigestView({ text, personas }: { text: string; personas: Persona[] }) {
  const sections = parseDigest(text)
  if (sections.length === 0) {
    return <p className="whitespace-pre-wrap text-sm leading-relaxed">{text}</p>
  }
  return (
    <div className="flex flex-col gap-4">
      {sections.map((section) => (
        <section key={section.title}>
          <h3 className="mb-1.5 text-xs font-semibold uppercase tracking-wide text-[color:var(--mc-text-muted)]">
            {section.title}
          </h3>
          {section.body ? (
            <p className="mb-1.5 whitespace-pre-wrap text-sm leading-relaxed">{section.body}</p>
          ) : null}
          {section.items.length > 0 ? (
            <ul className="flex flex-col gap-1.5">
              {section.items.map((item, index) => (
                <li
                  key={`${section.title}-${index}`}
                  className="rounded-md border border-[color:var(--mc-border-soft)] bg-[color:var(--mc-bg-panel)] px-2.5 py-1.5 text-sm leading-relaxed"
                >
                  <DigestLine item={item} personas={personas} />
                </li>
              ))}
            </ul>
          ) : section.body ? null : (
            <p className="text-sm text-[color:var(--mc-text-muted)]">None</p>
          )}
        </section>
      ))}
    </div>
  )
}

export function CoordinatorWorkspace({
  chatId,
  personas,
  onOpenPersona,
  focusTodoId = null,
  onFocusTodoHandled,
}: Props) {
  const [overview, setOverview] = useState<Overview | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')

  const load = useCallback(async () => {
    if (chatId == null) return
    setLoading(true)
    try {
      const data = await api<Overview>('/api/coordinator/digest')
      setOverview(data)
      setError('')
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Could not load today\u2019s overview')
    } finally {
      setLoading(false)
    }
  }, [chatId])

  useEffect(() => {
    void load()
  }, [load])

  const brief = overview?.brief?.trim() ?? ''
  const digest = overview?.digest?.trim() ?? ''
  const when = formatWhen(overview?.brief_at)

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-hidden p-3">
      <section className="flex h-[50vh] shrink-0 flex-col overflow-hidden rounded-lg border border-[color:var(--mc-border-soft)]">
        <div className="flex flex-wrap items-center justify-between gap-2 border-b border-[color:var(--mc-border-soft)] px-3 py-2">
          <h2 className="text-sm font-medium">Today</h2>
          <Button size="sm" variant="secondary" type="button" onClick={() => void load()}>
            {loading ? 'Refreshing\u2026' : 'Refresh'}
          </Button>
        </div>
        <div className="min-h-0 flex-1 overflow-auto p-3">
          {error ? (
            <p className="mb-2 text-sm text-red-600" role="alert">
              {error}
            </p>
          ) : null}
          {brief ? (
            <section className="mb-4">
              <h3 className="mb-1.5 text-xs font-semibold uppercase tracking-wide text-[color:var(--mc-text-muted)]">
                {when ? `Brief \u00b7 ${when}` : 'Brief'}
              </h3>
              <MarkdownStream className="[&_h1]:text-base [&_h2]:text-sm [&_h3]:text-sm [&_li]:my-0.5 [&_ol]:my-1 [&_p]:my-1 [&_ul]:my-1">
                {brief}
              </MarkdownStream>
            </section>
          ) : null}
          {digest ? (
            <DigestView text={digest} personas={personas} />
          ) : (
            <p className="text-sm text-[color:var(--mc-text-muted)]">
              {loading ? 'Loading\u2026' : 'No activity in the last day.'}
            </p>
          )}
        </div>
      </section>
      <BoardView
        chatId={chatId}
        personas={personas}
        onOpenPersona={onOpenPersona}
        focusTodoId={focusTodoId}
        onFocusTodoHandled={onFocusTodoHandled}
      />
    </div>
  )
}
