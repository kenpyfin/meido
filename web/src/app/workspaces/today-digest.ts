export type DigestItem =
  | {
      kind: 'card'
      id: string
      status: string
      owner: string
      due: string | null
      title: string
    }
  | { kind: 'change'; id: string; from: string; to: string; actor: string }
  | { kind: 'run'; id: string; taskId: string; status: string; preview: string }
  | { kind: 'schedule'; id: string; persona: string; next: string; prompt: string }
  | { kind: 'topic'; name: string; title: string; summary: string }
  | { kind: 'note'; text: string }

export type DigestSection = {
  title: string
  body: string
  items: DigestItem[]
}

const CARD = /^#(\d+)\s+\[([^\]]+)\]\s+owner\s+(\S+)\s+due\s+(\S+)(?:\s+(.*))?$/
const CHANGE = /^#(\d+)\s+(\S+)\s+->\s+(\S+)\s+\(([^)]*)\)$/
const RUN = /^run #(\d+)\s+task #(\d+)\s+(\S+)(?:\s+(.*))?$/
const SCHEDULE = /^#(\d+)\s+persona\s+(\S+)\s+next\s+(\S+)(?:\s+(.*))?$/

type ItemKind = 'card' | 'change' | 'run' | 'schedule' | 'topic' | 'note'

function itemKind(title: string): ItemKind {
  const name = title.toLowerCase()
  if (name.startsWith('open board')) return 'card'
  if (name.startsWith('board changes')) return 'change'
  if (name.startsWith('scheduled runs')) return 'run'
  if (name.startsWith('upcoming')) return 'schedule'
  if (name.startsWith('latest topics')) return 'topic'
  return 'note'
}

function prettyJson(text: string): string {
  const trimmed = text.trim()
  if (!trimmed.startsWith('{') && !trimmed.startsWith('[')) return text
  try {
    return JSON.stringify(JSON.parse(trimmed) as unknown, null, 2)
  } catch {
    return text
  }
}

function parseTopic(line: string): DigestItem {
  const split = line.match(/^(.+?):\s+(.*)$/)
  if (!split) return { kind: 'note', text: line }
  const rest = split[2]
  const dash = rest.indexOf(' \u2014 ')
  if (dash === -1) return { kind: 'topic', name: split[1], title: rest, summary: '' }
  return {
    kind: 'topic',
    name: split[1],
    title: rest.slice(0, dash),
    summary: rest.slice(dash + 3),
  }
}

function parseItem(section: string, line: string): DigestItem {
  const kind = itemKind(section)
  if (kind === 'card') {
    const match = line.match(CARD)
    if (match) {
      return {
        kind: 'card',
        id: match[1],
        status: match[2],
        owner: match[3],
        due: match[4] === '-' ? null : match[4],
        title: (match[5] ?? '').trim(),
      }
    }
  }
  if (kind === 'change') {
    const match = line.match(CHANGE)
    if (match) {
      return { kind: 'change', id: match[1], from: match[2], to: match[3], actor: match[4] }
    }
  }
  if (kind === 'run') {
    const match = line.match(RUN)
    if (match) {
      return {
        kind: 'run',
        id: match[1],
        taskId: match[2],
        status: match[3],
        preview: (match[4] ?? '').trim(),
      }
    }
  }
  if (kind === 'schedule') {
    const match = line.match(SCHEDULE)
    if (match) {
      return {
        kind: 'schedule',
        id: match[1],
        persona: match[2],
        next: match[3],
        prompt: (match[4] ?? '').trim(),
      }
    }
  }
  if (kind === 'topic') return parseTopic(line)
  return { kind: 'note', text: prettyJson(line) }
}

function startSection(sections: DigestSection[], title: string, body: string): DigestSection {
  const section: DigestSection = { title, body, items: [] }
  sections.push(section)
  return section
}

export function parseDigest(text: string): DigestSection[] {
  const sections: DigestSection[] = []
  let current: DigestSection | null = null

  for (const raw of text.split('\n')) {
    const line = raw.trim()
    if (!line) continue
    if (line.startsWith('- ')) {
      if (!current) current = startSection(sections, 'Notes', '')
      current.items.push(parseItem(current.title, line.slice(2).trim()))
      continue
    }
    const header = line.match(/^([^:]+):\s*(.*)$/)
    if (header) {
      current = startSection(sections, header[1].trim(), header[2].trim())
      continue
    }
    if (!current) current = startSection(sections, 'Notes', line)
    else current.body = current.body ? `${current.body}\n${line}` : line
  }

  return sections.filter(
    (section) => section.title.toLowerCase() !== 'digest' || section.body.length > 0 || section.items.length > 0,
  )
}
