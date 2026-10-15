import { Switch } from '@/components/ui/switch'
import { Select, SelectTrigger, SelectContent, SelectItem, SelectValue } from '@/components/ui/select'
import { useCallback, useEffect, useMemo, useState } from 'react'
import { parsePersonaBulletinHistorySuffix } from '../lib/bulletin'
import type { PersonaBulletinHistorySuffix, PersonaDenseDeliveryInfo } from '../types'

type Props = {
  api: <T>(path: string, init?: RequestInit) => Promise<T>
  onError: (message: string) => void
  activePersonaId: number | null
  personaName: string
  /** Called after a successful PATCH so the app can reload bulletin-derived state. */
  onPersonaBulletinChanged?: () => void | Promise<void>
}

function historyDepthSelectValue(hs: PersonaBulletinHistorySuffix | null): string {
  if (hs == null) return '6'
  const u = hs.min_user.effective
  const a = hs.min_assistant.effective
  if (u === a && (u === 2 || u === 6 || u === 10)) return String(u)
  return 'custom'
}

export function SettingsRunContextPanel({
  api,
  onError,
  activePersonaId,
  personaName,
  onPersonaBulletinChanged,
}: Props) {
  const [historySuffix, setHistorySuffix] = useState<PersonaBulletinHistorySuffix | null>(null)
  const [denseDelivery, setDenseDelivery] = useState<PersonaDenseDeliveryInfo | null>(null)
  const [depthBusy, setDepthBusy] = useState(false)
  const [deliveryBusy, setDeliveryBusy] = useState(false)
  const [depthError, setDepthError] = useState('')
  const [deliveryError, setDeliveryError] = useState('')
  const [saveNotice, setSaveNotice] = useState<string | null>(null)

  const load = useCallback(async () => {
    if (activePersonaId == null) {
      setHistorySuffix(null)
      setDenseDelivery(null)
      return
    }
    try {
      const data = await api<{
        history_suffix?: unknown
        dense_delivery?: PersonaDenseDeliveryInfo | null
        dense_delivery_enabled?: boolean
      }>(`/api/personas/${activePersonaId}/bulletin`)
      setHistorySuffix(parsePersonaBulletinHistorySuffix(data.history_suffix))
      if (data.dense_delivery && typeof data.dense_delivery.enabled === 'boolean') {
        setDenseDelivery(data.dense_delivery)
      } else {
        setDenseDelivery({
          enabled: data.dense_delivery_enabled === true,
          messaging_max_chars: 2000,
          web_max_chars: 1000,
          summary_chars: 800,
        })
      }
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
      setHistorySuffix(null)
      setDenseDelivery(null)
    }
  }, [activePersonaId, api, onError])

  useEffect(() => {
    void load()
  }, [load])

  const depthSelectValue = useMemo(() => historyDepthSelectValue(historySuffix), [historySuffix])
  const disabled = activePersonaId == null

  const applyDepthPreset = useCallback(
    async (v: string) => {
      if (activePersonaId == null || v === 'custom') return
      setDepthBusy(true)
      setDepthError('')
      setSaveNotice(null)
      try {
        await api(`/api/personas/${activePersonaId}/bulletin`, {
          method: 'PATCH',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            recent_history_min_user: Number(v),
            recent_history_min_assistant: Number(v),
          }),
        })
        await load()
        await onPersonaBulletinChanged?.()
        setSaveNotice('Chat context depth updated.')
      } catch (e) {
        setDepthError(e instanceof Error ? e.message : String(e))
      } finally {
        setDepthBusy(false)
      }
    },
    [activePersonaId, api, load, onPersonaBulletinChanged],
  )

  const applyDenseDelivery = useCallback(
    async (enabled: boolean) => {
      if (activePersonaId == null) return
      setDeliveryBusy(true)
      setDeliveryError('')
      setSaveNotice(null)
      try {
        await api(`/api/personas/${activePersonaId}/bulletin`, {
          method: 'PATCH',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ dense_delivery_enabled: enabled }),
        })
        await load()
        await onPersonaBulletinChanged?.()
        setSaveNotice(enabled ? 'Dense delivery on.' : 'Dense delivery off.')
      } catch (e) {
        setDeliveryError(e instanceof Error ? e.message : String(e))
      } finally {
        setDeliveryBusy(false)
      }
    },
    [activePersonaId, api, load, onPersonaBulletinChanged],
  )

  return (
    <div className="flex flex-col gap-3 mc-run-context">
      <div className="flex flex-col gap-1">
        <span className="font-medium">
          Run context{personaName ? ` for ${personaName}` : ''}
        </span>
        {disabled ? (
          <span>
            Select a persona in the sidebar to edit chat depth and dense delivery.
          </span>
        ) : (
          <span>
            Persona-scoped knobs applied on the next agent run. Moved here from the session cockpit.
          </span>
        )}
      </div>

      {saveNotice ? (
        <div role="status" className="rounded-lg border border-green-500/30 bg-green-500/10 p-3 text-sm text-green-900 dark:text-green-100">{saveNotice}</div>
      ) : null}

      <div className="mc-run-context-block">
        <span className="font-medium">
          Chat context depth
        </span>
        <span className="mt-1 block leading-snug">
          Minimum user and assistant dialogue turns kept at the tail of each run. Set{' '}
          <code className="text-[11px]">MAX_HISTORY_MESSAGES</code> ≥ user + assistant mins when turns
          alternate.
        </span>
        {historySuffix ? (
          <div className="flex mt-2 flex-col gap-1 mc-run-context-control">
            <Select
              value={depthSelectValue}
              onValueChange={(v) => void applyDepthPreset(v)}
              disabled={disabled || depthBusy}
            >
              <SelectTrigger className="w-full" ><SelectValue /></SelectTrigger>
              <SelectContent position="popper">
                <SelectItem value="2">Compact (2 / 2)</SelectItem>
                <SelectItem value="6">Standard (6 / 6)</SelectItem>
                <SelectItem value="10">Deep (10 / 10)</SelectItem>
                {depthSelectValue === 'custom' ? (
                  <SelectItem value="custom">
                    Custom ({historySuffix.min_user.effective} / {historySuffix.min_assistant.effective})
                  </SelectItem>
                ) : null}
              </SelectContent>
            </Select>
            {depthError ? (
              <span>
                {depthError}
              </span>
            ) : (
              <span>
                Effective: {historySuffix.min_user.effective} user, {historySuffix.min_assistant.effective}{' '}
                assistant
                {historySuffix.min_user.persona_override != null ||
                historySuffix.min_assistant.persona_override != null
                  ? ' (persona override)'
                  : ''}
              </span>
            )}
          </div>
        ) : (
          <span className="mt-1">
            {disabled ? 'Select a persona to load depth.' : 'Loading depth…'}
          </span>
        )}
      </div>

      <div className="mc-run-context-block mc-run-context-delivery">
        <div className="flex flex-col gap-1 min-w-0 flex-1">
          <span className="font-medium">
            Dense delivery (short message + PDF link)
          </span>
          <span className="leading-snug">
            Spill over the channel cap to PDF, upload to catbox, then send a summary with a public HTTPS
            URL. Needed for WeCom/Telegram long reports.
          </span>
          {denseDelivery?.enabled ? (
            <span>
              Caps: messaging {denseDelivery.messaging_max_chars} chars, web {denseDelivery.web_max_chars}{' '}
              chars
            </span>
          ) : null}
          {deliveryError ? (
            <span>
              {deliveryError}
            </span>
          ) : null}
        </div>
        <div className="mc-run-context-switch-row">
          <Switch
            size="default"
            checked={denseDelivery?.enabled ?? false}
            disabled={disabled || deliveryBusy}
            onCheckedChange={(checked) => void applyDenseDelivery(checked)}
          />
        </div>
      </div>
    </div>
  )
}
