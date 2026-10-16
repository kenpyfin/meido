import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { Select, SelectTrigger, SelectContent, SelectItem, SelectValue } from '@/components/ui/select'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { SettingsPanelSkeleton } from './skeleton'
import type { LlmCatalogModel, LlmConfigResponse, LlmLiveCatalogResponse, LlmProviderOption } from '../types'

type Props = {
  api: <T>(path: string, init?: RequestInit) => Promise<T>
  onError: (message: string) => void
  onSaved?: (model: string) => void
  /** When set, load/save classic strategy LLM for this persona. */
  activePersonaId?: number | null
}

export function SettingsLlmPanel({ api, onError, onSaved, activePersonaId = null }: Props) {
  const [llm, setLlm] = useState<LlmConfigResponse | null>(null)
  const [loading, setLoading] = useState(true)
  const [selectedProvider, setSelectedProvider] = useState('')
  const [selectedModel, setSelectedModel] = useState('')
  const [customModel, setCustomModel] = useState('')
  const [useCustom, setUseCustom] = useState(false)
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
      const providerEntry = data.providers?.find((p) => p.id === providerId)
      const catalog = providerEntry?.models ?? data.catalog ?? []
      const current = data.model ?? ''
      const inCatalog = catalog.some((m) => m.id === current)
      if (inCatalog || !current) {
        setUseCustom(false)
        setSelectedModel(current || catalog[0]?.id || '')
        setCustomModel('')
      } else {
        setUseCustom(true)
        setCustomModel(current)
        setSelectedModel(catalog[0]?.id || '')
      }
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

  const staticCatalog = activeProviderEntry?.models ?? llm?.catalog ?? []
  const catalogForProvider = liveModels ?? staticCatalog

  const isLocalProvider =
    selectedProvider === 'ollama' ||
    selectedProvider === 'llama' ||
    selectedProvider === 'llamacpp'

  const thinkingSupported =
    selectedProvider === 'google' ||
    selectedProvider === 'gemini' ||
    llm?.thinking_supported === true

  const applyCatalogSelection = useCallback(
    (models: LlmCatalogModel[]) => {
      const current = (useCustom ? customModel : selectedModel).trim() || (llm?.model ?? '').trim()
      if (current && models.some((m) => m.id === current)) {
        setUseCustom(false)
        setSelectedModel(current)
        setCustomModel('')
        return
      }
      if (current) {
        setUseCustom(true)
        setCustomModel(current)
        return
      }
      if (models[0]) {
        setUseCustom(false)
        setSelectedModel(models[0].id)
      }
    },
    [customModel, llm?.model, selectedModel, useCustom],
  )

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
          const truncated = res.truncated ? ' (truncated; use custom id for others)' : ''
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
    const firstModel = entry?.models?.[0]?.id ?? ''
    if (!useCustom) {
      setSelectedModel(firstModel)
    }
    const local =
      nextProvider === 'ollama' || nextProvider === 'llama' || nextProvider === 'llamacpp'
    if (local) {
      setServerUrl(entry?.default_base_url?.trim() || '')
    } else {
      setServerUrl('')
    }
  }

  async function saveSelection() {
    const model = (useCustom ? customModel : selectedModel).trim()
    const provider = selectedProvider.trim()
    if (!provider) {
      onError('Pick a provider.')
      return
    }
    if (!model) {
      onError('Pick a model or enter a custom model id.')
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
          custom: useCustom,
          thinking_enabled: thinkingEnabled,
          show_thinking: showThinking,
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
        Put API keys in repo-root <code className="text-xs">.env</code> only (never in this UI).
        Provider and model are saved for{' '}
        {activePersonaId != null && activePersonaId > 0 ? (
          <>
            this persona
            {llm.model_scope === 'persona' ? ' (persona override active)' : ' (inherits global until saved)'}
          </>
        ) : (
          <>all personas that have not set an override</>
        )}
        . Model lists load live from the provider API; curated cost hints are shown when the id
        matches. Thinking toggles stay shared across personas.
      </span>

      <div className="flex flex-col gap-2">
        <span className="font-medium">
          Provider
        </span>
        {(llm.providers ?? []).length === 0 ? (
          <div role="status" className="rounded-lg border border-amber-500/30 bg-amber-500/10 p-3 text-sm text-amber-900 dark:text-amber-100">
              No provider API keys found in <code className="text-xs">.env</code>. Add keys such as{' '}
              <code className="text-xs">ANTHROPIC_API_KEY</code>,{' '}
              <code className="text-xs">OPENAI_API_KEY</code>, or{' '}
              <code className="text-xs">XAI_API_KEY</code>, then reload this page.
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
                  API key found in .env ({activeProviderEntry.api_key_env_hints.join(' or ')})
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
        {!useCustom && selectedModel && catalogForProvider.some((m) => m.id === selectedModel) ? (
          <Select value={selectedModel} onValueChange={setSelectedModel}>
            <SelectTrigger placeholder="Select model" ><SelectValue /></SelectTrigger>
            <SelectContent>
              {catalogForProvider.filter((m) => m.id).map((m) => (
                <SelectItem key={m.id} value={m.id}>
                  {catalogLabel(m)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        ) : (
          <Input
            placeholder="Custom model id"
            value={customModel}
            onChange={(e) => setCustomModel(e.target.value)}
          />
        )}
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
        {llm.custom_model_allowed ? (
          <Button
            size="sm"
            variant="ghost"
            type="button"
            onClick={() => {
              setUseCustom((v) => !v)
              setSaveNotice(null)
            }}
          >
            {useCustom ? 'Use catalog list' : 'Use custom model id'}
          </Button>
        ) : null}
      </div>

      {(useCustom ? customModel : selectedCatalog) ? (
        <div
          className="rounded-md border p-3 text-sm"
          style={{ borderColor: 'var(--gray-6)' }}
        >
          <span className="font-semibold mb-1 block">
            Cost reference
          </span>
          {useCustom ? (
            <span>
              Custom model - check your provider&apos;s pricing page.
            </span>
          ) : selectedCatalog ? (
            <div className="flex flex-col gap-1">
              <span>
                Tier: <span className="capitalize">{selectedCatalog.cost_tier}</span>
              </span>
              <span>
                {selectedCatalog.cost_summary}
              </span>
            </div>
          ) : null}
          {llm.cost_reference_note ? (
            <span className="mt-2 block">
              {llm.cost_reference_note}
            </span>
          ) : null}
        </div>
      ) : null}

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
          {llm.model_scope === 'persona' || llm.provider_source === 'persona'
            ? ' (saved for this persona)'
            : llm.provider_source === 'app_settings' && llm.model_source === 'app_settings'
              ? ' (saved in app)'
              : ' (auto-selected - save to confirm)'}
        </span>
      </div>

      {saveNotice ? (
        <div role="status" className="rounded-lg border border-green-500/30 bg-green-500/10 p-3 text-sm text-green-900 dark:text-green-100">{saveNotice}</div>
      ) : null}
    </div>
  )
}
