import { useCallback, useEffect, useState, type ReactNode } from 'react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import type { Persona } from '../types'

type Props = {
  activePersonaId: number | null
  personas: Persona[]
  onCreatePersona: () => void | Promise<void>
  onRenamePersona: (personaId: number, name: string) => Promise<void>
  onDeletePersona: (personaId: number) => void
}

function SettingsAlert({ tone, children }: { tone: 'ok' | 'err'; children: ReactNode }) {
  return (
    <div
      role={tone === 'ok' ? 'status' : undefined}
      className={
        tone === 'ok'
          ? 'rounded-md border border-green-500/30 bg-green-500/10 px-3 py-2 text-sm text-green-800 dark:text-green-200'
          : 'rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive'
      }
    >
      {children}
    </div>
  )
}

/**
 * Settings → Overview: rename / delete the sidebar-selected persona.
 * Create stays available here too so management is not buried in the list UI.
 */
export function SettingsPersonaPanel({
  activePersonaId,
  personas,
  onCreatePersona,
  onRenamePersona,
  onDeletePersona,
}: Props) {
  const active = personas.find((p) => p.id === activePersonaId) ?? null
  const isDefault = active?.name === 'default'
  const [nameDraft, setNameDraft] = useState(active?.name ?? '')
  const [busy, setBusy] = useState(false)
  const [notice, setNotice] = useState<string | null>(null)
  const [error, setError] = useState('')

  useEffect(() => {
    setNameDraft(active?.name ?? '')
    setNotice(null)
    setError('')
  }, [active?.id, active?.name])

  const dirty = active != null && nameDraft.trim() !== active.name
  const canRename = active != null && !isDefault && dirty && nameDraft.trim().length > 0 && !busy

  const saveRename = useCallback(async () => {
    if (active == null || !canRename) return
    setBusy(true)
    setError('')
    setNotice(null)
    try {
      await onRenamePersona(active.id, nameDraft.trim())
      setNotice(`Renamed to “${nameDraft.trim()}”.`)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }, [active, canRename, nameDraft, onRenamePersona])

  return (
    <div className="mc-settings-persona rounded-md border border-[color:var(--mc-border-soft)] p-3">
      <span className="mb-1 block text-sm font-bold">Active persona</span>
      <span className="mb-3 block text-xs leading-snug text-muted-foreground">
        Select a persona in the sidebar, then rename or delete it here. The reserved{' '}
        <code className="text-[11px]">default</code> persona cannot be renamed or deleted.
      </span>

      {active == null ? (
        <span className="text-xs text-muted-foreground">No persona selected.</span>
      ) : (
        <div className="flex flex-col gap-3">
          <Input
            value={nameDraft}
            onChange={(e) => setNameDraft(e.target.value)}
            disabled={isDefault || busy}
            placeholder="Persona name"
            aria-label="Persona name"
            onKeyDown={(e) => {
              if (e.key === 'Enter' && canRename) {
                e.preventDefault()
                void saveRename()
              }
            }}
          />
          <div className="flex flex-wrap items-center gap-2">
            <Button size="sm" disabled={!canRename} onClick={() => void saveRename()}>
              {busy ? 'Saving…' : 'Rename'}
            </Button>
            <Button
              size="sm"
              variant="destructive"
              disabled={isDefault || busy}
              onClick={() => onDeletePersona(active.id)}
            >
              Delete persona
            </Button>
            <Button size="sm" variant="secondary" disabled={busy} onClick={() => void onCreatePersona()}>
              + New persona
            </Button>
          </div>
          {isDefault ? (
            <span className="text-xs text-muted-foreground">
              This is the default persona — rename and delete are disabled.
            </span>
          ) : null}
          {notice ? <SettingsAlert tone="ok">{notice}</SettingsAlert> : null}
          {error ? <SettingsAlert tone="err">{error}</SettingsAlert> : null}
        </div>
      )}
    </div>
  )
}
