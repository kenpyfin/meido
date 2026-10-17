import { Button } from '@/components/ui/button'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { listedModelIds, ModelSelect } from './model-select'
import { Switch } from '@/components/ui/switch'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { LlmConfigResponse, LlmLiveCatalogResponse, SystemLlmJobRow, TopicsModelResponse } from '../types'

const SHARED_DEFAULT = '__default__'

type JobDraft = {
  id: string
  label: string
  detail: string
  provider: string
  model: string
}

type Props = {
  api: <T>(path: string, init?: RequestInit) => Promise<T>
  onError: (message: string) => void
}

function isLocalProvider(providerId: string): boolean {
  return providerId === 'local' || providerId === 'ollama' || providerId === 'llama' || providerId === 'llamacpp'
}

export function SettingsTopicsModelPanel({ api, onError }: Props) {
  const [data, setData] = useState<TopicsModelResponse | null>(null)
  const [llm, setLlm] = useState<LlmConfigResponse | null>(null)
  const [provider, setProvider] = useState('')
  const [model, setModel] = useState('')
  const [liveModels, setLiveModels] = useState<Record<string, string[]>>({})
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [loadingModels, setLoadingModels] = useState(false)
  const [notice, setNotice] = useState<string | null>(null)
  const [modelsNotice, setModelsNotice] = useState<string | null>(null)
  const [thinkingEnabled, setThinkingEnabled] = useState(false)
  const [showThinking, setShowThinking] = useState(false)
  const [thinkingBaseline, setThinkingBaseline] = useState({ enabled: false, show: false })
  const [thinkingBusy, setThinkingBusy] = useState(false)
  const [thinkingNotice, setThinkingNotice] = useState<string | null>(null)
  const [jobs, setJobs] = useState<JobDraft[]>([])
  const loadedProvidersRef = useRef<Set<string>>(new Set())

  const load = useCallback(async () => {
    setLoading(true)
    try {
      const [next, llmConfig] = await Promise.all([
        api<TopicsModelResponse>('/api/system-llm'),
        api<LlmConfigResponse>('/api/llm'),
      ])
      setData(next)
      setLlm(llmConfig)
      const providerId = next.provider?.trim() || (next.source === 'local' ? 'llama' : '')
      setProvider(providerId)
      setModel(next.model?.trim() || '')
      setJobs(
        (next.jobs ?? []).map((job: SystemLlmJobRow) => ({
          id: job.id,
          label: job.label,
          detail: job.detail,
          provider: job.provider?.trim() || '',
          model: job.model?.trim() || job.legacy_model?.trim() || '',
        })),
      )
      const enabled = llmConfig.thinking_enabled === true
      const show = llmConfig.show_thinking === true
      setThinkingEnabled(enabled)
      setShowThinking(show)
      setThinkingBaseline({ enabled, show })
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
      setData(null)
    } finally {
      setLoading(false)
    }
  }, [api, onError])

  useEffect(() => {
    void load()
  }, [load])

  const providers = llm?.providers ?? []
  const providerEntry = useMemo(
    () => providers.find((entry) => entry.id === provider),
    [provider, providers],
  )
  const catalogModels = listedModelIds(providerEntry?.models)
  const liveForProvider = liveModels[provider] ?? []
  const modelOptions = liveForProvider.length > 0 ? liveForProvider : catalogModels
  const providerLabel = providerEntry?.label || provider || 'the selected provider'
  const localBaseUrl = data?.local_base_url?.trim() ?? ''

  function modelsFor(providerId: string): string[] {
    const entry = providers.find((item) => item.id === providerId)
    const catalog = listedModelIds(entry?.models)
    const live = liveModels[providerId] ?? []
    return live.length > 0 ? live : catalog
  }

  function updateJob(id: string, patch: Partial<JobDraft>) {
    setJobs((current) => current.map((job) => (job.id === id ? { ...job, ...patch } : job)))
    setNotice(null)
  }

  async function save(nextProvider = provider, nextModel = model) {
    const chosenProvider = nextProvider.trim()
    const chosenModel = nextModel.trim()
    if (!chosenProvider || !modelOptions.includes(chosenModel)) {
      onError('Pick a shared model from the list.')
      return
    }
    for (const job of jobs) {
      if (job.provider && !modelsFor(job.provider).includes(job.model.trim())) {
        onError(`Pick a model from the list for ${job.label}, or set the row back to the shared default.`)
        return
      }
      if (!job.provider && job.id === 'prompt_hooks' && job.model.trim() && !modelOptions.includes(job.model.trim())) {
        onError('Pick a prompt hooks model from the list, or leave the shared default.')
        return
      }
      if (job.provider && isLocalProvider(job.provider) && !localBaseUrl) {
        onError(`Set a local server URL under Settings → Connections before using a local model for ${job.label}.`)
        return
      }
    }
    setSaving(true)
    setNotice(null)
    try {
      const next = await api<TopicsModelResponse>('/api/system-llm', {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          provider: chosenProvider,
          model: chosenModel,
          jobs: jobs.map((job) => ({
            id: job.id,
            provider: job.provider,
            model: job.provider || job.id === 'prompt_hooks' ? job.model.trim() : '',
          })),
        }),
      })
      setData(next)
      setProvider(next.provider?.trim() || chosenProvider)
      setModel(next.model?.trim() || chosenModel)
      setJobs(
        (next.jobs ?? []).map((job) => ({
          id: job.id,
          label: job.label,
          detail: job.detail,
          provider: job.provider?.trim() || '',
          model: job.model?.trim() || job.legacy_model?.trim() || '',
        })),
      )
      setNotice(next.message ?? 'System LLM saved.')
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
      await load()
    } finally {
      setSaving(false)
    }
  }

  const refreshModels = useCallback(
    async (providerId = provider, opts?: { silent?: boolean }) => {
      const chosen = providerId.trim()
      if (!chosen) return
      const local = isLocalProvider(chosen)
      if (local && !localBaseUrl) {
        if (!opts?.silent) {
          onError('Set a local server URL under Settings → Connections before loading models.')
        }
        return
      }
      setLoadingModels(true)
      try {
        const qs = new URLSearchParams({ provider: chosen })
        if (local) qs.set('base_url', localBaseUrl)
        const res = await api<LlmLiveCatalogResponse>(`/api/llm/models?${qs.toString()}`)
        const ids = listedModelIds(res.models)
        if (res.source === 'live' && ids.length > 0) {
          setLiveModels((prev) => ({ ...prev, [chosen]: ids }))
          const n = res.live_count ?? ids.length
          const truncated = res.truncated ? ' (list truncated)' : ''
          setModelsNotice(`${n} model${n === 1 ? '' : 's'} loaded from provider API${truncated}.`)
          setModel((current) => {
            if (current && ids.includes(current)) return current
            if (opts?.silent) return current
            return ids[0] ?? current
          })
        } else {
          const msg = res.message?.trim() || 'Provider API unavailable; showing curated list.'
          setModelsNotice(msg)
          if (!opts?.silent) onError(msg)
        }
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e)
        setModelsNotice(null)
        if (!opts?.silent) onError(msg)
      } finally {
        setLoadingModels(false)
      }
    },
    [api, localBaseUrl, onError, provider],
  )

  useEffect(() => {
    if (loading || !provider || loadedProvidersRef.current.has(provider)) return
    if (isLocalProvider(provider) && !localBaseUrl) return
    loadedProvidersRef.current.add(provider)
    void refreshModels(provider, { silent: true })
  }, [loading, localBaseUrl, provider, refreshModels])

  const thinkingDirty =
    thinkingEnabled !== thinkingBaseline.enabled || showThinking !== thinkingBaseline.show

  const systemDirty = useMemo(() => {
    if (!data) return false
    if ((data.provider?.trim() || '') !== provider.trim()) return true
    if ((data.model?.trim() || '') !== model.trim()) return true
    return jobs.some((job) => {
      const saved = (data.jobs ?? []).find((row) => row.id === job.id)
      const savedProvider = saved?.provider?.trim() || ''
      const savedModel = saved?.model?.trim() || saved?.legacy_model?.trim() || ''
      return job.provider !== savedProvider || job.model !== savedModel
    })
  }, [data, jobs, model, provider])

  async function saveThinking() {
    setThinkingBusy(true)
    setThinkingNotice(null)
    try {
      const next = await api<{ message?: string; thinking_enabled?: boolean; show_thinking?: boolean }>(
        '/api/llm',
        {
          method: 'PATCH',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            model: '',
            thinking_enabled: thinkingEnabled,
            show_thinking: showThinking,
          }),
        },
      )
      const enabled = next.thinking_enabled === true
      const show = next.show_thinking === true
      setThinkingEnabled(enabled)
      setShowThinking(show)
      setThinkingBaseline({ enabled, show })
      setThinkingNotice(next.message ?? 'Thinking settings saved.')
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
    } finally {
      setThinkingBusy(false)
    }
  }

  if (loading) {
    return <span className="text-sm text-muted-foreground">Loading system LLM…</span>
  }

  const using = model ? `${providerLabel} / ${model}` : providerLabel

  return (
    <div className="flex flex-col gap-3">
    <div className="flex flex-col gap-3 rounded-md border p-3" style={{ borderColor: 'var(--mc-border-soft)' }}>
      <span className="font-medium">Shared default</span>
      <span className="text-sm text-muted-foreground">
        Blank rows use this provider and model. Channel chat stays on the persona. Saving takes effect on the next
        pass. No restart.
      </span>
      <div className="flex flex-wrap items-center gap-2">
        <Select
          value={provider || undefined}
          disabled={saving}
          onValueChange={(value) => {
            setProvider(value)
            const entry = providers.find((item) => item.id === value)
            const ids = listedModelIds(entry?.models)
            setModel((current) => (ids.includes(current) ? current : (ids[0] ?? '')))
            if (!loadedProvidersRef.current.has(value)) {
              loadedProvidersRef.current.add(value)
              void refreshModels(value, { silent: true })
            }
          }}
        >
          <SelectTrigger className="min-w-[180px]" aria-label="Shared default provider">
            <SelectValue placeholder="Provider" />
          </SelectTrigger>
          <SelectContent>
            {providers.map((entry) => (
              <SelectItem key={entry.id} value={entry.id}>
                {entry.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <ModelSelect
          ariaLabel="Shared default model"
          value={model}
          disabled={saving}
          options={modelOptions.map((id) => ({ id }))}
          onChange={setModel}
        />
        <Button
          size="sm"
          variant="outline"
          type="button"
          disabled={saving || loadingModels || !provider || (isLocalProvider(provider) && !localBaseUrl)}
          onClick={() => void refreshModels()}
        >
          {loadingModels ? 'Loading…' : 'Refresh models'}
        </Button>
      </div>
      <span className="text-xs text-muted-foreground" role="status">
        Using {using}
        {data?.saved && data.source !== 'strategy' && data.source !== 'missing' ? ' (saved)' : ' (set a provider and model)'}.
        {isLocalProvider(provider) && !data?.local_base_url?.trim()
          ? ' Set the local server URL under Settings → Connections.'
          : ''}
      </span>
      {jobs.map((job) => {
        const rowModels = job.provider ? modelsFor(job.provider) : []
        return (
          <div
            key={job.id}
            className="flex flex-col gap-2 border-t pt-3"
            style={{ borderColor: 'var(--mc-border-soft)' }}
          >
            <span className="font-medium">{job.label}</span>
            <span className="text-sm text-muted-foreground">{job.detail}</span>
            <div className="flex flex-wrap items-center gap-2">
              <Select
                value={job.provider || SHARED_DEFAULT}
                disabled={saving}
                onValueChange={(value) => {
                  if (value === SHARED_DEFAULT) {
                    updateJob(job.id, { provider: '' })
                    return
                  }
                  const ids = modelsFor(value)
                  updateJob(job.id, {
                    provider: value,
                    model: ids.includes(job.model) ? job.model : (ids[0] ?? ''),
                  })
                  if (!loadedProvidersRef.current.has(value)) {
                    loadedProvidersRef.current.add(value)
                    void refreshModels(value, { silent: true })
                  }
                }}
              >
                <SelectTrigger className="min-w-[180px]" aria-label={`${job.label} provider`}>
                  <SelectValue placeholder="Shared default" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value={SHARED_DEFAULT}>Shared default</SelectItem>
                  {providers.map((entry) => (
                    <SelectItem key={entry.id} value={entry.id}>
                      {entry.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              {job.provider ? (
                <ModelSelect
                  ariaLabel={`${job.label} model`}
                  value={job.model}
                  disabled={saving}
                  options={rowModels.map((id) => ({ id }))}
                  onChange={(value) => updateJob(job.id, { model: value })}
                />
              ) : job.id === 'prompt_hooks' ? (
                <ModelSelect
                  ariaLabel="Prompt hooks model"
                  value={job.model}
                  blankLabel="Shared default"
                  disabled={saving}
                  options={modelOptions.map((id) => ({ id }))}
                  onChange={(value) => updateJob(job.id, { model: value })}
                  emptyHint="Load the shared provider’s models, then pick one or leave the shared default."
                />
              ) : (
                <span className="text-xs text-muted-foreground">Using the shared default.</span>
              )}
            </div>
          </div>
        )
      })}
      <div>
        <Button size="sm" type="button" disabled={saving || !provider || !model || !systemDirty} onClick={() => void save()}>
          {saving ? 'Saving…' : 'Save system LLM'}
        </Button>
      </div>
      {modelsNotice ? (
        <span className="text-xs text-muted-foreground" role="status">
          {modelsNotice}
        </span>
      ) : null}
      {notice ? (
        <span className="text-xs text-muted-foreground" role="status">
          {notice}
        </span>
      ) : null}
    </div>
    <div className="flex flex-col gap-2 rounded-md border p-3" style={{ borderColor: 'var(--mc-border-soft)' }}>
      <span className="font-medium">Thinking</span>
      <span className="text-sm text-muted-foreground">
        Shared across personas. A Google chat model sends this config. Other providers ignore it.
      </span>
      <div className="flex items-center justify-between gap-3">
        <span>Enable extended thinking</span>
        <Switch
          checked={thinkingEnabled}
          disabled={thinkingBusy}
          onCheckedChange={(checked) => {
            setThinkingEnabled(checked)
            if (!checked) setShowThinking(false)
            setThinkingNotice(null)
          }}
        />
      </div>
      <div className="flex items-center justify-between gap-3">
        <span>Show thinking in replies</span>
        <Switch
          checked={showThinking}
          disabled={thinkingBusy || !thinkingEnabled}
          onCheckedChange={(checked) => {
            setShowThinking(checked)
            setThinkingNotice(null)
          }}
        />
      </div>
      <div>
        <Button
          size="sm"
          type="button"
          disabled={thinkingBusy || !thinkingDirty}
          onClick={() => void saveThinking()}
        >
          {thinkingBusy ? 'Saving…' : 'Save thinking'}
        </Button>
      </div>
      {thinkingNotice ? (
        <span className="text-xs text-muted-foreground" role="status">
          {thinkingNotice}
        </span>
      ) : null}
    </div>
    </div>
  )
}
