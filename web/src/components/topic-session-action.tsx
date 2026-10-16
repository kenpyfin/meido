import { useState } from 'react'
import { Button } from './ui/button'

export type TopicSessionActionProps = {
  topicTitle: string
  disabled: boolean
  onCreate: () => Promise<void>
}

export function TopicSessionAction({ topicTitle, disabled, onCreate }: TopicSessionActionProps) {
  const [pending, setPending] = useState(false)
  const [error, setError] = useState('')

  return (
    <div className="border-t border-border px-3 py-3 md:px-4" data-topic-session-action="true">
      <div className="mx-auto flex w-full max-w-[var(--aui-thread-max-width)] flex-col gap-2">
        {topicTitle ? <p className="truncate text-sm font-medium">{topicTitle}</p> : null}
        <p className="text-sm text-muted-foreground">Continue this topic in a fresh session</p>
        {error ? (
          <p role="alert" className="text-sm">
            {error}
          </p>
        ) : null}
        <Button
          type="button"
          className="w-fit cursor-pointer"
          disabled={disabled || pending}
          onClick={() => {
            setPending(true)
            setError('')
            void onCreate()
              .catch((err: unknown) => {
                setError(err instanceof Error ? err.message : 'Could not create a session')
              })
              .finally(() => setPending(false))
          }}
        >
          {pending ? 'Creating session…' : 'Create a new session'}
        </Button>
      </div>
    </div>
  )
}
