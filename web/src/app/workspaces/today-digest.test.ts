import { describe, expect, it } from 'vitest'
import { parseDigest } from './today-digest'

const SAMPLE = `
Digest:
Previous brief: Ship the board panel.
Open board:
- #12 [implement] owner 38 due - Daily overview
- #4 [todo] owner 7 due 2026-10-21T15:00:00Z Window quote
Board changes (24h):
- #12 todo -> implement (user)
Scheduled runs (24h):
- run #9 task #3 success Morning brief
Upcoming schedules:
- #3 persona 38 next 2026-10-21T17:00:00Z Write the brief
Latest topics:
- Board: Today panel \u2014 Height and formatting
Rejected proposals (14d), do not repeat:
- {"status":"deploy","title":"Skip tests"}
`

describe('parseDigest', () => {
  it('splits the live digest into labeled sections', () => {
    const sections = parseDigest(SAMPLE)
    expect(sections.map((section) => section.title)).toEqual([
      'Previous brief',
      'Open board',
      'Board changes (24h)',
      'Scheduled runs (24h)',
      'Upcoming schedules',
      'Latest topics',
      'Rejected proposals (14d), do not repeat',
    ])
    expect(sections[0].body).toBe('Ship the board panel.')
    expect(sections[1].items).toEqual([
      {
        kind: 'card',
        id: '12',
        status: 'implement',
        owner: '38',
        due: null,
        title: 'Daily overview',
      },
      {
        kind: 'card',
        id: '4',
        status: 'todo',
        owner: '7',
        due: '2026-10-21T15:00:00Z',
        title: 'Window quote',
      },
    ])
    expect(sections[2].items[0]).toEqual({
      kind: 'change',
      id: '12',
      from: 'todo',
      to: 'implement',
      actor: 'user',
    })
    expect(sections[3].items[0]).toEqual({
      kind: 'run',
      id: '9',
      taskId: '3',
      status: 'success',
      preview: 'Morning brief',
    })
    expect(sections[4].items[0]).toEqual({
      kind: 'schedule',
      id: '3',
      persona: '38',
      next: '2026-10-21T17:00:00Z',
      prompt: 'Write the brief',
    })
    expect(sections[5].items[0]).toEqual({
      kind: 'topic',
      name: 'Board',
      title: 'Today panel',
      summary: 'Height and formatting',
    })
    expect(sections[6].items[0]).toEqual({
      kind: 'note',
      text: '{\n  "status": "deploy",\n  "title": "Skip tests"\n}',
    })
  })

  it('keeps a line that does not match its section shape', () => {
    const sections = parseDigest('Open board:\n- not a card')
    expect(sections[0].items).toEqual([{ kind: 'note', text: 'not a card' }])
  })

  it('returns nothing for an empty digest', () => {
    expect(parseDigest('Digest:\n')).toEqual([])
    expect(parseDigest('   ')).toEqual([])
  })
})
