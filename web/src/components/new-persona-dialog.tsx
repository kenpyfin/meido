import { useEffect, useId, useState } from 'react'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { AgentEngineSelect } from './agent-engine-select'
import { ENGINE_OPTIONS, type EngineId } from '../lib/agent-engine'

type Props = {
  open: boolean
  onOpenChange: (open: boolean) => void
  existingNames: string[]
  onCreate: (name: string, engine: EngineId) => Promise<void>
}

export function NewPersonaDialog({ open, onOpenChange, existingNames, onCreate }: Props) {
  const engineFieldId = useId()
  const [name, setName] = useState('')
  const [engine, setEngine] = useState<EngineId>('classic')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')

  useEffect(() => {
    if (open) {
      setName('')
      setEngine('classic')
      setError('')
    }
  }, [open])

  const trimmed = name.trim()
  const duplicate = existingNames.some((n) => n === trimmed)
  const reserved = trimmed === 'default'
  const validationError = duplicate
    ? `A persona named “${trimmed}” already exists.`
    : reserved
      ? '“default” is reserved.'
      : ''
  const canCreate = trimmed.length > 0 && !validationError && !busy

  const submit = async () => {
    if (!canCreate) return
    setBusy(true)
    setError('')
    try {
      await onCreate(trimmed, engine)
      onOpenChange(false)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={(o) => !busy && onOpenChange(o)}>
      <DialogContent className="max-w-[420px]">
        <DialogHeader>
          <DialogTitle>New persona</DialogTitle>
          <DialogDescription>
            Each persona has its own chat history, memory, and engine settings.
          </DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-2">
          <Input
            placeholder="e.g. research, sourdough, ops"
            value={name}
            onChange={(e) => setName(e.target.value)}
            autoFocus
            disabled={busy}
            aria-label="Persona name"
            onKeyDown={(e) => {
              if (e.key === 'Enter') {
                e.preventDefault()
                void submit()
              }
            }}
          />
          {validationError || error ? (
            <p className="text-xs text-destructive">{validationError || error}</p>
          ) : null}
          <div className="mt-2 flex flex-col gap-1.5">
            <label htmlFor={engineFieldId} className="text-xs text-muted-foreground">
              Agent engine
            </label>
            <AgentEngineSelect
              id={engineFieldId}
              value={engine}
              onChange={setEngine}
              disabled={busy}
            />
            <p className="text-xs text-muted-foreground">
              {ENGINE_OPTIONS.find((opt) => opt.id === engine)?.subtitle}
            </p>
          </div>
        </div>
        <DialogFooter>
          <Button variant="outline" disabled={busy} onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button onClick={() => void submit()} disabled={!canCreate}>
            {busy ? 'Creating…' : 'Create persona'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
