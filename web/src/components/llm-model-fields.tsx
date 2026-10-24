import { useEffect, useRef, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { listedModelIds, ModelSelect } from './model-select'
import type { CursorModelCatalogEntry, LlmConfigResponse, LlmLiveCatalogResponse, LlmProviderOption } from '../types'

type Api = <T>(path: string, init?: RequestInit) => Promise<T>

function resolveProviderId(raw: string): string {
  const trimmed = raw.trim().toLowerCase()
  if (trimmed === 'gemini') return 'google'
  if (trimmed === 'grok') return 'xai'
  return raw.trim()
}

function isLocalProvider(providerId: string): boolean {
  return providerId === 'ollama' || providerId === 'llama' || providerId === 'llamacpp'
}

type LlmFieldsProps = {
  api: Api
  provider: string
  model: string
  baseUrl?: string
  onProvider: (id: string) => void
  onModel: (id: string) => void
  onBaseUrl?: (url: string) => void
  disabled?: boolean
  /** Hide the provider menu and load this catalog id. */
  lockProvider?: string
  /** When the model is empty, select the first id the list returns. */
  fillWhenEmpty?: boolean
  /** When the provider is empty or unknown, select the first configured provider. */
  fillProviderWhenMissing?: boolean
  onListed?: (ids: string[]) => void
}

export function LlmModelFields({
  api,
  provider,
  model,
  baseUrl = '',
  onProvider,
  onModel,
  onBaseUrl,
  disabled,
  lockProvider,
  fillWhenEmpty = false,
  fillProviderWhenMissing = false,
  onListed,
}: LlmFieldsProps) {
  const [providers, setProviders] = useState<LlmProviderOption[]>([])
  const [modelIds, setModelIds] = useState<string[]>([])
  const [notice, setNotice] = useState<string | null>(null)
  const [loadingModels, setLoadingModels] = useState(false)
  const [connectionUrl, setConnectionUrl] = useState('')
  const resolved = lockProvider ?? resolveProviderId(provider)
  const local = isLocalProvider(resolved)
  const listUrl = baseUrl.trim() || connectionUrl
  const onProviderRef = useRef(onProvider)
  const onModelRef = useRef(onModel)
  const onListedRef = useRef(onListed)
  const modelRef = useRef(model)
  onProviderRef.current = onProvider
  onModelRef.current = onModel
  onListedRef.current = onListed
  modelRef.current = model

  useEffect(() => {
    if (lockProvider) return
    let cancelled = false
    void api<LlmConfigResponse>('/api/llm')
      .then((data) => {
        if (cancelled) return
        const next = data.providers ?? []
        setProviders(next)
        const known = next.some((entry) => entry.id === resolveProviderId(provider))
        if (!known && fillProviderWhenMissing && next[0]) onProviderRef.current(next[0].id)
      })
      .catch((error: unknown) => {
        if (!cancelled) setNotice(error instanceof Error ? error.message : String(error))
      })
    return () => {
      cancelled = true
    }
  }, [api, fillProviderWhenMissing, lockProvider, provider])

  useEffect(() => {
    if (!local || onBaseUrl) return
    let cancelled = false
    void api<{ connections?: Array<{ id: string; url?: string | null }> }>('/api/connections')
      .then((data) => {
        if (cancelled) return
        setConnectionUrl(data.connections?.find((row) => row.id === 'local_server')?.url?.trim() ?? '')
      })
      .catch(() => {
        if (!cancelled) setConnectionUrl('')
      })
    return () => {
      cancelled = true
    }
  }, [api, local, onBaseUrl])

  useEffect(() => {
    if (!resolved) {
      setModelIds([])
      return
    }
    if (local && !listUrl) {
      setModelIds([])
      onListedRef.current?.([])
      setNotice('Set the local server URL under Settings → Connections, then refresh models.')
      return
    }
    let cancelled = false
    const timer = window.setTimeout(() => {
      setLoadingModels(true)
      const qs = new URLSearchParams({ provider: resolved })
      if (local) qs.set('base_url', listUrl)
      void api<LlmLiveCatalogResponse>(`/api/llm/models?${qs.toString()}`)
        .then((res) => {
          if (cancelled) return
          const ids = listedModelIds(res.models)
          setModelIds(ids)
          onListedRef.current?.(ids)
          if (res.source === 'live') {
            const count = res.live_count ?? ids.length
            setNotice(`${count} model${count === 1 ? '' : 's'} loaded from the provider.`)
          } else {
            setNotice(res.message?.trim() || 'Provider API unavailable. Showing the curated list.')
          }
          if (fillWhenEmpty && !modelRef.current.trim() && ids[0]) onModelRef.current(ids[0])
        })
        .catch((error: unknown) => {
          if (cancelled) return
          setModelIds([])
          onListedRef.current?.([])
          setNotice(error instanceof Error ? error.message : String(error))
        })
        .finally(() => {
          if (!cancelled) setLoadingModels(false)
        })
    }, local ? 400 : 0)
    return () => {
      cancelled = true
      window.clearTimeout(timer)
    }
  }, [api, fillWhenEmpty, listUrl, local, resolved])

  const providerKnown = providers.some((entry) => entry.id === resolved)

  return (
    <div className="flex flex-col gap-2">
      {lockProvider ? null : (
        <Select
          value={providerKnown ? resolved : undefined}
          disabled={disabled || providers.length === 0}
          onValueChange={(id) => {
            onProvider(id)
            onModel('')
            const entry = providers.find((item) => item.id === id)
            if (isLocalProvider(id) && onBaseUrl && entry?.default_base_url) {
              onBaseUrl(entry.default_base_url)
            }
          }}
        >
          <SelectTrigger aria-label="Provider">
            <SelectValue placeholder={providers.length === 0 ? 'No providers configured' : 'Select a provider'} />
          </SelectTrigger>
          <SelectContent>
            {providers.map((entry) => (
              <SelectItem key={entry.id} value={entry.id}>
                {entry.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      )}
      {local && onBaseUrl ? (
        <Input
          value={baseUrl}
          aria-label="Base URL"
          placeholder="http://127.0.0.1:11434/v1"
          disabled={disabled}
          onChange={(event) => onBaseUrl(event.target.value)}
        />
      ) : null}
      <div className="flex flex-wrap items-start gap-2">
        <ModelSelect
          ariaLabel="Model"
          value={model}
          disabled={disabled || loadingModels}
          options={modelIds.map((id) => ({ id }))}
          onChange={onModel}
          emptyHint={notice ?? 'Refresh models, then pick one.'}
        />
        <Button
          type="button"
          size="sm"
          variant="outline"
          disabled={disabled || loadingModels || !resolved || (local && !listUrl)}
          onClick={() => {
            setNotice(null)
            const qs = new URLSearchParams({ provider: resolved })
            if (local) qs.set('base_url', listUrl)
            setLoadingModels(true)
            void api<LlmLiveCatalogResponse>(`/api/llm/models?${qs.toString()}`)
              .then((res) => {
                const ids = listedModelIds(res.models)
                setModelIds(ids)
                onListedRef.current?.(ids)
                setNotice(
                  res.source === 'live'
                    ? `${ids.length} model${ids.length === 1 ? '' : 's'} loaded from the provider.`
                    : res.message?.trim() || 'Provider API unavailable. Showing the curated list.',
                )
                if (fillWhenEmpty && !modelRef.current.trim() && ids[0]) onModelRef.current(ids[0])
              })
              .catch((error: unknown) => setNotice(error instanceof Error ? error.message : String(error)))
              .finally(() => setLoadingModels(false))
          }}
        >
          {loadingModels ? 'Loading…' : 'Refresh models'}
        </Button>
      </div>
      {notice && modelIds.length > 0 ? (
        <span className="text-xs text-muted-foreground" role="status">
          {notice}
        </span>
      ) : null}
    </div>
  )
}

type LocalFieldsProps = {
  api: Api
  baseUrl: string
  model: string
  onBaseUrl: (url: string) => void
  onModel: (id: string) => void
  onListed?: (ids: string[]) => void
  disabled?: boolean
  fillWhenEmpty?: boolean
}

export function LocalModelFields({
  api,
  baseUrl,
  model,
  onBaseUrl,
  onModel,
  onListed,
  disabled,
  fillWhenEmpty = false,
}: LocalFieldsProps) {
  const [modelIds, setModelIds] = useState<string[]>([])
  const [notice, setNotice] = useState<string | null>(null)
  const [loadingModels, setLoadingModels] = useState(false)
  const onModelRef = useRef(onModel)
  const onListedRef = useRef(onListed)
  const modelRef = useRef(model)
  onModelRef.current = onModel
  onListedRef.current = onListed
  modelRef.current = model

  useEffect(() => {
    const url = baseUrl.trim()
    if (!url) {
      setModelIds([])
      onListedRef.current?.([])
      setNotice('Enter the server URL, then the model list loads.')
      return
    }
    let cancelled = false
    const timer = window.setTimeout(() => {
      setLoadingModels(true)
      void api<{ models?: Array<{ id: string } | string> }>(
        `/api/multimodel/models?base_url=${encodeURIComponent(url)}`,
      )
        .then((res) => {
          if (cancelled) return
          const ids = (res.models ?? [])
            .map((entry) => (typeof entry === 'string' ? entry : entry.id))
            .filter((id): id is string => Boolean(id))
          setModelIds(ids)
          onListedRef.current?.(ids)
          setNotice(
            ids.length === 0
              ? 'No models returned from the local server.'
              : `${ids.length} model${ids.length === 1 ? '' : 's'} loaded from the server.`,
          )
          if (fillWhenEmpty && !modelRef.current.trim() && ids[0]) onModelRef.current(ids[0])
        })
        .catch((error: unknown) => {
          if (cancelled) return
          setModelIds([])
          onListedRef.current?.([])
          setNotice(error instanceof Error ? error.message : String(error))
        })
        .finally(() => {
          if (!cancelled) setLoadingModels(false)
        })
    }, 400)
    return () => {
      cancelled = true
      window.clearTimeout(timer)
    }
  }, [api, baseUrl, fillWhenEmpty])

  return (
    <div className="flex flex-col gap-2">
      <Input
        value={baseUrl}
        aria-label="Local server URL"
        placeholder="http://127.0.0.1:8080/v1"
        disabled={disabled}
        onChange={(event) => onBaseUrl(event.target.value)}
      />
      <ModelSelect
        ariaLabel="Local model"
        value={model}
        disabled={disabled || loadingModels}
        options={modelIds.map((id) => ({ id }))}
        onChange={onModel}
        emptyHint={notice ?? 'Refresh models, then pick one.'}
      />
      {notice && modelIds.length > 0 ? (
        <span className="text-xs text-muted-foreground" role="status">
          {notice}
        </span>
      ) : null}
    </div>
  )
}

type CursorFieldsProps = {
  api: Api
  model: string
  onModel: (id: string) => void
  disabled?: boolean
  fillWhenEmpty?: boolean
}

export function CursorModelFields({ api, model, onModel, disabled, fillWhenEmpty = false }: CursorFieldsProps) {
  const [models, setModels] = useState<CursorModelCatalogEntry[]>([])
  const [notice, setNotice] = useState<string | null>(null)
  const [loadingModels, setLoadingModels] = useState(false)
  const onModelRef = useRef(onModel)
  const modelRef = useRef(model)
  onModelRef.current = onModel
  modelRef.current = model

  useEffect(() => {
    let cancelled = false
    setLoadingModels(true)
    void api<{ models?: Array<CursorModelCatalogEntry | string | { id: string }> }>('/api/cursor-engine/models')
      .then((res) => {
        if (cancelled) return
        const catalog = (res.models ?? [])
          .map((entry) => (typeof entry === 'string' ? { id: entry } : entry))
          .filter((entry) => Boolean(entry.id))
        setModels(catalog)
        setNotice(
          catalog.length === 0
            ? 'No models returned from Cursor.'
            : `${catalog.length} model${catalog.length === 1 ? '' : 's'} loaded from Cursor.`,
        )
        if (fillWhenEmpty && !modelRef.current.trim() && catalog[0]) onModelRef.current(catalog[0].id)
      })
      .catch((error: unknown) => {
        if (cancelled) return
        setModels([])
        setNotice(error instanceof Error ? error.message : String(error))
      })
      .finally(() => {
        if (!cancelled) setLoadingModels(false)
      })
    return () => {
      cancelled = true
    }
  }, [api, fillWhenEmpty])

  return (
    <div className="flex flex-col gap-2">
      <ModelSelect
        ariaLabel="Cursor model"
        value={model}
        disabled={disabled || loadingModels}
        options={models.map((entry) => ({
          id: entry.id,
          label: entry.display_name?.trim() ? `${entry.display_name} (${entry.id})` : entry.id,
        }))}
        onChange={onModel}
        emptyHint={notice ?? 'Refresh models, then pick one.'}
      />
      {notice && models.length > 0 ? (
        <span className="text-xs text-muted-foreground" role="status">
          {notice}
        </span>
      ) : null}
    </div>
  )
}
