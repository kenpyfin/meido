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
import { api } from '../api/client'
import { AgentEngineSelect } from './agent-engine-select'
import { CursorModelFields, LlmModelFields, LocalModelFields } from './llm-model-fields'
import { ENGINE_OPTIONS, type EngineId } from '../lib/agent-engine'

export type PersonaCreateInput = {
  name: string
  engine: EngineId
  llm_provider?: string
  llm_model?: string
  llm_base_url?: string
  local_base_url?: string
  local_model?: string
  cursor_sdk_model?: string
  gemini_default_model?: string
}

type Props = {
  open: boolean
  onOpenChange: (open: boolean) => void
  existingNames: string[]
  onCreate: (input: PersonaCreateInput) => Promise<void>
}

export function NewPersonaDialog({ open, onOpenChange, existingNames, onCreate }: Props) {
  const engineFieldId = useId()
  const [name, setName] = useState('')
  const [engine, setEngine] = useState<EngineId>('classic')
  const [provider, setProvider] = useState('')
  const [model, setModel] = useState('')
  const [baseUrl, setBaseUrl] = useState('')
  const [localUrl, setLocalUrl] = useState('')
  const [localModel, setLocalModel] = useState('')
  const [cursorModel, setCursorModel] = useState('')
  const [geminiModel, setGeminiModel] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')

  useEffect(() => {
    if (open) {
      setName('')
      setEngine('classic')
      setProvider('')
      setModel('')
      setBaseUrl('')
      setLocalUrl('')
      setLocalModel('')
      setCursorModel('')
      setGeminiModel('')
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
  const localProvider = provider === 'ollama' || provider === 'llama' || provider === 'llamacpp'
  const modelReady =
    engine === 'cursor'
      ? cursorModel.trim().length > 0
      : engine === 'gemini_adk'
        ? geminiModel.trim().length > 0
        : provider.trim().length > 0 &&
          model.trim().length > 0 &&
          (!localProvider || baseUrl.trim().length > 0) &&
          (engine !== 'classic_cost_routing' || (localUrl.trim().length > 0 && localModel.trim().length > 0))
  const canCreate = trimmed.length > 0 && !validationError && modelReady && !busy

  const submit = async () => {
    if (!canCreate) return
    setBusy(true)
    setError('')
    try {
      await onCreate({
        name: trimmed,
        engine,
        llm_provider: engine === 'classic' || engine === 'classic_cost_routing' ? provider.trim() : undefined,
        llm_model: engine === 'classic' || engine === 'classic_cost_routing' ? model.trim() : undefined,
        llm_base_url: localProvider ? baseUrl.trim() : undefined,
        local_base_url: engine === 'classic_cost_routing' ? localUrl.trim() : undefined,
        local_model: engine === 'classic_cost_routing' ? localModel.trim() : undefined,
        cursor_sdk_model: engine === 'cursor' ? cursorModel.trim() : undefined,
        gemini_default_model: engine === 'gemini_adk' ? geminiModel.trim() : undefined,
      })
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
            Each persona has its own chat history, memory, and engine settings. Models come from the provider list.
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
          {engine === 'classic' || engine === 'classic_cost_routing' ? (
            <LlmModelFields
              api={api}
              provider={provider}
              model={model}
              baseUrl={baseUrl}
              onProvider={setProvider}
              onModel={setModel}
              onBaseUrl={setBaseUrl}
              disabled={busy}
              fillWhenEmpty
              fillProviderWhenMissing
            />
          ) : null}
          {engine === 'classic_cost_routing' ? (
            <LocalModelFields
              api={api}
              baseUrl={localUrl}
              model={localModel}
              onBaseUrl={setLocalUrl}
              onModel={setLocalModel}
              disabled={busy}
              fillWhenEmpty
            />
          ) : null}
          {engine === 'cursor' ? (
            <CursorModelFields api={api} model={cursorModel} onModel={setCursorModel} disabled={busy} fillWhenEmpty />
          ) : null}
          {engine === 'gemini_adk' ? (
            <LlmModelFields
              api={api}
              provider="google"
              model={geminiModel}
              onProvider={() => undefined}
              onModel={setGeminiModel}
              disabled={busy}
              lockProvider="google"
              fillWhenEmpty
            />
          ) : null}
          {!modelReady && trimmed ? (
            <p className="text-xs text-muted-foreground">Create stays off until you pick a model from the list.</p>
          ) : null}
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
