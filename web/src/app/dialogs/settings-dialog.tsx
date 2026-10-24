import { Button } from '@/components/ui/button'
import { Select, SelectTrigger, SelectContent, SelectItem, SelectValue } from '@/components/ui/select'
import { Dialog, DialogContent, DialogClose } from '@/components/ui/dialog'
import React from 'react'
import { useIsMobile } from '@/lib/use-is-mobile'

import { LedgerBook, LedgerDialogHeader, LedgerJump, LedgerSection, LedgerStatusRow, ServiceNote, type LedgerIndexEntry } from '../../components/ledger-chrome'
import { SettingsPanelSkeleton, OverviewStatusSkeleton } from '../../components/skeleton'
import type { InstallationStatus } from '../../types'
import type { SettingsDialogProps, SettingsLedgerPage } from './types'

const SettingsRuntimePanel = React.lazy(() =>
  import('../../components/settings-runtime').then((m) => ({ default: m.SettingsRuntimePanel })),
)
const SettingsIntegrationsPanel = React.lazy(() =>
  import('../../components/settings-integrations').then((m) => ({ default: m.SettingsIntegrationsPanel })),
)
const SettingsConnectionsPanel = React.lazy(() =>
  import('../../components/settings-connections').then((m) => ({ default: m.SettingsConnectionsPanel })),
)
const SettingsTopicsModelPanel = React.lazy(() =>
  import('../../components/settings-topics-model').then((m) => ({ default: m.SettingsTopicsModelPanel })),
)
const SettingsEnvironmentPanel = React.lazy(() =>
  import('../../components/settings-environment').then((m) => ({ default: m.SettingsEnvironmentPanel })),
)

const PAGE_LABELS: Record<SettingsLedgerPage, string> = {
  overview: 'Overview',
  environment: 'Environment',
  'system-llm': 'System LLM',
  connections: 'Connections',
  integrations: 'Integrations',
  channels: 'Channels',
}

function restartOwed(status: InstallationStatus | null): boolean {
  if (!status) return false
  return (
    (status.requires_restart_for_env_changes ?? status.requires_restart_to_apply_runtime_settings) === true
  )
}

function indexEntries(status: InstallationStatus | null): LedgerIndexEntry[] {
  return (Object.keys(PAGE_LABELS) as SettingsLedgerPage[]).map((id) => {
    if (id === 'overview' && restartOwed(status)) return { id, label: PAGE_LABELS[id], state: 'Restart needed' }
    if (id === 'overview' && status?.needs_initialization) return { id, label: PAGE_LABELS[id], state: 'Missing' }
    if (id === 'connections' && status != null && !status.llm_ready) return { id, label: PAGE_LABELS[id], state: 'Missing' }
    if (id === 'integrations' && status != null && !status.channel_ready) return { id, label: PAGE_LABELS[id], state: 'Missing' }
    return { id, label: PAGE_LABELS[id] }
  })
}

export function SettingsDialog({ appearance, api, activePersonaId, personas, settings, onRunSetupWizard, onConfigurePersona }: SettingsDialogProps) {
  const settingsDialogOpen = settings.open
  const setSettingsDialogOpen = settings.onOpenChange
  const settingsError = settings.error
  const setSettingsError = settings.setError
  const restartNotice = settings.restartNotice
  const installationStatus = settings.installationStatus
  const restartBusy = settings.restartBusy
  const requestRestart = settings.requestRestart
  const bindings = settings.bindings
  const updateChannelPersonaPolicy = settings.updateChannelPersonaPolicy
  const reloadInstallationStatus = settings.reloadInstallationStatus
  const mobile = useIsMobile()
  const personaName = personas.find((item) => item.id === activePersonaId)?.name ?? null
  const page = settings.page

  return (
    <Dialog open={settingsDialogOpen} onOpenChange={setSettingsDialogOpen}>
      <DialogContent className="mc-ledger-dialog max-w-[1040px]">
        <LedgerDialogHeader
          eyebrow="Household ledger"
          title="Web UI configuration"
          description="Shared setup for every persona. Provider API keys are under Connections. A persona’s engine, hooks, skills, and secrets are on the Persona page. Configure Telegram, Discord, and WhatsApp under Integrations. Restart Meido after changing channel tokens."
        />
        <div className="mc-ledger-body">
        {settingsError ? (
          <div role="alert" className="rounded-lg border border-destructive/30 bg-destructive/10 p-3 text-sm text-destructive mb-2">{settingsError}</div>
        ) : null}
        {restartNotice ? (
          <div role="status" className="mc-notice-owed mb-2">{restartNotice}</div>
        ) : null}
        <LedgerBook
          label="Household index"
          entries={indexEntries(installationStatus)}
          page={page}
          onPageChange={(id) => settings.onPageChange(id as SettingsLedgerPage)}
          revealPage={settings.revealPage}
          open={settingsDialogOpen}
          mobile={mobile}
        >
          <React.Suspense fallback={<SettingsPanelSkeleton />}>
          {page === 'environment' ? (
            <SettingsEnvironmentPanel
              api={api}
              onRunWizard={() => onRunSetupWizard?.()}
              onChanged={() => void reloadInstallationStatus()}
            />
          ) : null}
          {page === 'overview' ? (
            <>
            {installationStatus ? (
              <LedgerSection
                title="Readiness"
                owed={
                  !installationStatus.llm_ready ||
                  !installationStatus.channel_ready ||
                  !installationStatus.cursor_engine_ready ||
                  installationStatus.needs_initialization === true ||
                  restartOwed(installationStatus)
                }
                className="mb-2"
              >
                <div className="mc-ledger-rows">
                  <LedgerStatusRow
                    label="LLM"
                    state={installationStatus.llm_ready ? 'Ready' : 'Missing'}
                    detail={installationStatus.llm_ready ? undefined : (
                      <LedgerJump onClick={() => settings.onPageChange('connections')}>Open Connections</LedgerJump>
                    )}
                  />
                  <LedgerStatusRow
                    label="Channels"
                    state={installationStatus.channel_ready ? 'Ready' : 'Missing'}
                    detail={installationStatus.channel_ready ? undefined : (
                      <LedgerJump onClick={() => settings.onPageChange('integrations')}>Open Integrations</LedgerJump>
                    )}
                  />
                  <LedgerStatusRow
                    label="Cursor"
                    state={installationStatus.cursor_engine_ready ? 'Ready' : 'Missing'}
                    detail={installationStatus.cursor_engine_ready ? undefined : (
                      <LedgerJump onClick={() => onConfigurePersona?.('engine')}>Open persona engine</LedgerJump>
                    )}
                  />
                  {installationStatus.needs_initialization ? (
                    <LedgerStatusRow
                      label="Setup"
                      state="Missing"
                      detail={<LedgerJump onClick={() => onRunSetupWizard?.()}>Run Setup Wizard</LedgerJump>}
                    />
                  ) : null}
                  <LedgerStatusRow
                    label="Restart"
                    state={restartOwed(installationStatus) ? 'Restart needed' : 'No restart owed'}
                  />
                </div>
                <p className="mc-notice-settled mt-2">
                  Stop requests between LLM/tool iterations (cooperative cancel). Use Queue for FIFO visibility.
                </p>
                <div className="mt-3 flex gap-2 flex-wrap">
                  <Button
                    size="sm"
                    variant="outline"
                    onClick={() => onRunSetupWizard?.()}
                  >
                    Run Setup Wizard
                  </Button>
                  <Button
                    size="sm"
                    variant="default"
                    disabled={restartBusy || !restartOwed(installationStatus)}
                    onClick={() => void requestRestart()}
                  >
                    {restartBusy
                      ? 'Restarting…'
                      : restartOwed(installationStatus)
                        ? 'Restart Meido'
                        : 'No restart owed'}
                  </Button>
                </div>
              </LedgerSection>
            ) : (
              <OverviewStatusSkeleton />
            )}
            <LedgerSection title="Runtime toggles" className="mt-3">
              <SettingsRuntimePanel api={api} onError={setSettingsError} />
            </LedgerSection>
            </>
          ) : null}
          {page === 'system-llm' ? (
            <SettingsTopicsModelPanel api={api} onError={setSettingsError} />
          ) : null}
          {page === 'connections' ? (
            <SettingsConnectionsPanel api={api} onError={setSettingsError} />
          ) : null}
          {page === 'integrations' ? (
            <SettingsIntegrationsPanel
              api={api}
              appearance={appearance}
              onError={setSettingsError}
              onSaved={() => void reloadInstallationStatus()}
              requestRestart={requestRestart}
              restartBusy={restartBusy}
            />
          ) : null}
          {page === 'channels' ? (
        <LedgerSection title="External channel persona mode" scope="live">
          <ServiceNote summary="Choose which personas answer this contact. The next message from that bot uses the choice.">
            Replies are directional: a channel only receives responses to messages it sent. The web UI still shows every message. Telegram, Discord, and WeCom can answer as all personas or one persona. WhatsApp stays on one persona. Focused sessions stay in the web UI.
          </ServiceNote>
          <div className="space-y-2">
            {bindings.length === 0 ? (
              <span>No bot integrations configured. Add bots under Integrations.</span>
            ) : bindings.map((b) => {
              const platform = b.platform ?? b.channel_type
              const currentMode = platform === 'whatsapp'
                ? 'single'
                : b.persona_mode === 'single' ? 'single' : 'all'
              const currentPersonaId = b.persona_id ?? activePersonaId ?? personas[0]?.id ?? null
              const handleLabel = b.linked && b.channel_handle
                ? b.channel_handle
                : 'pending link'
              const otherAllOnPlatform = bindings.some(
                (other) =>
                  other.bot_instance_id !== b.bot_instance_id &&
                  (other.platform ?? other.channel_type) === platform &&
                  other.persona_mode !== 'single',
              )
              const allPersonasDisabled = otherAllOnPlatform && currentMode !== 'all'
              return (
                <div className="flex gap-2 items-center flex-wrap" key={`${b.bot_instance_id}:${b.channel_type}:${b.channel_handle ?? 'pending'}`}>
                  <span className="min-w-[220px]">
                    {b.label ? `${b.label} · ` : ''}{platform} (bot #{b.bot_instance_id}): {handleLabel}
                  </span>
                  <Select
                    value={currentMode}
                    onValueChange={(mode) => {
                      if (platform === 'whatsapp') {
                        if (currentPersonaId != null) {
                          void updateChannelPersonaPolicy(b.bot_instance_id, 'single', currentPersonaId)
                        }
                        return
                      }
                      if (mode === 'all') {
                        if (allPersonasDisabled) {
                          setSettingsError(
                            `Only one ${platform} bot can use all personas for this contact. Lock the other bot to a single persona first.`,
                          )
                          return
                        }
                        void updateChannelPersonaPolicy(b.bot_instance_id, 'all')
                      } else if (currentPersonaId != null) {
                        void updateChannelPersonaPolicy(b.bot_instance_id, 'single', currentPersonaId)
                      }
                    }}
                  >
                    <SelectTrigger className="w-[140px]" ><SelectValue /></SelectTrigger>
                    <SelectContent>
                      {platform === 'whatsapp' ? null : (
                        <SelectItem value="all" disabled={allPersonasDisabled}>
                          All personas
                        </SelectItem>
                      )}
                      {platform === 'whatsapp' ? null : (
                        <SelectItem value="single">Single persona</SelectItem>
                      )}
                      {platform === 'whatsapp' ? (
                        <SelectItem value="single">Single persona</SelectItem>
                      ) : null}
                    </SelectContent>
                  </Select>
                  {currentMode === 'single' ? (
                    <Select
                      value={currentPersonaId != null ? String(currentPersonaId) : ''}
                      onValueChange={(value) => {
                        const pid = Number(value)
                        if (Number.isFinite(pid) && pid > 0) {
                          void updateChannelPersonaPolicy(b.bot_instance_id, 'single', pid)
                        }
                      }}
                    >
                      <SelectTrigger className="w-[180px]" placeholder="Persona" ><SelectValue /></SelectTrigger>
                      <SelectContent>
                        {personas.map((p) => (
                          <SelectItem key={p.id} value={String(p.id)}>
                            {p.name}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  ) : null}
                </div>
              )
            })}
          </div>
        </LedgerSection>
          ) : null}
          </React.Suspense>
        </LedgerBook>
        <div className="mc-ledger-footer">
          {personaName && onConfigurePersona ? (
            <LedgerJump onClick={() => onConfigurePersona()}>Configure {personaName}</LedgerJump>
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
