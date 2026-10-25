import { lazy, Suspense, useEffect, useRef, useState } from 'react'
import { ensureArtifactSession } from '../api/artifacts'
import { formatMessageTimestamp } from '../lib/format-message-time'
import { formatTopicWhen } from '../lib/conversation-topics'
import { dossierMarkdown, dossierPlainText, type DossierArtifact, type TopicDossier } from '../lib/topic-focus'
import type { BackendMessage, PersonaMessageBookmark } from '../types'
import { ImageFileDialog } from './image-file-dialog'
import { MarkdownStream } from './markdown-stream'
import { Button } from './ui/button'

const MarkdownFileDialog = lazy(() =>
  import('./markdown-file-dialog').then((m) => ({ default: m.MarkdownFileDialog })),
)

export type TopicFocusViewProps = {
  topicTitle: string
  assistantLabel: string
  loading: boolean
  error: string
  segmenting: boolean
  dossier: TopicDossier | null
  pageKey: string
  highlightMessageId: string | null
  paging: boolean
  onSelectSideChat: (sideChatId: string) => void
  onRetry: () => void
  onEarlier: () => void
  onLater: () => void
  onOpenAround: (messageId: string) => void
  onHighlight: (messageId: string) => void
}

export function TopicFocusView({
  topicTitle,
  assistantLabel,
  loading,
  error,
  segmenting,
  dossier,
  pageKey,
  highlightMessageId,
  paging,
  onSelectSideChat,
  onRetry,
  onEarlier,
  onLater,
  onOpenAround,
  onHighlight,
}: TopicFocusViewProps) {
  const scrollerRef = useRef<HTMLDivElement | null>(null)
  const topic = dossier?.topic
  const messages = dossier?.messages ?? []
  const summary = topic?.summary?.trim() ?? ''
  const updated = topic?.updated_at ? formatTopicWhen(topic.updated_at) : ''

  useEffect(() => {
    const root = scrollerRef.current
    if (!root) return
    if (highlightMessageId && messageSelector(highlightMessageId)) {
      const node = root.querySelector(messageSelector(highlightMessageId) as string)
      if (node instanceof HTMLElement) {
        const rootRect = root.getBoundingClientRect()
        const nodeRect = node.getBoundingClientRect()
        const target =
          root.scrollTop + (nodeRect.top - rootRect.top) - root.clientHeight / 2 + nodeRect.height / 2
        root.scrollTop = Math.max(0, target)
        return
      }
    }
    root.scrollTop = 0
  }, [highlightMessageId, pageKey, messages.length])

  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden" data-thread-surface="focus">
      <div ref={scrollerRef} className="min-h-0 flex-1 overflow-y-auto px-3 py-3 md:px-4">
        <header className="border-b border-border pb-3">
          <p className="text-xs text-muted-foreground">Topic</p>
          <h2 className="truncate text-base font-medium">{topic?.title || topicTitle || 'Topic'}</h2>
          <p className="text-xs text-muted-foreground">
            {topic ? `${topic.message_count} messages` : 'One topic at a time'}
            {topic?.status === 'open' ? ' · still open' : ''}
            {segmenting ? ' · updating titles' : ''}
          </p>
        </header>

        {loading ? <p className="py-6 text-sm text-muted-foreground">Loading this topic…</p> : null}
        {!loading && error ? (
          <div className="py-6" role="alert">
            <p className="text-sm">{error}</p>
            <Button type="button" size="sm" className="mt-3 cursor-pointer" onClick={onRetry}>
              Try again
            </Button>
          </div>
        ) : null}

        {!loading && !error && !topic ? (
          <p className="py-6 text-sm text-muted-foreground">
            {segmenting ? 'Topics are still being prepared.' : 'No topic is ready for this conversation yet.'}
          </p>
        ) : null}

        {topic ? (
          <div className="flex flex-col gap-4 py-4">
            <section aria-labelledby="dossier-summary">
              <div className="flex items-center gap-2">
                <h3 id="dossier-summary" className="text-sm font-medium">
                  Generated summary
                </h3>
                {updated ? <span className="text-xs text-muted-foreground">Updated {updated}</span> : null}
              </div>
              <p className="mt-1 text-sm text-muted-foreground">{summary || 'No summary yet.'}</p>
            </section>

            <BookmarkList
              bookmarks={dossier?.bookmarks ?? []}
              messages={messages}
              onOpenAround={onOpenAround}
              onHighlight={onHighlight}
            />
            <BranchList sideChats={topic.side_chats} onSelectSideChat={onSelectSideChat} />
            <MessageList
              messages={messages}
              assistantLabel={assistantLabel}
              hasOlder={dossier?.has_older ?? false}
              hasNewer={dossier?.has_newer ?? false}
              paging={paging}
              highlightMessageId={highlightMessageId}
              onEarlier={onEarlier}
              onLater={onLater}
            />
            <OutputStrip artifacts={dossier?.artifacts ?? []} />
          </div>
        ) : null}
      </div>
    </div>
  )
}

function messageSelector(id: string): string | null {
  if (!/^[A-Za-z0-9_-]+$/.test(id)) return null
  return `[data-message-id="${id}"]`
}

function pageLabel(hasOlder: boolean, hasNewer: boolean): string {
  if (hasOlder && hasNewer) return 'Middle of this topic'
  if (hasOlder) return 'Latest turns'
  if (hasNewer) return 'Earliest turns'
  return 'All turns'
}

function BookmarkList({
  bookmarks,
  messages,
  onOpenAround,
  onHighlight,
}: {
  bookmarks: PersonaMessageBookmark[]
  messages: BackendMessage[]
  onOpenAround: (messageId: string) => void
  onHighlight: (messageId: string) => void
}) {
  if (bookmarks.length === 0) return null
  return (
    <section aria-labelledby="dossier-bookmarks">
      <h3 id="dossier-bookmarks" className="text-sm font-medium">
        Bookmarks
      </h3>
      <ul className="mt-1 flex flex-col gap-1">
        {bookmarks.map((bookmark) => (
          <li key={bookmark.message_id}>
            <button
              type="button"
              className="w-full cursor-pointer rounded-md px-2 py-1 text-left text-sm hover:bg-muted focus-visible:outline focus-visible:outline-2 focus-visible:outline-ring"
              onClick={() => {
                const onPage = messages.some((message) => message.id === bookmark.message_id)
                if (onPage) onHighlight(bookmark.message_id)
                else onOpenAround(bookmark.message_id)
              }}
            >
              {dossierPlainText(bookmark.content_preview || '') || 'Bookmarked message'}
            </button>
          </li>
        ))}
      </ul>
    </section>
  )
}

function BranchList({
  sideChats,
  onSelectSideChat,
}: {
  sideChats: TopicDossier['topic']['side_chats']
  onSelectSideChat: (sideChatId: string) => void
}) {
  if (sideChats.length === 0) return null
  return (
    <section aria-labelledby="dossier-branches">
      <h3 id="dossier-branches" className="text-sm font-medium">
        Branches
      </h3>
      <ul className="mt-1 flex flex-wrap gap-2">
        {sideChats.map((side) => (
          <li key={side.id}>
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="cursor-pointer"
              onClick={() => onSelectSideChat(side.id)}
            >
              {side.title.trim() || 'Side chat'}
              {side.turn_count > 0 ? ` · ${side.turn_count}` : ''}
            </Button>
          </li>
        ))}
      </ul>
    </section>
  )
}

function MessageList({
  messages,
  assistantLabel,
  hasOlder,
  hasNewer,
  paging,
  highlightMessageId,
  onEarlier,
  onLater,
}: {
  messages: BackendMessage[]
  assistantLabel: string
  hasOlder: boolean
  hasNewer: boolean
  paging: boolean
  highlightMessageId: string | null
  onEarlier: () => void
  onLater: () => void
}) {
  const label = pageLabel(hasOlder, hasNewer)
  return (
    <section aria-labelledby="dossier-messages">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 id="dossier-messages" className="text-sm font-medium">
          Turns
        </h3>
        <p className="text-xs text-muted-foreground">{label}</p>
      </div>
      {hasOlder || hasNewer ? (
        <div className="mt-2 flex gap-2">
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="cursor-pointer"
            disabled={!hasOlder || paging}
            onClick={onEarlier}
          >
            Earlier
          </Button>
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="cursor-pointer"
            disabled={!hasNewer || paging}
            onClick={onLater}
          >
            Later
          </Button>
        </div>
      ) : null}
      {messages.length === 0 ? (
        <p className="mt-1 text-sm text-muted-foreground">No messages in this topic yet.</p>
      ) : (
        <ol className="mt-3 flex flex-col gap-3" data-focus-message-count={messages.length} aria-label="Topic turns">
          {messages.map((message) => (
            <FocusTurn
              key={message.id || message.content?.slice(0, 24)}
              message={message}
              assistantLabel={assistantLabel}
              highlighted={Boolean(message.id && message.id === highlightMessageId)}
            />
          ))}
        </ol>
      )}
    </section>
  )
}

function FocusTurn({
  message,
  assistantLabel,
  highlighted,
}: {
  message: BackendMessage
  assistantLabel: string
  highlighted: boolean
}) {
  const isAssistant = Boolean(message.is_from_bot)
  const role = isAssistant ? assistantLabel || 'Assistant' : 'You'
  const markdown = dossierMarkdown(message.content ?? '')
  const when = message.timestamp ? formatMessageTimestamp(new Date(message.timestamp)) : ''
  return (
    <li
      data-message-id={message.id || undefined}
      data-message-role={isAssistant ? 'assistant' : 'user'}
      aria-label={role}
      className={
        isAssistant
          ? `mr-auto w-full px-1 py-2 ${highlighted ? 'rounded-md ring-2 ring-ring' : ''}`
          : `ml-auto max-w-[85%] rounded-2xl bg-muted px-3 py-2 ${highlighted ? 'ring-2 ring-ring' : ''}`
      }
    >
      <div className={`flex items-baseline justify-between gap-3 text-xs text-muted-foreground ${isAssistant ? '' : 'flex-row-reverse'}`}>
        <span className="font-medium">{role}</span>
        {when ? <time dateTime={message.timestamp}>{when}</time> : null}
      </div>
      {markdown ? (
        <div className="mt-1" data-focus-markdown="true">
          <MarkdownStream deferMedia>{markdown}</MarkdownStream>
        </div>
      ) : (
        <p className="mt-1 text-sm text-muted-foreground">No text</p>
      )}
    </li>
  )
}

function OutputStrip({ artifacts }: { artifacts: DossierArtifact[] }) {
  if (artifacts.length === 0) return null
  return (
    <section aria-labelledby="dossier-outputs">
      <h3 id="dossier-outputs" className="text-sm font-medium">
        Outputs
      </h3>
      <ul className="mt-2 grid grid-cols-2 gap-2 sm:grid-cols-3 md:grid-cols-4">
        {artifacts.map((artifact) => (
          <li key={artifact.id} className="min-w-0">
            <LazyOutput artifact={artifact} />
          </li>
        ))}
      </ul>
    </section>
  )
}

function canOpenInApp(artifact: DossierArtifact): boolean {
  return artifact.status === 'active' && artifact.availability !== 'missing'
}

function LazyOutput({ artifact }: { artifact: DossierArtifact }) {
  const ref = useRef<HTMLDivElement | null>(null)
  const mounted = useRef(true)
  const [visible, setVisible] = useState(false)
  const [viewerOpen, setViewerOpen] = useState(false)
  const [openError, setOpenError] = useState('')
  const readable = artifact.kind === 'markdown' && canOpenInApp(artifact)
  const viewable = artifact.kind === 'image' && canOpenInApp(artifact)

  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
    }
  }, [])

  useEffect(() => {
    const node = ref.current
    if (!node) return
    if (typeof IntersectionObserver === 'undefined') return
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) {
          setVisible(true)
          observer.disconnect()
        }
      },
      { rootMargin: '200px' },
    )
    observer.observe(node)
    return () => observer.disconnect()
  }, [])

  useEffect(() => {
    if (visible && viewable) void ensureArtifactSession()
  }, [viewable, visible])

  const openViewer = () => {
    setOpenError('')
    void ensureArtifactSession()
      .then(() => {
        if (mounted.current) setViewerOpen(true)
      })
      .catch((error: unknown) => {
        if (mounted.current) setOpenError(error instanceof Error ? error.message : String(error))
      })
  }

  const label = `${artifact.name} · ${artifact.kind}`
  const preview =
    viewable && visible ? (
      <img src={artifact.url} alt="" className="h-24 w-full object-cover" loading="lazy" />
    ) : (
      <div className="flex h-24 items-center px-2 text-xs text-muted-foreground">{label}</div>
    )
  return (
    <div ref={ref} className="overflow-hidden rounded-md border border-border">
      {readable || viewable ? (
        <button
          type="button"
          className="block w-full cursor-pointer text-left focus-visible:outline focus-visible:outline-2 focus-visible:outline-ring"
          aria-label={viewable ? `View ${artifact.name}` : `Read ${artifact.name}`}
          onClick={openViewer}
        >
          {preview}
          <span className="block truncate px-2 py-1 text-xs underline-offset-2 hover:underline">Open</span>
        </button>
      ) : (
        <>
          {preview}
          {artifact.kind !== 'image' ? (
            <a
              href={artifact.url}
              target="_blank"
              rel="noopener noreferrer"
              className="block truncate px-2 py-1 text-xs underline-offset-2 hover:underline"
            >
              Open
            </a>
          ) : null}
        </>
      )}
      {openError ? (
        <p role="alert" className="px-2 pb-1 text-xs text-destructive">
          {openError}
        </p>
      ) : null}
      {viewerOpen && readable ? (
        <Suspense fallback={null}>
          <MarkdownFileDialog item={artifact} open={viewerOpen} onOpenChange={setViewerOpen} />
        </Suspense>
      ) : null}
      {viewerOpen && viewable ? (
        <ImageFileDialog name={artifact.name} url={artifact.url} open={viewerOpen} onOpenChange={setViewerOpen} />
      ) : null}
    </div>
  )
}
