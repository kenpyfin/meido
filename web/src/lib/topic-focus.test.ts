import { describe, expect, it } from 'vitest'
import type { ConversationTopic } from './conversation-topics'
import {
  activeDossierTopic,
  dossierMarkdown,
  dossierPlainText,
  readThreadSurface,
  sideChatHost,
  writeThreadSurface,
  THREAD_SURFACE_KEY,
} from './topic-focus'

function topic(id: string, start: string): ConversationTopic {
  return {
    id,
    session_id: null,
    parent_id: null,
    continues_topic_id: null,
    title: id,
    summary: '',
    start_message_id: `${id}-start`,
    end_message_id: `${id}-end`,
    start_ts: start,
    end_ts: start,
    message_count: 1,
    status: 'closed',
    source: 'auto',
    pinned: false,
    side_chats: [],
  }
}

describe('thread surface', () => {
  it('defaults to the transcript and persists focus', () => {
    const storage = new Map<string, string>()
    const memory = {
      getItem: (key: string) => storage.get(key) ?? null,
      setItem: (key: string, value: string) => {
        storage.set(key, value)
      },
    } as Storage
    expect(readThreadSurface(memory)).toBe('transcript')
    writeThreadSurface('focus', memory)
    expect(memory.getItem(THREAD_SURFACE_KEY)).toBe('focus')
    expect(readThreadSurface(memory)).toBe('focus')
    expect(sideChatHost('focus')).toBe('overlay')
    expect(sideChatHost('transcript')).toBe('split')
  })
})

describe('activeDossierTopic', () => {
  const topics = [topic('old', '2026-01-01T00:00:00Z'), topic('new', '2026-02-01T00:00:00Z')]

  it('uses the selected topic, then the newest', () => {
    expect(activeDossierTopic(topics, 'old')?.id).toBe('old')
    expect(activeDossierTopic(topics, 'missing')?.id).toBe('new')
    expect(activeDossierTopic([], 'old')).toBeNull()
  })
})

describe('dossierPlainText', () => {
  it('removes markdown and html images without dropping the surrounding words', () => {
    expect(dossierPlainText('Before ![shot](/api/uploads/a.png) after')).toBe('Before  after')
    expect(dossierPlainText('See <img src="/x.png" alt="x"> this')).toBe('See  this')
  })
})

describe('dossierMarkdown', () => {
  it('keeps markdown and drops inline images', () => {
    expect(dossierMarkdown('Keep **bold** ![shot](/api/uploads/a.png)')).toBe('Keep **bold**')
    expect(dossierMarkdown('See <img src="/x.png" alt="x"> this')).toBe('See  this')
  })

  it('drops upload note lines', () => {
    const content = [
      'combine these',
      '',
      '[document] filename=a.png bytes=1 mime=image/png saved_path=/home/u/a.png url=/api/artifacts/a/content',
      '![a.png](/api/artifacts/a/content)',
    ].join('\n')
    expect(dossierMarkdown(content)).toBe('combine these')
  })
})
