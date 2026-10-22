import { Button } from '@/components/ui/button'
import { Dialog, DialogClose, DialogContent } from '@/components/ui/dialog'
import { useIsMobile } from '@/lib/use-is-mobile'
import React from 'react'
import { LedgerBook, LedgerDialogHeader, LedgerJump, LedgerSection, ServiceNote, type LedgerIndexEntry } from '../../components/ledger-chrome'
import { SettingsPanelSkeleton } from '../../components/skeleton'
import type { PersonaDialogProps, PersonaLedgerPage } from './types'

const SettingsAgentEnginePanel = React.lazy(() =>
  import('../../components/settings-agent-engine').then((m) => ({ default: m.SettingsAgentEnginePanel })),
)
const SettingsPersonaPanel = React.lazy(() =>
  import('../../components/settings-persona').then((m) => ({ default: m.SettingsPersonaPanel })),
)
const SettingsHooksPanel = React.lazy(() =>
  import('../../components/settings-hooks-skills').then((m) => ({ default: m.SettingsHooksPanel })),
)
const SettingsSkillsPanel = React.lazy(() =>
  import('../../components/settings-hooks-skills').then((m) => ({ default: m.SettingsSkillsPanel })),
)

type VaultSecretRow = {
  name: string
  kind: string
  skill: string | null
  created_at: string
  persona_id?: number | null
  legacy?: boolean
}

function VaultSecretList({
  items,
  busyName,
  onForget,
}: {
  items: VaultSecretRow[]
  busyName: string | null
  onForget: (name: string) => void
}) {
  return (
    <ul className="mt-2 space-y-2">
      {items.map((item) => (
        <li key={item.name} className="flex items-center justify-between gap-3 text-sm">
          <span>
            <code className="text-xs">{item.name}</code>
            <span className="ml-2 text-xs text-muted-foreground">
              {item.kind}
              {item.skill ? ` · ${item.skill}` : ''}
            </span>
          </span>
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={busyName === item.name}
            onClick={() => onForget(item.name)}
          >
            Forget
          </Button>
        </li>
      ))}
    </ul>
  )
}

function VaultedSecretsPanel({
  api,
  open,
  activePersonaId,
}: {
  api: PersonaDialogProps['api']
  open: boolean
  activePersonaId: number | null
}) {
  const [items, setItems] = React.useState<VaultSecretRow[]>([])
  const [error, setError] = React.useState('')
  const [busyName, setBusyName] = React.useState<string | null>(null)

  const load = React.useCallback(async () => {
    try {
      const path =
        activePersonaId != null
          ? `/api/secrets?persona_id=${encodeURIComponent(String(activePersonaId))}`
          : '/api/secrets'
      const data = await api<{ secrets: VaultSecretRow[] }>(path)
      setItems(data.secrets)
      setError('')
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Could not load vaulted secrets')
    }
  }, [activePersonaId, api])

  React.useEffect(() => {
    if (open) void load()
  }, [open, load])

  async function forget(name: string) {
    setBusyName(name)
    try {
      await api(`/api/secrets?name=${encodeURIComponent(name)}`, { method: 'DELETE' })
      await load()
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Could not forget that secret')
    } finally {
      setBusyName(null)
    }
  }

  const personaItems =
    activePersonaId == null ? items : items.filter((item) => item.legacy !== true)
  const legacyItems = activePersonaId == null ? [] : items.filter((item) => item.legacy === true)

  return (
    <LedgerSection title="Vaulted secrets" scope="live" className="mb-4">
      <ServiceNote summary="Forget removes a name stored for this persona. The value is not shown.">
        The agent stores the name when it calls vault_secret. The value stays in the runtime secrets
        file. Names saved before persona tagging are listed under Shared (legacy).
      </ServiceNote>
      {error ? <p className="mt-2 text-sm text-destructive">{error}</p> : null}
      {personaItems.length === 0 && legacyItems.length === 0 ? (
        <p className="mt-2 text-sm text-muted-foreground">No vaulted secrets for this persona.</p>
      ) : null}
      {personaItems.length > 0 ? (
        <div className="mt-2">
          {activePersonaId != null ? (
            <p className="text-xs font-medium text-muted-foreground">This persona</p>
          ) : null}
          <VaultSecretList items={personaItems} busyName={busyName} onForget={(name) => void forget(name)} />
        </div>
      ) : null}
      {legacyItems.length > 0 ? (
        <div className="mt-3">
          <p className="text-xs font-medium text-muted-foreground">Shared (legacy)</p>
          <VaultSecretList items={legacyItems} busyName={busyName} onForget={(name) => void forget(name)} />
        </div>
      ) : null}
    </LedgerSection>
  )
}

const PAGE_LABELS: Record<PersonaLedgerPage, string> = {
  profile: 'Profile',
  engine: 'Engine',
  hooks: 'Hooks',
  skills: 'Skills',
}

function indexEntries(engineMark: 'Missing' | undefined): LedgerIndexEntry[] {
  return (Object.keys(PAGE_LABELS) as PersonaLedgerPage[]).map((id) => {
    if (id === 'engine' && engineMark) return { id, label: PAGE_LABELS[id], state: engineMark }
    return { id, label: PAGE_LABELS[id] }
  })
}

export function PersonaDialog({ api, activePersonaId, personas, persona }: PersonaDialogProps) {
  const personaName = personas.find((item) => item.id === activePersonaId)?.name ?? null
  const onCreatePersona = persona.onCreatePersona
  const onRenamePersona = persona.onRenamePersona
  const onDeletePersona = persona.onDeletePersona
  const mobile = useIsMobile()
  const page = persona.page

  return (
    <Dialog open={persona.open} onOpenChange={persona.onOpenChange}>
      <DialogContent className="mc-ledger-dialog max-w-[1040px]">
        <LedgerDialogHeader
          eyebrow="Attendant dossier"
          title={personaName ?? 'Persona'}
          description="Engine, hooks, skills, and secrets for this persona. Connections, channels, and shared engine defaults stay in Settings."
        />
        <div className="mc-ledger-body">
        {persona.error ? (
          <div role="alert" className="rounded-lg border border-destructive/30 bg-destructive/10 p-3 text-sm text-destructive mb-2">
            {persona.error}
          </div>
        ) : null}
        <LedgerBook
          label="Attendant index"
          entries={indexEntries(persona.engineMark)}
          page={page}
          onPageChange={(id) => persona.onPageChange(id as PersonaLedgerPage)}
          revealPage={persona.revealPage}
          open={persona.open}
          mobile={mobile}
        >
          <React.Suspense fallback={<SettingsPanelSkeleton />}>
            {page === 'profile' ? (
              <>
              {onCreatePersona && onRenamePersona && onDeletePersona ? (
                <div className="mb-3">
                  <SettingsPersonaPanel
                    activePersonaId={activePersonaId}
                    personas={personas}
                    onCreatePersona={onCreatePersona}
                    onRenamePersona={onRenamePersona}
                    onDeletePersona={onDeletePersona}
                  />
                </div>
              ) : null}
              <VaultedSecretsPanel api={api} open={persona.open} activePersonaId={activePersonaId} />
              </>
            ) : null}
            {page === 'engine' ? (
              <SettingsAgentEnginePanel
                api={api}
                onError={persona.setError}
                activePersonaId={activePersonaId}
                personas={personas}
                onPersonaBulletinChanged={persona.onPersonaBulletinChanged}
                onOpenConnections={
                  persona.onOpenSettingsPage ? () => persona.onOpenSettingsPage?.('connections') : undefined
                }
              />
            ) : null}
            {page === 'hooks' ? (
              <SettingsHooksPanel
                api={api}
                onError={persona.setError}
                activePersonaId={activePersonaId}
              />
            ) : null}
            {page === 'skills' ? (
              <SettingsSkillsPanel
                api={api}
                onError={persona.setError}
                activePersonaId={activePersonaId}
              />
            ) : null}
          </React.Suspense>
        </LedgerBook>
        <div className="mc-ledger-footer">
          {persona.onOpenHousehold ? (
            <LedgerJump onClick={persona.onOpenHousehold}>Open household settings</LedgerJump>
          ) : <span />}
          <DialogClose>
            <Button variant="secondary">Close</Button>
          </DialogClose>
        </div>
        </div>
      </DialogContent>
    </Dialog>
  )
}
