import { Button } from '@/components/ui/button'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { LlmConfigResponse, LlmLiveCatalogResponse, TopicsModelResponse } from '../types'

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
  const loadedProvidersRef = useRef<Set<string>>(new Set())

  const load = useCallback(async () => {
    setLoading(true)
    try {
      const [next, llmConfig] = await Promise.all([
        api<TopicsModelResponse>('/api/topics/model'),
        api<LlmConfigResponse>('/api/llm'),
      ])
      setData(next)
      setLlm(llmConfig)
      const providerId = next.provider?.trim() || (next.source === 'local' ? 'llama' : '')
      setProvider(providerId)
      setModel(next.model?.trim() || '')
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
  const catalogModels = providerEntry?.models.map((entry) => entry.id) ?? []
  const liveForProvider = liveModels[provider] ?? []
  const modelOptions =
    liveForProvider.length > 0
      ? Array.from(new Set([...liveForProvider, ...(model ? [model] : [])]))
      : Array.from(new Set([...catalogModels, ...(model ? [model] : [])]))
  const providerLabel = providerEntry?.label || provider || 'the selected provider'
  const localBaseUrl = data?.local_base_url?.trim() ?? ''

  async function save(nextProvider = provider, nextModel = model) {
    const chosenProvider = nextProvider.trim()
    const chosenModel = nextModel.trim()
    if (!chosenProvider || !chosenModel) {
      onError('Choose a provider and a model for topic titles.')
      return
    }
    setSaving(true)
    setNotice(null)
    try {
      const next = await api<TopicsModelResponse>('/api/topics/model', {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ provider: chosenProvider, model: chosenModel }),
      })
      setData(next)
      setProvider(next.provider?.trim() || chosenProvider)
      setModel(next.model?.trim() || chosenModel)
      setNotice(next.message ?? 'Topic model saved.')
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
        const ids = (res.models ?? []).map((entry) => entry.id).filter((id) => Boolean(id))
        if (res.source === 'live' && ids.length > 0) {
          setLiveModels((prev) => ({ ...prev, [chosen]: ids }))
          const n = res.live_count ?? ids.length
          const truncated = res.truncated ? ' (truncated; use custom id for others)' : ''
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

  if (loading) {
    return <span className="text-sm text-muted-foreground">Loading topic model…</span>
  }

  const using = model ? `${providerLabel} / ${model}` : providerLabel
  const titleTooltip = `A background pass names each stretch of the chat. It reads the open topic plus newer messages, up to 40 shortened lines, and asks ${using} for JSON boundaries and titles. Closed topics and titles you edited stay as saved. If that request fails, the previous titles stay.`

  return (
    <div className="flex flex-col gap-2 rounded-md border p-3" style={{ borderColor: 'var(--gray-6)' }}>
      <span className="font-medium">Topic titles</span>
      <span className="text-sm text-muted-foreground">
        Provider and model that name conversation sections. The next segmentation pass uses this choice. Existing
        titles stay until a topic is still open and untitled by you.
      </span>
      <div className="flex flex-wrap items-center gap-2">
        <Select
          value={provider || undefined}
          disabled={saving}
          onValueChange={(value) => {
            setProvider(value)
            const entry = providers.find((item) => item.id === value)
            const ids = entry?.models.map((item) => item.id) ?? []
            setModel((current) => (ids.includes(current) ? current : (ids[0] ?? '')))
          }}
        >
          <SelectTrigger className="min-w-[180px]" aria-label="Topic title provider">
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
        <Tooltip>
          <TooltipTrigger asChild>
            <span className="inline-flex min-w-[220px] flex-1">
              <Select value={model || undefined} disabled={saving} onValueChange={setModel}>
                <SelectTrigger className="w-full" aria-label="Topic title model">
                  <SelectValue placeholder="Model" />
                </SelectTrigger>
                <SelectContent>
                  {modelOptions.map((id) => (
                    <SelectItem key={id} value={id}>
                      {id}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </span>
          </TooltipTrigger>
          <TooltipContent side="top" className="max-w-sm text-left">
            {titleTooltip}
          </TooltipContent>
        </Tooltip>
        <Button
          size="sm"
          variant="outline"
          type="button"
          disabled={saving || loadingModels || !provider || (isLocalProvider(provider) && !localBaseUrl)}
          onClick={() => void refreshModels()}
        >
          {loadingModels ? 'Loading…' : 'Refresh models'}
        </Button>
        <Button size="sm" type="button" disabled={saving || !provider || !model} onClick={() => void save()}>
          {saving ? 'Saving…' : 'Save'}
        </Button>
      </div>
      <span className="text-xs text-muted-foreground" role="status">
        Using {using}
        {data?.saved && data.source !== 'strategy' ? ' (saved in app)' : ' (chat model until you save)'}.
        {isLocalProvider(provider) && !data?.local_base_url?.trim()
          ? ' Set the local server URL under Settings → Connections.'
          : ''}
      </span>
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
  )
}
