import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectTrigger, SelectContent, SelectItem, SelectValue } from '@/components/ui/select'
import { Tooltip } from '@/components/ui/tooltip'
import { useCallback, useEffect, useMemo, useState } from 'react'
import { SettingsPanelSkeleton } from './skeleton'
import { useConfirmDialog } from './confirm-dialog'
import type { BotInstanceRow, ChannelIntegrationSettings } from '../types'

type Platform = 'telegram' | 'discord' | 'whatsapp' | 'wecom'

type Props = {
  api: <T>(path: string, init?: RequestInit) => Promise<T>
  onError: (message: string) => void
  onSaved?: () => void
  requestRestart: () => Promise<void>
  restartBusy: boolean
  appearance: 'dark' | 'light'
}

type InstanceDraft = {
  label: string
  token: string
  botUsername: string
  allowedGroups: string
  discordAllowedChannels: string
  whatsappPhoneNumberId: string
  whatsappVerifyToken: string
  whatsappWebhookPort: string
  wecomCorpId: string
  wecomAgentId: string
  wecomCallbackToken: string
  wecomEncodingAesKey: string
  wecomWebhookPort: string
  wecomAllowedChats: string
  wecomAibotId: string
  wecomMode: string
}

function draftFromRow(row: BotInstanceRow): InstanceDraft {
  return {
    label: row.label ?? '',
    token: '',
    botUsername: row.bot_username ?? '',
    allowedGroups: row.allowed_groups ?? '',
    discordAllowedChannels: row.discord_allowed_channels ?? '',
    whatsappPhoneNumberId: row.whatsapp_phone_number_id ?? '',
    whatsappVerifyToken: '',
    whatsappWebhookPort: String(row.whatsapp_webhook_port ?? 8080),
    wecomCorpId: row.wecom_corp_id ?? '',
    wecomAgentId: row.wecom_agent_id != null ? String(row.wecom_agent_id) : '',
    wecomCallbackToken: '',
    wecomEncodingAesKey: '',
    wecomWebhookPort: String(row.wecom_webhook_port ?? 8081),
    wecomAllowedChats: row.wecom_allowed_chats ?? '',
    wecomAibotId: row.wecom_aibot_id ?? '',
    wecomMode: wecomModeFromRow(row),
  }
}

function wecomModeFromRow(row: BotInstanceRow): 'aibot' | 'callback' {
  const mode = (row.wecom_mode ?? '').trim().toLowerCase()
  if (mode === 'callback') return 'callback'
  if (mode === 'aibot' || mode === 'websocket' || mode === 'long_connection' || mode === 'long-connection') {
    return 'aibot'
  }
  if ((row.wecom_aibot_id ?? '').trim()) return 'aibot'
  if ((row.wecom_corp_id ?? '').trim()) return 'callback'
  return 'aibot'
}

function platformLabel(platform: string): string {
  switch (platform) {
    case 'telegram':
      return 'Telegram'
    case 'discord':
      return 'Discord'
    case 'whatsapp':
      return 'WhatsApp Cloud API'
    case 'wecom':
      return 'WeCom (企业微信)'
    default:
      return platform
  }
}

function defaultLabel(platform: Platform): string {
  switch (platform) {
    case 'telegram':
      return 'Telegram bot'
    case 'discord':
      return 'Discord bot'
    case 'whatsapp':
      return 'WhatsApp number'
    case 'wecom':
      return 'WeCom bot'
  }
}

function FieldLabel({ children }: { children: string }) {
  return (
    <span className="mb-1 block">
      {children}
    </span>
  )
}

type WecomExtraValues = {
  mode: 'aibot' | 'callback'
  aibotId: string
  corpId: string
  agentId: string
  callbackToken: string
  encodingAesKey: string
  webhookPort: string
  allowedChats: string
}

function WecomExtraFields({
  values,
  callbackTokenPlaceholder,
  encodingAesKeyPlaceholder,
  onChange,
}: {
  values: WecomExtraValues
  callbackTokenPlaceholder: string
  encodingAesKeyPlaceholder: string
  onChange: (patch: Partial<WecomExtraValues>) => void
}) {
  return (
    <>
      <div>
        <FieldLabel>Connection</FieldLabel>
        <Select
          value={values.mode}
          onValueChange={(value) => onChange({ mode: value === 'callback' ? 'callback' : 'aibot' })}
        >
          <SelectTrigger className="w-full" ><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem value="aibot">AI Bot long connection (no callback URL)</SelectItem>
            <SelectItem value="callback">Self-built app callback</SelectItem>
          </SelectContent>
        </Select>
      </div>
      {values.mode === 'aibot' ? (
        <div>
          <FieldLabel>Bot ID</FieldLabel>
          <Input
            placeholder="AI Bot ID from WeCom admin"
            value={values.aibotId}
            onChange={(e) => onChange({ aibotId: e.target.value })}
          />
        </div>
      ) : (
        <>
          <div>
            <FieldLabel>Corp ID</FieldLabel>
            <Input
              placeholder="wwxxxxxxxx"
              value={values.corpId}
              onChange={(e) => onChange({ corpId: e.target.value })}
            />
          </div>
          <div>
            <FieldLabel>Agent ID</FieldLabel>
            <Input
              placeholder="1000002"
              value={values.agentId}
              onChange={(e) => onChange({ agentId: e.target.value })}
            />
          </div>
          <div>
            <FieldLabel>Callback token</FieldLabel>
            <Input
              type="password"
              placeholder={callbackTokenPlaceholder}
              value={values.callbackToken}
              onChange={(e) => onChange({ callbackToken: e.target.value })}
              autoComplete="off"
            />
          </div>
          <div>
            <FieldLabel>EncodingAESKey</FieldLabel>
            <Input
              type="password"
              placeholder={encodingAesKeyPlaceholder}
              value={values.encodingAesKey}
              onChange={(e) => onChange({ encodingAesKey: e.target.value })}
              autoComplete="off"
            />
          </div>
          <div>
            <FieldLabel>Callback port</FieldLabel>
            <Input
              placeholder="8081"
              value={values.webhookPort}
              onChange={(e) => onChange({ webhookPort: e.target.value })}
            />
          </div>
        </>
      )}
      <div>
        <FieldLabel>Allowed chat IDs (WeCom chatid or userid, not the group name)</FieldLabel>
        <Input
          placeholder="Comma-separated; empty = all; applies on save"
          value={values.allowedChats}
          onChange={(e) => onChange({ allowedChats: e.target.value })}
        />
      </div>
    </>
  )
}

function emptyWecomExtra(port = '8081'): WecomExtraValues {
  return {
    mode: 'aibot',
    aibotId: '',
    corpId: '',
    agentId: '',
    callbackToken: '',
    encodingAesKey: '',
    webhookPort: port,
    allowedChats: '',
  }
}

function integrationDescription(row: BotInstanceRow): string {
  if (row.platform === 'telegram') {
    return 'Bot token, @mention username, and optional group allowlist.'
  }
  if (row.platform === 'discord') {
    return 'Bot token and optional Discord channel allowlist.'
  }
  if (row.platform === 'whatsapp') {
    return 'Single supported WhatsApp Business number. Persona routing lives on the Channels tab.'
  }
  if (row.platform === 'wecom') {
    return 'Single WeCom 智能机器人 (long connection) or self-built app. Long connection needs Bot ID + Secret only — no public callback. Restart after save.'
  }
  return 'External chat bot integration.'
}

export function SettingsIntegrationsPanel({
  api,
  onError,
  onSaved,
  requestRestart,
  restartBusy,
  appearance,
}: Props) {
  const [loading, setLoading] = useState(true)
  const [busyId, setBusyId] = useState<number | 'new' | 'shared' | null>(null)
  const { requestConfirm, confirmDialog } = useConfirmDialog()
  const [notice, setNotice] = useState<string | null>(null)
  const [cfg, setCfg] = useState<ChannelIntegrationSettings | null>(null)
  const [instances, setInstances] = useState<BotInstanceRow[]>([])
  const [drafts, setDrafts] = useState<Record<number, InstanceDraft>>({})
  const [controlChatIds, setControlChatIds] = useState('')
  const [newPlatform, setNewPlatform] = useState<Platform>('telegram')
  const [newLabel, setNewLabel] = useState('')
  const [newToken, setNewToken] = useState('')
  const [newWecom, setNewWecom] = useState<WecomExtraValues>(() => emptyWecomExtra())

  const panelStyle =
    appearance === 'dark'
      ? { borderColor: 'var(--mc-border-soft)', background: 'var(--mc-bg-panel)' }
      : { borderColor: 'var(--gray-6)', background: 'var(--gray-2)' }

  const applyCfg = useCallback((data: ChannelIntegrationSettings) => {
    const rows = Array.isArray(data.instances) ? data.instances : []
    setCfg(data)
    setInstances(rows)
    setControlChatIds(data.control_chat_ids ?? '')
    setDrafts(Object.fromEntries(rows.map((row) => [row.id, draftFromRow(row)])))
    if (rows.some((row) => row.platform === 'whatsapp') && newPlatform === 'whatsapp') {
      setNewPlatform('telegram')
    }
    if (rows.some((row) => row.platform === 'wecom') && newPlatform === 'wecom') {
      setNewPlatform('telegram')
    }
  }, [newPlatform])

  const load = useCallback(async () => {
    setLoading(true)
    setNotice(null)
    try {
      const data = await api<ChannelIntegrationSettings>('/api/channels/integration')
      applyCfg(data)
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [api, applyCfg, onError])

  useEffect(() => {
    void load()
  }, [load])

  const whatsappExists = useMemo(
    () => instances.some((row) => row.platform === 'whatsapp'),
    [instances],
  )
  const wecomExists = useMemo(
    () => instances.some((row) => row.platform === 'wecom'),
    [instances],
  )

  function updateDraft(id: number, patch: Partial<InstanceDraft>): void {
    setDrafts((current) => ({
      ...current,
      [id]: { ...(current[id] ?? draftFromRow(instances.find((row) => row.id === id)!)), ...patch },
    }))
  }

  async function saveShared(): Promise<void> {
    setBusyId('shared')
    setNotice(null)
    onError('')
    try {
      const data = await api<ChannelIntegrationSettings>('/api/channels/integration', {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ control_chat_ids: controlChatIds }),
      })
      applyCfg(data)
      setNotice(data.message ?? 'Shared access saved.')
      onSaved?.()
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusyId(null)
    }
  }

  async function saveInstance(row: BotInstanceRow): Promise<void> {
    const draft = drafts[row.id] ?? draftFromRow(row)
    if (!draft.label.trim()) {
      onError('Label is required.')
      return
    }
    setBusyId(row.id)
    setNotice(null)
    onError('')
    try {
      const body: Record<string, unknown> = {
        label: draft.label.trim(),
      }
      if (draft.token.trim()) body.token = draft.token.trim()
      if (row.platform === 'telegram') {
        body.bot_username = draft.botUsername.trim()
        body.allowed_groups = draft.allowedGroups.trim()
      }
      if (row.platform === 'discord') {
        body.discord_allowed_channels = draft.discordAllowedChannels.trim()
      }
      if (row.platform === 'whatsapp') {
        body.whatsapp_phone_number_id = draft.whatsappPhoneNumberId.trim()
        body.whatsapp_webhook_port = Number.parseInt(draft.whatsappWebhookPort, 10) || 8080
        if (draft.whatsappVerifyToken.trim()) {
          body.whatsapp_verify_token = draft.whatsappVerifyToken.trim()
        }
      }
      if (row.platform === 'wecom') {
        body.wecom_mode = draft.wecomMode === 'callback' ? 'callback' : 'aibot'
        body.wecom_aibot_id = draft.wecomAibotId.trim()
        body.wecom_corp_id = draft.wecomCorpId.trim()
        body.wecom_agent_id = Number.parseInt(draft.wecomAgentId, 10) || 0
        body.wecom_webhook_port = Number.parseInt(draft.wecomWebhookPort, 10) || 8081
        body.wecom_allowed_chats = draft.wecomAllowedChats.trim()
        if (draft.wecomCallbackToken.trim()) {
          body.wecom_callback_token = draft.wecomCallbackToken.trim()
        }
        if (draft.wecomEncodingAesKey.trim()) {
          body.wecom_encoding_aes_key = draft.wecomEncodingAesKey.trim()
        }
      }
      await api(`/api/channel_bot_instances/${row.id}`, {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      })
      setNotice(
        'Integration saved. Group/chat allowlists apply immediately. Restart the gateway only for token or connection changes.',
      )
      await load()
      onSaved?.()
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusyId(null)
    }
  }

  async function addBotInstance(): Promise<void> {
    const label = newLabel.trim() || defaultLabel(newPlatform)
    const token = newToken.trim()
    if (!token) {
      onError(
        newPlatform === 'wecom'
          ? newWecom.mode === 'aibot'
            ? 'AI Bot secret is required.'
            : 'Corp secret is required.'
          : 'Token is required.',
      )
      return
    }
    if (newPlatform === 'wecom') {
      if (newWecom.mode === 'aibot') {
        if (!newWecom.aibotId.trim()) {
          onError('Bot ID is required.')
          return
        }
      } else {
        if (!newWecom.corpId.trim()) {
          onError('Corp ID is required.')
          return
        }
        if ((Number.parseInt(newWecom.agentId, 10) || 0) <= 0) {
          onError('Agent ID is required.')
          return
        }
        if (!newWecom.callbackToken.trim()) {
          onError('Callback token is required.')
          return
        }
        if (!newWecom.encodingAesKey.trim()) {
          onError('EncodingAESKey is required.')
          return
        }
      }
    }
    setBusyId('new')
    setNotice(null)
    onError('')
    try {
      const body: Record<string, unknown> = { platform: newPlatform, label, token }
      if (newPlatform === 'wecom') {
        body.wecom_mode = newWecom.mode
        body.wecom_aibot_id = newWecom.aibotId.trim()
        body.wecom_corp_id = newWecom.corpId.trim()
        body.wecom_agent_id = Number.parseInt(newWecom.agentId, 10) || 0
        body.wecom_callback_token = newWecom.callbackToken.trim()
        body.wecom_encoding_aes_key = newWecom.encodingAesKey.trim()
        body.wecom_webhook_port = Number.parseInt(newWecom.webhookPort, 10) || 8081
        body.wecom_allowed_chats = newWecom.allowedChats.trim()
      }
      const data = await api<{ message?: string }>('/api/channel_bot_instances', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      })
      setNewLabel('')
      setNewToken('')
      setNewWecom(emptyWecomExtra())
      setNotice(data.message ?? 'Bot instance created. Restart the gateway to activate it.')
      await load()
      onSaved?.()
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusyId(null)
    }
  }

  function removeBotInstance(row: BotInstanceRow): void {
    requestConfirm({
      title: 'Delete bot instance',
      description: `Delete ${row.label || platformLabel(row.platform)}? Related channel bindings will be removed.`,
      confirmLabel: 'Delete',
      destructive: true,
      onConfirm: async () => {
        setBusyId(row.id)
        onError('')
        try {
          await api(`/api/channel_bot_instances/${row.id}`, { method: 'DELETE' })
          setNotice('Bot instance removed. Restart the gateway to stop its dispatcher.')
          await load()
          onSaved?.()
        } catch (e) {
          onError(e instanceof Error ? e.message : String(e))
        } finally {
          setBusyId(null)
        }
      },
    })
  }

  if (loading || !cfg) {
    return <SettingsPanelSkeleton />
  }

  return (
    <div className="space-y-3">
      {notice ? (
        <div role="status" className="rounded-lg border border-green-500/30 bg-green-500/10 p-3 text-sm text-green-900 dark:text-green-100">{notice}</div>
      ) : null}

      <div role="status" className="rounded-lg border border-border bg-muted/50 p-3 text-sm">
          Integrations are bot credentials and platform access controls. Persona routing for a
          contact lives on the Channels tab.
        </div>

      {instances.length === 0 ? (
        <div className="rounded-md border p-3" style={panelStyle}>
          <span>
            No bot instances configured yet.
          </span>
        </div>
      ) : null}

      {instances.map((row) => {
        const draft = drafts[row.id] ?? draftFromRow(row)
        const tokenLabel = row.token_set ? `Current token: ${row.token_redacted}` : 'No token set'
        return (
          <div key={row.id} className="rounded-md border p-3" style={panelStyle}>
            <div className="flex justify-between gap-2 flex-wrap items-start mb-2">
              <div>
                <span className="font-semibold block">
                  {platformLabel(row.platform)} #{row.id}
                  {row.is_primary ? ' · primary' : ''}
                </span>
                <span>
                  {integrationDescription(row)}
                </span>
              </div>
              <span>
                {tokenLabel}
              </span>
            </div>
            <div className="flex flex-col gap-2">
              <Input
                placeholder="Label"
                value={draft.label}
                onChange={(e) => updateDraft(row.id, { label: e.target.value })}
              />
              <div>
                <FieldLabel>
                  {row.platform === 'wecom'
                    ? draft.wecomMode === 'callback'
                      ? 'Corp secret'
                      : 'AI Bot secret'
                    : 'Token'}
                </FieldLabel>
                <Input
                  type="password"
                  placeholder={
                    row.platform === 'wecom'
                      ? row.token_set
                        ? 'New secret (optional)'
                        : draft.wecomMode === 'callback'
                          ? 'Corp secret'
                          : 'AI Bot secret'
                      : row.token_set
                        ? 'New token (optional)'
                        : 'Bot token'
                  }
                  value={draft.token}
                  onChange={(e) => updateDraft(row.id, { token: e.target.value })}
                  autoComplete="off"
                />
              </div>
              {row.platform === 'telegram' ? (
                <>
                  <div>
                    <FieldLabel>Bot username for @mentions (without @)</FieldLabel>
                    <Input
                      placeholder="example_bot"
                      value={draft.botUsername}
                      onChange={(e) => updateDraft(row.id, { botUsername: e.target.value })}
                    />
                  </div>
                  <div>
                    <FieldLabel>Allowed Telegram group IDs (numeric; empty = all)</FieldLabel>
                    <Input
                      placeholder="-1001234567890, -1009876543210"
                      value={draft.allowedGroups}
                      onChange={(e) => updateDraft(row.id, { allowedGroups: e.target.value })}
                    />
                  </div>
                </>
              ) : null}
              {row.platform === 'discord' ? (
                <Input
                  placeholder="Allowed Discord channel IDs (comma-separated; empty = all)"
                  value={draft.discordAllowedChannels}
                  onChange={(e) => updateDraft(row.id, { discordAllowedChannels: e.target.value })}
                />
              ) : null}
              {row.platform === 'whatsapp' ? (
                <>
                  <Input
                    placeholder="Phone number ID"
                    value={draft.whatsappPhoneNumberId}
                    onChange={(e) => updateDraft(row.id, { whatsappPhoneNumberId: e.target.value })}
                  />
                  <Input
                    type="password"
                    placeholder={
                      row.whatsapp_verify_token_set
                        ? `New verify token (optional; current ${row.whatsapp_verify_token_redacted})`
                        : 'Verify token'
                    }
                    value={draft.whatsappVerifyToken}
                    onChange={(e) => updateDraft(row.id, { whatsappVerifyToken: e.target.value })}
                    autoComplete="off"
                  />
                  <Input
                    placeholder="Webhook port"
                    value={draft.whatsappWebhookPort}
                    onChange={(e) => updateDraft(row.id, { whatsappWebhookPort: e.target.value })}
                  />
                </>
              ) : null}
              {row.platform === 'wecom' ? (
                <WecomExtraFields
                  values={{
                    mode: draft.wecomMode === 'callback' ? 'callback' : 'aibot',
                    aibotId: draft.wecomAibotId,
                    corpId: draft.wecomCorpId,
                    agentId: draft.wecomAgentId,
                    callbackToken: draft.wecomCallbackToken,
                    encodingAesKey: draft.wecomEncodingAesKey,
                    webhookPort: draft.wecomWebhookPort,
                    allowedChats: draft.wecomAllowedChats,
                  }}
                  callbackTokenPlaceholder={
                    row.wecom_callback_token_set
                      ? `New callback token (optional; current ${row.wecom_callback_token_redacted})`
                      : 'Receive-server Token'
                  }
                  encodingAesKeyPlaceholder={
                    row.wecom_encoding_aes_key_set
                      ? `New EncodingAESKey (optional; current ${row.wecom_encoding_aes_key_redacted})`
                      : '43-character EncodingAESKey'
                  }
                  onChange={(patch) =>
                    updateDraft(row.id, {
                      ...(patch.mode != null ? { wecomMode: patch.mode } : {}),
                      ...(patch.aibotId != null ? { wecomAibotId: patch.aibotId } : {}),
                      ...(patch.corpId != null ? { wecomCorpId: patch.corpId } : {}),
                      ...(patch.agentId != null ? { wecomAgentId: patch.agentId } : {}),
                      ...(patch.callbackToken != null ? { wecomCallbackToken: patch.callbackToken } : {}),
                      ...(patch.encodingAesKey != null ? { wecomEncodingAesKey: patch.encodingAesKey } : {}),
                      ...(patch.webhookPort != null ? { wecomWebhookPort: patch.webhookPort } : {}),
                      ...(patch.allowedChats != null ? { wecomAllowedChats: patch.allowedChats } : {}),
                    })
                  }
                />
              ) : null}
            </div>
            <div className="flex gap-2 mt-3 flex-wrap">
              <Button size="sm" disabled={busyId === row.id} onClick={() => void saveInstance(row)}>
                {busyId === row.id ? 'Saving…' : 'Save'}
              </Button>
              <Button
                size="sm"
                variant="destructive"
                disabled={busyId === row.id}
                onClick={() => removeBotInstance(row)}
              >
                Delete
              </Button>
            </div>
          </div>
        )
      })}

      <div className="rounded-md border p-3" style={panelStyle}>
        <span className="font-semibold mb-1 block">
          Add Bot Instance
        </span>
        <span className="mb-2 block">
          Telegram and Discord can have multiple bot instances. WhatsApp and WeCom support one
          app/number in this gateway.
        </span>
        <div className="flex flex-col gap-2">
          <div className="flex gap-2 flex-wrap items-end">
            <div>
              <FieldLabel>Platform</FieldLabel>
              <Select
                value={newPlatform}
                onValueChange={(value) =>
                  setNewPlatform(
                    value === 'discord'
                      ? 'discord'
                      : value === 'whatsapp'
                        ? 'whatsapp'
                        : value === 'wecom'
                          ? 'wecom'
                          : 'telegram',
                  )
                }
              >
                <SelectTrigger className="w-[150px]" ><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="telegram">telegram</SelectItem>
                  <SelectItem value="discord">discord</SelectItem>
                  <SelectItem value="whatsapp" disabled={whatsappExists}>
                    whatsapp
                  </SelectItem>
                  <SelectItem value="wecom" disabled={wecomExists}>
                    wecom
                  </SelectItem>
                </SelectContent>
              </Select>
            </div>
            <div className="min-w-[160px] flex-1">
              <FieldLabel>Label</FieldLabel>
              <Input
                placeholder={defaultLabel(newPlatform)}
                value={newLabel}
                onChange={(e) => setNewLabel(e.target.value)}
              />
            </div>
            <div className="min-w-[220px] flex-1">
              <FieldLabel>
                {newPlatform === 'wecom'
                  ? newWecom.mode === 'callback'
                    ? 'Corp secret'
                    : 'AI Bot secret'
                  : newPlatform === 'whatsapp'
                    ? 'Access token'
                    : 'Bot token'}
              </FieldLabel>
              <Input
                type="password"
                placeholder={
                  newPlatform === 'wecom'
                    ? newWecom.mode === 'callback'
                      ? 'Corp secret'
                      : 'Long-connection Secret'
                    : newPlatform === 'whatsapp'
                      ? 'Access token'
                      : 'Bot token'
                }
                value={newToken}
                onChange={(e) => setNewToken(e.target.value)}
                autoComplete="off"
              />
            </div>
            {newPlatform !== 'wecom' ? (
              <Button size="sm" disabled={busyId === 'new'} onClick={() => void addBotInstance()}>
                {busyId === 'new' ? 'Adding…' : 'Add'}
              </Button>
            ) : null}
          </div>
          {newPlatform === 'wecom' ? (
            <>
              <span>
                {newWecom.mode === 'aibot'
                  ? 'Create a 智能机器人 in WeCom, enable API mode → 长连接, then paste Bot ID and Secret. No HTTPS callback is required.'
                  : 'Receive-server URL after restart: https://your-host/callback (HTTPS). Token and EncodingAESKey must match WeCom admin.'}
              </span>
              <WecomExtraFields
                values={newWecom}
                callbackTokenPlaceholder="Receive-server Token"
                encodingAesKeyPlaceholder="43-character EncodingAESKey"
                onChange={(patch) => setNewWecom((current) => ({ ...current, ...patch }))}
              />
              <div>
                <Button size="sm" disabled={busyId === 'new'} onClick={() => void addBotInstance()}>
                  {busyId === 'new' ? 'Adding…' : 'Add'}
                </Button>
              </div>
            </>
          ) : null}
        </div>
      </div>

      <div className="rounded-md border p-3" style={panelStyle}>
        <div className="flex items-center gap-1 mb-1">
          <span className="font-semibold">
            Shared Access
          </span>
          <Tooltip
            maxWidth="320px"
            side="top"
            content={
              <>
                Chats listed here are control chats. From them, the agent can use cross-chat tools
                (for example send_message, schedule tasks, or read another chat&apos;s history).
                <br />
                <br />
                Enter comma-separated chat IDs (Telegram group/user IDs, Discord channel IDs, etc.).
                This setting is global across all integrations. Restart the gateway after saving.
              </>
            }
          >
            <Button
              size="sm"
              variant="outline"
              type="button"
              aria-label="About shared access"
            >
              <svg
                className="size-3.5 shrink-0"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="2"
                strokeLinecap="round"
                strokeLinejoin="round"
                aria-hidden
              >
                <circle cx="12" cy="12" r="10" />
                <path d="M12 16v-4" />
                <path d="M12 8h.01" />
              </svg>
            </Button>
          </Tooltip>
        </div>
        <span className="mb-2 block">
          Privileged chat IDs for cross-chat tools. This is global, not platform-specific.
        </span>
        <div className="flex gap-2 flex-wrap items-center">
          <Input
            className="min-w-[260px] flex-1"
            placeholder="Control chat IDs"
            value={controlChatIds}
            onChange={(e) => setControlChatIds(e.target.value)}
          />
          <Button size="sm" disabled={busyId === 'shared'} onClick={() => void saveShared()}>
            {busyId === 'shared' ? 'Saving…' : 'Save Shared Access'}
          </Button>
          <Button size="sm" variant="secondary" disabled={restartBusy} onClick={() => void requestRestart()}>
            {restartBusy ? 'Restarting…' : 'Restart Gateway'}
          </Button>
        </div>
      </div>
      {confirmDialog}
    </div>
  )
}
