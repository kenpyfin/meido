import { describe, expect, it } from 'vitest'
import { parseUserAttachmentNotes, stripDocumentNoteLines } from './user-attachment-notes'

const WEB_MESSAGE = [
  'please help me combine these two images into a pdf',
  '',
  '[document] filename=IMG_3083.jpeg bytes=211472 mime=image/jpeg tool_path=upload/web/1/20/a-IMG_3083.jpeg saved_path=/home/u/proj/./workspace/shared/upload/web/1/20/a-IMG_3083.jpeg url=/api/artifacts/aaa/content',
  '![IMG_3083.jpeg](/api/artifacts/aaa/content)',
  '[document] filename=IMG_3080.png bytes=386176 mime=image/png tool_path=upload/web/1/20/b-IMG_3080.png saved_path=/home/u/proj/./workspace/shared/upload/web/1/20/b-IMG_3080.png url=/api/artifacts/bbb/content',
  '![IMG_3080.png](/api/artifacts/bbb/content)',
].join('\n')

describe('parseUserAttachmentNotes', () => {
  it('splits typed text from web upload notes and dedupes by url', () => {
    const parsed = parseUserAttachmentNotes(WEB_MESSAGE)
    expect(parsed.text).toBe('please help me combine these two images into a pdf')
    expect(parsed.attachments).toEqual([
      { name: 'IMG_3083.jpeg', url: '/api/artifacts/aaa/content', mime: 'image/jpeg', isImage: true, skipped: undefined },
      { name: 'IMG_3080.png', url: '/api/artifacts/bbb/content', mime: 'image/png', isImage: true, skipped: undefined },
    ])
  })

  it('keeps filenames that contain spaces', () => {
    const parsed = parseUserAttachmentNotes(
      '[document] filename=My Scan 2.pdf bytes=10 mime=application/pdf saved_path=/tmp/x.pdf',
    )
    expect(parsed.attachments[0]).toMatchObject({ name: 'My Scan 2.pdf', isImage: false })
    expect(parsed.text).toBe('')
  })

  it('reports skipped attachments', () => {
    const parsed = parseUserAttachmentNotes('[document] filename=big.mov bytes=999 mime=video/quicktime skipped=too_large')
    expect(parsed.attachments[0]).toMatchObject({ name: 'big.mov', skipped: 'too_large' })
  })

  it('leaves ordinary text untouched', () => {
    const text = 'see [the doc](https://example.com) and [document] notes in general'
    expect(parseUserAttachmentNotes(text)).toEqual({ text, attachments: [] })
  })
})

describe('stripDocumentNoteLines', () => {
  it('removes note lines but keeps markdown image links', () => {
    expect(stripDocumentNoteLines(WEB_MESSAGE)).toBe(
      [
        'please help me combine these two images into a pdf',
        '',
        '![IMG_3083.jpeg](/api/artifacts/aaa/content)',
        '![IMG_3080.png](/api/artifacts/bbb/content)',
      ].join('\n'),
    )
  })
})
