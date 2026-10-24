import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { Select, SelectTrigger, SelectContent, SelectItem, SelectValue } from '@/components/ui/select'
import { listedModelIds, ModelSelect } from './model-select'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { SettingsPanelSkeleton } from './skeleton'
import type { LlmCatalogModel, LlmConfigResponse, LlmLiveCatalogResponse, LlmProviderOption } from '../types'

type Props = {
  api: <T>(path: string, init?: RequestInit) => Promise<T>
  onError: (message: string) => void
  onSaved?: (model: string) => void
  /** When set, load/save classic strategy LLM for this persona. */
  activePersonaId?: number | null
  /** Thinking toggles are shared. Persona pages omit them. */
  includeThinking?: boolean
}

export function SettingsLlmPanel({
  api,
  onError,
  onSaved,
  activePersonaId = null,
  includeThinking = true,
}: Props) {
  const [llm, setLlm] = useState<LlmConfigResponse | null>(null)
  const [loading, setLoading] = useState(true)
  const [selectedProvider, setSelectedProvider] = useState('')
  const [selectedModel, setSelectedModel] = useState('')
  const [serverUrl, setServerUrl] = useState('')
  const [thinkingEnabled, setThinkingEnabled] = useState(false)
  const [showThinking, setShowThinking] = useState(false)
  const [saving, setSaving] = useState(false)
  const [saveNotice, setSaveNotice] = useState<string | null>(null)
  const [liveModels, setLiveModels] = useState<LlmCatalogModel[] | null>(null)
  const [catalogSource, setCatalogSource] = useState<'live' | 'static_fallback' | 'static_curated'>(
    'static_curated',
  )
  const [loadingModels, setLoadingModels] = useState(false)
  const [modelsNotice, setModelsNotice] = useState<string | null>(null)
  const lastLoadedKeyRef = useRef('')

  const load = useCallback(async (opts?: { silent?: boolean }) => {
    if (!opts?.silent) {
      setLoading(true)
      setSaveNotice(null)
    }
    try {
      const qs =
        activePersonaId != null && activePersonaId > 0
          ? `?persona_id=${activePersonaId}`
          : ''
      const data = await api<LlmConfigResponse>(`/api/llm${qs}`)
      setLlm(data)
      const available = data.providers ?? []
      const activeId = data.provider?.id ?? ''
      const providerId = available.some((p) => p.id === activeId)
        ? activeId
        : (available[0]?.id ?? '')
      setSelectedProvider(providerId)
      setSelectedModel(data.model ?? '')
      const activeIsLocal =
        providerId === 'ollama' || providerId === 'llama' || providerId === 'llamacpp'
      if (activeIsLocal) {
        const entry = data.providers?.find((p) => p.id === providerId)
        setServerUrl(
          data.base_url?.trim() ||
            entry?.default_base_url?.trim() ||
            data.default_base_url?.trim() ||
            '',
        )
      } else {
        setServerUrl('')
      }
      setThinkingEnabled(data.thinking_enabled === true)
      setShowThinking(data.show_thinking === true)
      setLiveModels(null)
      setCatalogSource('static_curated')
      setModelsNotice(null)
      lastLoadedKeyRef.current = ''
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
      setLlm(null)
    } finally {
      setLoading(false)
    }
  }, [activePersonaId, api, onError])

  useEffect(() => {
    void load()
  }, [load])

  const activeProviderEntry = useMemo(
    () => llm?.providers?.find((p) => p.id === selectedProvider),
    [llm?.providers, selectedProvider],
  )

  const staticCatalog = (activeProviderEntry?.models ?? llm?.catalog ?? []).filter(
    (model) => !model.from_active_config,
  )
  const catalogForProvider = (liveModels ?? staticCatalog).filter((model) => !model.from_active_config)

  const isLocalProvider =
    selectedProvider === 'ollama' ||
    selectedProvider === 'llama' ||
    selectedProvider === 'llamacpp'

  const thinkingSupported =
    selectedProvider === 'google' ||
    selectedProvider === 'gemini' ||
    llm?.thinking_supported === true

  const applyCatalogSelection = useCallback((models: LlmCatalogModel[]) => {
    const listed = listedModelIds(models)
    const current = selectedModel.trim() || (llm?.model ?? '').trim()
    if (current && listed.includes(current)) {
      setSelectedModel(current)
      return
    }
    if (!current && listed[0]) setSelectedModel(listed[0])
  }, [llm?.model, selectedModel])

  const loadLiveModels = useCallback(
    async (opts?: { silent?: boolean; provider?: string; baseUrl?: string }) => {
      const provider = (opts?.provider ?? selectedProvider).trim()
      if (!provider) return
      const local =
        provider === 'ollama' || provider === 'llama' || provider === 'llamacpp'
      const baseUrl = (opts?.baseUrl ?? serverUrl).trim()
      if (local && !baseUrl) {
        if (!opts?.silent) {
          onError('Enter the local server URL before loading models.')
        }
        return
      }

      const loadKey = `${provider}|${local ? baseUrl : ''}`
      setLoadingModels(true)
      setModelsNotice(null)
      try {
        const qs = new URLSearchParams({ provider })
        if (local) qs.set('base_url', baseUrl)
        const res = await api<LlmLiveCatalogResponse>(`/api/llm/models?${qs.toString()}`)
        const models = res.models ?? []
        lastLoadedKeyRef.current = loadKey
        setLiveModels(models)
        setCatalogSource(res.source === 'live' ? 'live' : 'static_fallback')
        applyCatalogSelection(models)
        if (res.source === 'live') {
          const n = res.live_count ?? models.filter((m) => m.from_live).length
          const truncated = res.truncated ? ' (list truncated)' : ''
          setModelsNotice(`${n} model${n === 1 ? '' : 's'} loaded from provider API${truncated}.`)
        } else {
          const msg = res.message?.trim() || 'Provider API unavailable; showing curated list.'
          setModelsNotice(msg)
          if (!opts?.silent) onError(msg)
        }
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e)
        setLiveModels(null)
        setCatalogSource('static_fallback')
        setModelsNotice(null)
        if (!opts?.silent) onError(msg)
      } finally {
        setLoadingModels(false)
      }
    },
    [api, applyCatalogSelection, onError, selectedProvider, serverUrl],
  )

  useEffect(() => {
    if (!selectedProvider || loading) return
    const local =
      selectedProvider === 'ollama' ||
      selectedProvider === 'llama' ||
      selectedProvider === 'llamacpp'
    if (local && !serverUrl.trim()) {
      lastLoadedKeyRef.current = ''
      return
    }
    const loadKey = `${selectedProvider}|${local ? serverUrl.trim() : ''}`
    if (loadKey === lastLoadedKeyRef.current) return

    const timer = window.setTimeout(() => {
      void loadLiveModels({ silent: true, provider: selectedProvider, baseUrl: serverUrl })
    }, local ? 500 : 0)

    return () => window.clearTimeout(timer)
  }, [loadLiveModels, loading, selectedProvider, serverUrl])

  function onProviderChange(nextProvider: string) {
    setSelectedProvider(nextProvider)
    setSaveNotice(null)
    setLiveModels(null)
    setCatalogSource('static_curated')
    setModelsNotice(null)
    lastLoadedKeyRef.current = ''
    const entry = llm?.providers?.find((p) => p.id === nextProvider)
    setSelectedModel(listedModelIds(entry?.models)[0] ?? '')
    const local =
      nextProvider === 'ollama' || nextProvider === 'llama' || nextProvider === 'llamacpp'
    if (local) {
      setServerUrl(entry?.default_base_url?.trim() || '')
    } else {
      setServerUrl('')
    }
  }

  async function saveSelection() {
    const model = selectedModel.trim()
    const provider = selectedProvider.trim()
    if (!provider) {
      onError('Pick a provider.')
      return
    }
    if (!catalogForProvider.some((entry) => entry.id === model)) {
      onError('Pick a model from the list.')
      return
    }
    if (isLocalProvider && !serverUrl.trim()) {
      onError('Enter the local server URL (OpenAI-compatible /v1 base).')
      return
    }
    setSaving(true)
    setSaveNotice(null)
    try {
      const res = await api<{ ok?: boolean; message?: string; model?: string }>('/api/llm', {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          provider,
          model,
          custom: false,
          ...(includeThinking
            ? { thinking_enabled: thinkingEnabled, show_thinking: showThinking }
            : {}),
          ...(isLocalProvider ? { base_url: serverUrl.trim() } : {}),
          ...(activePersonaId != null && activePersonaId > 0
            ? { persona_id: activePersonaId }
            : {}),
        }),
      })
      setSaveNotice(res.message ?? 'Saved.')
      await load({ silent: true })
      onSaved?.(res.model ?? model)
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
    } finally {
      setSaving(false)
    }
  }

  if (loading) {
    return <SettingsPanelSkeleton />
  }

  if (!llm?.ok) {
    return (
      <span>
        Could not load LLM configuration.
      </span>
    )
  }

  const selectedCatalog = catalogForProvider.find((m) => m.id === selectedModel)
  const catalogLabel = (m: LlmCatalogModel) => {
    if (m.from_active_config) return `${m.id} (active, from config)`
    return m.id
  }

  return (
    <div className="flex flex-col gap-3">
      <span>
        Add provider keys under Settings → Connections. Provider and model are saved on this
        persona. A missing model stops the turn. Model lists load live from the provider API;
        curated cost hints are shown when the id matches.
        {includeThinking
          ? ' Thinking toggles stay shared across personas.'
          : ' Thinking toggles are on Settings → System LLM.'}
      </span>

      <div className="flex flex-col gap-2">
        <span className="font-medium">
          Provider
        </span>
        {(llm.providers ?? []).length === 0 ? (
          <div role="status" className="mc-notice-owed">
              No provider API keys yet. Add one under Settings → Connections, then reload this page.
            </div>
        ) : (
          <>
            <Select value={selectedProvider} onValueChange={onProviderChange}>
              <SelectTrigger placeholder="Select provider" ><SelectValue /></SelectTrigger>
              <SelectContent>
                {(llm.providers ?? []).map((p: LlmProviderOption) => (
                  <SelectItem key={p.id} value={p.id}>
                    {p.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            {activeProviderEntry ? (
              isLocalProvider ? (
                <span>
                  Local provider — no API key required.
                </span>
              ) : (
                <span>
                  API key is set ({activeProviderEntry.api_key_env_hints.join(' or ')}). Add or replace it under Settings → Connections.
                </span>
              )
            ) : null}
          </>
        )}
      </div>

      {isLocalProvider ? (
        <div className="flex flex-col gap-2">
          <span className="font-medium">
            Server URL
          </span>
          <Input
            placeholder={
              selectedProvider === 'ollama'
                ? 'http://127.0.0.1:11434/v1'
                : 'http://127.0.0.1:8080/v1'
            }
            value={serverUrl}
            onChange={(e) => setServerUrl(e.target.value)}
          />
          <span>
            OpenAI-compatible API base (include <code className="text-xs">/v1</code>). Saved in the
            app database — not read from <code className="text-xs">.env</code>.
          </span>
        </div>
      ) : null}

      <div className="flex flex-col gap-2">
        <div className="flex items-center justify-between gap-3">
          <span className="font-medium">
            Model
          </span>
          <Button
            size="sm"
            variant="secondary"
            type="button"
            disabled={loadingModels || (llm.providers ?? []).length === 0}
            onClick={() => void loadLiveModels()}
          >
            {loadingModels ? 'Loading…' : 'Refresh models'}
          </Button>
        </div>
        <ModelSelect
          ariaLabel="Model"
          value={selectedModel}
          options={catalogForProvider.filter((entry) => entry.id).map((entry) => ({
            id: entry.id,
            label: catalogLabel(entry),
          }))}
          onChange={setSelectedModel}
          disabled={loadingModels || (llm.providers ?? []).length === 0}
        />
        {modelsNotice ? (
          <span>
            {modelsNotice}
          </span>
        ) : (
          <span>
            {catalogSource === 'live'
              ? 'Showing live models from the provider API.'
              : 'Showing curated fallback until the provider API responds.'}
          </span>
        )}
      </div>

      {selectedCatalog ? (
        <div
          className="rounded-md border p-3 text-sm"
          style={{ borderColor: 'var(--gray-6)' }}
        >
          <span className="font-semibold mb-1 block">
            Cost reference
          </span>
          <div className="flex flex-col gap-1">
            <span>
              Tier: <span className="capitalize">{selectedCatalog.cost_tier}</span>
            </span>
            <span>
              {selectedCatalog.cost_summary}
            </span>
          </div>
          {llm.cost_reference_note ? (
            <span className="mt-2 block">
              {llm.cost_reference_note}
            </span>
          ) : null}
        </div>
      ) : null}

      {includeThinking ? (
      <div className="rounded-md border p-3" style={{ borderColor: 'var(--gray-6)' }}>
        <span className="font-semibold mb-1 block">
          Thinking
        </span>
        <div className="flex flex-col gap-2">
          <div className="flex items-center justify-between gap-3">
            <div className="flex flex-col gap-1" style={{ flex: 1 }}>
              <span>Enable extended thinking</span>
              <span>
                {thinkingSupported
                  ? 'Sends provider thinking config (Gemini thinkingLevel / thinkingBudget).'
                  : 'Currently supported for Google (Gemini API) only.'}
              </span>
            </div>
            <Switch
              size="default"
              checked={thinkingEnabled}
              disabled={!thinkingSupported}
              onCheckedChange={(checked) => {
                setThinkingEnabled(checked)
                if (!checked) setShowThinking(false)
              }}
            />
          </div>
          <div className="flex items-center justify-between gap-3">
            <div className="flex flex-col gap-1" style={{ flex: 1 }}>
              <span>Show thinking in replies</span>
              <span>
                When enabled, reasoning is included in channel output instead of being hidden.
              </span>
            </div>
            <Switch
              size="default"
              checked={showThinking}
              disabled={!thinkingEnabled}
              onCheckedChange={setShowThinking}
            />
          </div>
        </div>
      </div>
      ) : null}

      <div className="flex gap-2 items-center flex-wrap">
        <Button
          size="default"
          disabled={saving || (llm.providers ?? []).length === 0}
          onClick={() => void saveSelection()}
        >
          {saving ? 'Saving…' : 'Save provider & model'}
        </Button>
        <span>
          Active:{' '}
          <span className="font-mono">
            {llm.provider?.label ?? llm.provider?.id} / {llm.model}
            {llm.is_local_provider && llm.base_url ? ` @ ${llm.base_url}` : ''}
          </span>
          {llm.model?.trim()
            ? ' (saved on this persona)'
            : ' (set a model before the next turn)'}
        </span>
      </div>

      {saveNotice ? (
        <span className="text-xs text-muted-foreground" role="status">{saveNotice}</span>
      ) : null}
    </div>
  )
}
