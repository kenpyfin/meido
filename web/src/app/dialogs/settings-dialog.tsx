import { Button } from '@/components/ui/button'
import { Select, SelectTrigger, SelectContent, SelectItem, SelectValue } from '@/components/ui/select'
import { Dialog, DialogContent, DialogTitle, DialogDescription, DialogClose } from '@/components/ui/dialog'
import { Tabs, TabsList, TabsTrigger, TabsContent } from '@/components/ui/tabs'
import React from 'react'

import { SettingsPanelSkeleton, OverviewStatusSkeleton } from '../../components/skeleton'
import type { SettingsDialogProps } from './types'

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
const SettingsLlmPanel = React.lazy(() =>
  import('../../components/settings-llm').then((m) => ({ default: m.SettingsLlmPanel })),
)
const SettingsLocalDelegatePanel = React.lazy(() =>
  import('../../components/settings-local-delegate').then((m) => ({ default: m.SettingsLocalDelegatePanel })),
)
const SettingsCursorPanel = React.lazy(() =>
  import('../../components/settings-cursor').then((m) => ({ default: m.SettingsCursorPanel })),
)
const SettingsGeminiAdkPanel = React.lazy(() =>
  import('../../components/settings-gemini-adk').then((m) => ({ default: m.SettingsGeminiAdkPanel })),
)

export function SettingsDialog({ appearance, api, activePersonaId, personas, settings }: SettingsDialogProps) {
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

  return (
    <Dialog open={settingsDialogOpen} onOpenChange={setSettingsDialogOpen}>
      <DialogContent className="max-w-[920px]">
        <DialogTitle>Web UI configuration</DialogTitle>
        <DialogDescription className="mb-3">
          Shared setup for every persona. Model and engine API keys can live in repo-root{' '}
          <code className="text-xs">.env</code>, or under Connections when they are not in{' '}
          <code className="text-xs">.env</code>. A persona’s engine, hooks, skills, and secrets are on
          the Persona page. Configure Telegram, Discord, and WhatsApp under Integrations. Restart
          Meido after changing channel tokens.
        </DialogDescription>
        {settingsError ? (
          <div role="status" className="rounded-lg border border-destructive/30 bg-destructive/10 p-3 text-sm text-destructive mb-2">{settingsError}</div>
        ) : null}
        {restartNotice ? (
          <div role="status" className="rounded-lg border border-green-500/30 bg-green-500/10 p-3 text-sm text-green-900 dark:text-green-100 mb-2">{restartNotice}</div>
        ) : null}
        <Tabs defaultValue="overview">
          <TabsList className="mc-settings-tabs-sticky mb-3 h-auto flex-wrap">
            <TabsTrigger value="overview">Overview</TabsTrigger>
            <TabsTrigger value="engine">Engine</TabsTrigger>
            <TabsTrigger value="system-llm">System LLM</TabsTrigger>
            <TabsTrigger value="connections">Connections</TabsTrigger>
            <TabsTrigger value="integrations">Integrations</TabsTrigger>
            <TabsTrigger value="channels">Channels</TabsTrigger>
          </TabsList>
          <React.Suspense fallback={<SettingsPanelSkeleton />}>
          <TabsContent value="overview">
            {installationStatus ? (
              <div className="flex flex-col gap-2 mb-2">
                <div className="flex gap-2 flex-wrap items-center">
                  <span>
                    LLM: {installationStatus.llm_ready ? 'ready' : 'missing'}
                  </span>
                  <span>
                    Channels: {installationStatus.channel_ready ? 'ready' : 'missing'}
                  </span>
                  <span>
                    Cursor engine:{' '}
                    {installationStatus.cursor_engine_ready ? 'ready' : 'not ready'}
                  </span>
                  {installationStatus.agent_engine === 'classic_cost_routing' ? (
                    <span>
                      Local delegate:{' '}
                      {installationStatus.local_delegate_ready ? 'verified' : 'not verified'}
                    </span>
                  ) : null}
                  <span>
                    Env restart needed:{' '}
                    {(installationStatus.requires_restart_for_env_changes ??
                      installationStatus.requires_restart_to_apply_runtime_settings) === true
                      ? 'yes'
                      : 'no'}
                  </span>
                </div>
                <span>
                  Stop requests between LLM/tool iterations (cooperative cancel). Use Queue for FIFO visibility.
                </span>
                <div>
                  <Button
                    size="sm"
                    variant="default"
                    disabled={restartBusy}
                    onClick={() => void requestRestart()}
                  >
                    {restartBusy ? 'Restarting…' : 'Restart Meido'}
                  </Button>
                </div>
              </div>
            ) : (
              <OverviewStatusSkeleton />
            )}
            <div
              className="rounded-md border p-3 mt-3"
              style={appearance === 'dark'
                ? { borderColor: 'var(--mc-border-soft)', background: 'var(--mc-bg-panel)' }
                : { borderColor: 'var(--gray-6)', background: 'var(--gray-2)' }}
            >
              <span className="font-semibold mb-2 block">
                Runtime toggles
              </span>
              <SettingsRuntimePanel api={api} onError={setSettingsError} />
            </div>
          </TabsContent>
          <TabsContent value="engine">
            <div className="flex flex-col gap-4">
              <span>
                Defaults shared by every persona. Choose a persona’s engine, model override, Cursor
                model, and Gemini topology on the Persona page.
              </span>
              <SettingsLlmPanel api={api} onError={setSettingsError} />
              <SettingsLocalDelegatePanel api={api} onError={setSettingsError} />
              <SettingsCursorPanel api={api} onError={setSettingsError} scope="shared" />
              <SettingsGeminiAdkPanel
                api={api}
                onError={setSettingsError}
                activePersonaId={null}
                scope="engine"
              />
            </div>
          </TabsContent>
          <TabsContent value="system-llm">
            <SettingsTopicsModelPanel api={api} onError={setSettingsError} />
          </TabsContent>
          <TabsContent value="connections">
            <SettingsConnectionsPanel api={api} onError={setSettingsError} />
          </TabsContent>
          <TabsContent value="integrations">
            <SettingsIntegrationsPanel
              api={api}
              appearance={appearance}
              onError={setSettingsError}
              onSaved={() => void reloadInstallationStatus()}
              requestRestart={requestRestart}
              restartBusy={restartBusy}
            />
          </TabsContent>
          <TabsContent value="channels">
        <div
          className="rounded-md border p-3"
          style={appearance === 'dark'
            ? { borderColor: 'var(--mc-border-soft)', background: 'var(--mc-bg-panel)' }
            : { borderColor: 'var(--gray-6)', background: 'var(--gray-2)' }}
        >
          <span className="font-semibold">External channel persona mode</span>
          <span className="mb-2 block">
            Bot integrations appear here for this contact’s main chat. Replies are directional: each channel only receives responses to messages it sent. All traffic still appears in the web UI. Telegram, Discord, and WeCom can use all personas or a single persona. WhatsApp is single-persona by design. Focused sessions stay in the web UI.
          </span>
          <div className="space-y-2">
            {bindings.length === 0 ? (
              <span>No bot integrations configured. Add bots under the Integrations tab.</span>
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
        </div>
          </TabsContent>
          </React.Suspense>
        </Tabs>
        <div className="flex justify-end mt-4">
          <DialogClose>
            <Button variant="secondary">Close</Button>
          </DialogClose>
        </div>
      </DialogContent>
    </Dialog>
  )
}
