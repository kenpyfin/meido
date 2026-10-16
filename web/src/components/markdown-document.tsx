import { useEffect, useState } from 'react'
import { MarkdownStream } from '@/components/markdown-stream'

/** Fetches a same-origin text preview. `url` null skips the request. */
export function usePreviewText(url: string | null): { text: string; error: string; loading: boolean } {
  const [text, setText] = useState('')
  const [error, setError] = useState('')
  const [loading, setLoading] = useState(Boolean(url))

  useEffect(() => {
    if (!url) {
      setText('')
      setError('')
      setLoading(false)
      return
    }
    const controller = new AbortController()
    setLoading(true)
    setError('')
    setText('')
    fetch(url, { signal: controller.signal })
      .then(async (res) => {
        if (!res.ok) throw new Error(await res.text())
        return res.text()
      })
      .then((body) => {
        if (!controller.signal.aborted) {
          setText(body)
          setLoading(false)
        }
      })
      .catch((e: unknown) => {
        if (!controller.signal.aborted) {
          setError(e instanceof Error ? e.message : String(e))
          setLoading(false)
        }
      })
    return () => controller.abort()
  }, [url])

  return { text, error, loading }
}

/** Static rendered markdown. Callers own the 256 KiB preview notice. */
export function MarkdownDocument({ text }: { text: string }) {
  return <MarkdownStream>{text}</MarkdownStream>
}
