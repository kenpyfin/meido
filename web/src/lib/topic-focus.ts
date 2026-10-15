import type { BackendMessage, PersonaMessageBookmark } from '../types'
import {
  sortTopicsByTimeDesc,
  type ConversationTopic,
  type TopicSideChat,
} from './conversation-topics'
import { stripDocumentNoteLines } from './user-attachment-notes'

export const THREAD_SURFACE_KEY = 'finally-a-value-bot_thread_surface'
export const TOPIC_DOSSIER_LIMIT = 20

export type ThreadSurface = 'focus' | 'transcript'
export type SideChatHost = 'split' | 'overlay'

export type DossierArtifact = {
  id: string
  name: string
  kind: string
  size_bytes: number
  status: string
  availability: string
  message_id?: string | null
  url: string
  preview_url: string
}

export type TopicDossier = {
  ok: boolean
  segmenting: boolean
  topic: ConversationTopic
  messages: BackendMessage[]
  has_older: boolean
  has_newer: boolean
  bookmarks: PersonaMessageBookmark[]
  artifacts: DossierArtifact[]
  evidence: {
    start_message_id: string
    older_before_id: string | null
    newer_after_id: string | null
  }
}

export type DossierCursor =
  | { kind: 'latest' }
  | { kind: 'before' | 'after' | 'around'; messageId: string }

export function dossierCursorKey(cursor: DossierCursor): string {
  return cursor.kind === 'latest' ? 'latest' : `${cursor.kind}:${cursor.messageId}`
}

export function readThreadSurface(storage: Storage | null | undefined): ThreadSurface {
  try {
    return storage?.getItem(THREAD_SURFACE_KEY) === 'focus' ? 'focus' : 'transcript'
  } catch {
    return 'transcript'
  }
}

export function writeThreadSurface(surface: ThreadSurface, storage: Storage | null | undefined) {
  try {
    storage?.setItem(THREAD_SURFACE_KEY, surface)
  } catch {
    // The in-memory choice still applies when storage is blocked.
  }
}

export function sideChatHost(surface: ThreadSurface): SideChatHost {
  return surface === 'focus' ? 'overlay' : 'split'
}

export function activeDossierTopic(
  topics: ConversationTopic[],
  selectedId: string | null,
): ConversationTopic | null {
  if (selectedId) {
    const selected = topics.find((topic) => topic.id === selectedId)
    if (selected) return selected
  }
  return sortTopicsByTimeDesc(topics)[0] ?? null
}

function stripDossierImages(content: string): string {
  return stripDocumentNoteLines(content)
    .replace(/!\[[^\]]*]\([^)]*\)/g, '')
    .replace(/<img\b[^>]*>/gi, '')
    .replace(/[ \t]+\n/g, '\n')
    .trim()
}

/** Drop inline images so bookmark previews stay plain text. */
export function dossierPlainText(content: string): string {
  return stripDossierImages(content)
}

/** Markdown for a focus turn. Images are removed; the rest of the markup stays. */
export function dossierMarkdown(content: string): string {
  return stripDossierImages(content)
}

export function dossierSideChats(topic: ConversationTopic | null): TopicSideChat[] {
  return topic?.side_chats ?? []
}
