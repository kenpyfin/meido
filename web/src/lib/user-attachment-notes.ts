/**
 * Channels append model-facing attachment notes to the stored user message:
 *
 *   [document] filename=a.png bytes=12 mime=image/png tool_path=… saved_path=/abs/… url=/api/artifacts/…
 *   ![a.png](/api/artifacts/…)
 *
 * The model needs those paths; the operator only needs the file. These helpers split the notes
 * out so the thread can show the typed text plus a thumbnail or chip per attachment.
 */

export type UserAttachmentNote = {
  name: string
  url?: string
  mime?: string
  isImage: boolean
  skipped?: string
}

export type ParsedUserMessage = {
  text: string
  attachments: UserAttachmentNote[]
}

const DOCUMENT_NOTE_RE = /^\[document\]\s+(?=.*\bfilename=)(.*)$/
const ARTIFACT_LINK_RE = /^(!?)\[([^\]]*)\]\((\/api\/artifacts\/[^\s)]+)\)$/
const NOTE_FIELD_RE = /(\w+)=(.*?)(?=\s+\w+=|$)/g
const IMAGE_EXT_RE = /\.(png|jpe?g|gif|webp|bmp|avif|heic|heif)$/i

function parseNoteFields(body: string): Record<string, string> {
  const fields: Record<string, string> = {}
  for (const match of body.matchAll(NOTE_FIELD_RE)) {
    fields[match[1]] = match[2].trim()
  }
  return fields
}

function looksLikeImage(name: string, mime?: string): boolean {
  if (mime) return mime.toLowerCase().startsWith('image/')
  return IMAGE_EXT_RE.test(name)
}

export function isAttachmentNoteLine(line: string): boolean {
  const trimmed = line.trim()
  return DOCUMENT_NOTE_RE.test(trimmed) || ARTIFACT_LINK_RE.test(trimmed)
}

export function parseUserAttachmentNotes(content: string): ParsedUserMessage {
  const attachments: UserAttachmentNote[] = []
  const byUrl = new Map<string, UserAttachmentNote>()
  const textLines: string[] = []

  for (const line of content.split('\n')) {
    const trimmed = line.trim()
    const doc = DOCUMENT_NOTE_RE.exec(trimmed)
    if (doc) {
      const fields = parseNoteFields(doc[1])
      const name = fields.filename || 'attachment'
      const note: UserAttachmentNote = {
        name,
        url: fields.url || undefined,
        mime: fields.mime || undefined,
        isImage: looksLikeImage(name, fields.mime),
        skipped: fields.skipped || undefined,
      }
      if (note.url) {
        if (byUrl.has(note.url)) continue
        byUrl.set(note.url, note)
      }
      attachments.push(note)
      continue
    }
    const link = ARTIFACT_LINK_RE.exec(trimmed)
    if (link) {
      const [, bang, label, url] = link
      const existing = byUrl.get(url)
      if (existing) {
        if (bang) existing.isImage = true
        continue
      }
      const name = label.trim() || 'attachment'
      const note: UserAttachmentNote = { name, url, isImage: Boolean(bang) || looksLikeImage(name) }
      byUrl.set(url, note)
      attachments.push(note)
      continue
    }
    textLines.push(line)
  }

  if (attachments.length === 0) return { text: content, attachments }
  return { text: textLines.join('\n').trim(), attachments }
}

/** Drops `[document] …` note lines, keeping markdown links for renderers that show them. */
export function stripDocumentNoteLines(content: string): string {
  if (!content.includes('[document]')) return content
  return content
    .split('\n')
    .filter((line) => !DOCUMENT_NOTE_RE.test(line.trim()))
    .join('\n')
    .replace(/\n{3,}/g, '\n\n')
    .trim()
}
