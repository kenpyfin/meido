import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectTrigger, SelectContent, SelectItem, SelectValue } from '@/components/ui/select'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { SettingsPanelSkeleton } from './skeleton'
import type { LocalDelegateConfigResponse } from '../types'

type Props = {
  api: <T>(path: string, init?: RequestInit) => Promise<T>
  onError: (message: string) => void
}

type StepId = 'configure' | 'test' | 'save'

export function SettingsLocalDelegatePanel({ api, onError }: Props) {
  const [config, setConfig] = useState<LocalDelegateConfigResponse | null>(null)
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [testing, setTesting] = useState(false)
  const [saveNotice, setSaveNotice] = useState<string | null>(null)
  const [testNotice, setTestNotice] = useState<string | null>(null)
  const [testPassed, setTestPassed] = useState<boolean | null>(null)

  const [localBaseUrl, setLocalBaseUrl] = useState('')
  const [localModel, setLocalModel] = useState('')
  const [availableModels, setAvailableModels] = useState<string[]>([])
  const [loadingModels, setLoadingModels] = useState(false)
  const [modelsNotice, setModelsNotice] = useState<string | null>(null)
  const [useCustomModel, setUseCustomModel] = useState(false)
  const autoLoadedModelsRef = useRef(false)
  const lastLoadedBaseUrlRef = useRef('')

  const load = useCallback(async () => {
    setLoading(true)
    setSaveNotice(null)
    setTestNotice(null)
    try {
      const data = await api<LocalDelegateConfigResponse>('/api/multimodel')
      setConfig(data)
      setLocalBaseUrl(data.local_base_url ?? data.tier1_base_url ?? '')
      setLocalModel(data.local_model ?? data.tier1_model ?? '')
      setTestPassed(data.local_tools_ok === true ? true : null)
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
      setConfig(null)
    } finally {
      setLoading(false)
    }
  }, [api, onError])

  useEffect(() => {
    void load()
  }, [load])

  const loadModels = useCallback(
    async (opts?: { silent?: boolean; baseUrl?: string; model?: string }) => {
      const baseUrl = (opts?.baseUrl ?? localBaseUrl).trim()
      if (!baseUrl) {
        if (!opts?.silent) {
          onError('Enter the local server URL before loading models.')
        }
        return
      }

      setLoadingModels(true)
      setModelsNotice(null)
      try {
        const res = await api<{ models?: Array<{ id: string } | string> }>(
          `/api/multimodel/models?base_url=${encodeURIComponent(baseUrl)}`,
        )
        const ids = (res.models ?? [])
          .map((m) => (typeof m === 'string' ? m : m.id))
          .filter((id): id is string => Boolean(id))
        if (ids.length === 0) {
          const msg = 'No models returned from local server.'
          setModelsNotice(msg)
          if (!opts?.silent) onError(msg)
          return
        }
        setAvailableModels(ids)
        lastLoadedBaseUrlRef.current = baseUrl
        const current = (opts?.model ?? localModel).trim()
        if (current && ids.includes(current)) {
          setUseCustomModel(false)
        } else if (!current && ids[0]) {
          setLocalModel(ids[0])
          setUseCustomModel(false)
        }
        setModelsNotice(`${ids.length} model${ids.length === 1 ? '' : 's'} loaded from server.`)
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e)
        setModelsNotice(null)
        if (!opts?.silent) onError(msg)
      } finally {
        setLoadingModels(false)
      }
    },
    [api, localBaseUrl, localModel, onError],
  )

  useEffect(() => {
    const model = localModel.trim()
    if (model && availableModels.includes(model)) {
      setUseCustomModel(false)
    }
  }, [localModel, availableModels])

  useEffect(() => {
    const baseUrl = localBaseUrl.trim()
    if (!baseUrl) {
      setAvailableModels([])
      setModelsNotice(null)
      autoLoadedModelsRef.current = false
      lastLoadedBaseUrlRef.current = ''
      return
    }
    if (baseUrl === lastLoadedBaseUrlRef.current) return

    const timer = window.setTimeout(() => {
      if (!autoLoadedModelsRef.current) {
        autoLoadedModelsRef.current = true
      }
      void loadModels({ silent: true, baseUrl })
    }, 500)

    return () => window.clearTimeout(timer)
  }, [localBaseUrl, loadModels])

  const localToolsOk = config?.local_tools_ok === true
  const configured = localBaseUrl.trim().length > 0 && localModel.trim().length > 0

  const activeStep: StepId = useMemo(() => {
    if (!configured) return 'configure'
    if (!localToolsOk) return 'test'
    return 'save'
  }, [configured, localToolsOk])

  const steps: { id: StepId; label: string }[] = [
    { id: 'configure', label: '1. Configure local' },
    { id: 'test', label: '2. Test tools' },
    { id: 'save', label: '3. Save' },
  ]

  async function save() {
    setSaving(true)
    setSaveNotice(null)
    try {
      const res = await api<{ message?: string }>('/api/multimodel', {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          local_base_url: localBaseUrl,
          local_model: localModel,
        }),
      })
      setSaveNotice(res.message ?? 'Saved.')
      await load()
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
    } finally {
      setSaving(false)
    }
  }

  async function testLocal() {
    const base_url = localBaseUrl.trim()
    const model = localModel.trim()
    if (!base_url || !model) {
      onError('Enter server URL and model before testing.')
      return
    }
    setTesting(true)
    setTestNotice(null)
    setTestPassed(null)
    try {
      const res = await api<{ message?: string; tools_ok?: boolean }>('/api/multimodel/test', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ tier: 'local', base_url, model }),
      })
      const ok = res.tools_ok === true
      setTestPassed(ok)
      setTestNotice(res.message ?? (ok ? 'Local model OK.' : 'Test finished.'))
      setConfig((prev) =>
        prev
          ? {
              ...prev,
              local_tools_ok: ok,
            }
          : prev,
      )
    } catch (e) {
      setTestPassed(false)
      onError(e instanceof Error ? e.message : String(e))
    } finally {
      setTesting(false)
    }
  }

  if (loading) {
    return <SettingsPanelSkeleton />
  }

  if (!config?.ok) {
    return (
      <span>
        Could not load local delegate configuration.
      </span>
    )
  }

  return (
    <div className="flex flex-col gap-3">
      <div className="mc-multimodel-stepper" aria-label="Setup progress">
        {steps.map((step) => {
          const done =
            (step.id === 'configure' && configured) ||
            (step.id === 'test' && localToolsOk) ||
            (step.id === 'save' && localToolsOk)
          const active = step.id === activeStep
          return (
            <span
              key={step.id}
              className={`mc-multimodel-step ${active ? 'mc-multimodel-step--active' : ''} ${done ? 'mc-multimodel-step--done' : ''}`.trim()}
            >
              {step.label}
            </span>
          )
        })}
      </div>

      <span>
        Configure a local OpenAI-compatible server for Classic · Cost routing (read-only discovery
        and delegated sub-jobs) and PTE/PDQE sidecars. Enable cost routing from Settings → Agent
        engine (per persona, or the global inherit default).
      </span>

      {!localToolsOk ? (
        <div role="alert" className="rounded-lg border border-amber-500/30 bg-amber-500/10 p-3 text-sm text-amber-900 dark:text-amber-100">
            Tool calling is not verified for the local model. Run the test below before using
            Classic · Cost routing. Until verified, cost routing runs use the cloud model only.
          </div>
      ) : null}

      <div
        className="rounded-md border p-3"
        style={{ borderColor: 'var(--gray-6)' }}
      >
        <span className="font-semibold mb-1 block">
          Local model
        </span>
        <span className="mb-2 block">
          OpenAI-compatible server (llama.cpp, vLLM, Ollama).
        </span>
        <div className="flex flex-col gap-2">
          <Input
            placeholder="http://127.0.0.1:8080/v1"
            value={localBaseUrl}
            onChange={(e) => setLocalBaseUrl(e.target.value)}
          />
          <div className="flex gap-2 flex-wrap items-center">
            {!useCustomModel && availableModels.length > 0 ? (
              <Select value={localModel} onValueChange={setLocalModel}>
                <SelectTrigger placeholder="Select model" style={{ flex: 1, minWidth: 160 }} ><SelectValue /></SelectTrigger>
                <SelectContent>
                  {availableModels.map((id) => (
                    <SelectItem key={id} value={id}>
                      {id}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            ) : (
              <Input
                placeholder="qwen2.5-coder-14b-instruct"
                value={localModel}
                onChange={(e) => setLocalModel(e.target.value)}
                style={{ flex: 1, minWidth: 160 }}
              />
            )}
            <Button
              size="sm"
              variant="outline"
              className="cursor-pointer"
              disabled={loadingModels || !localBaseUrl.trim()}
              onClick={() => void loadModels()}
            >
              {loadingModels ? 'Loading…' : 'Refresh models'}
            </Button>
          </div>
          {availableModels.length > 0 ? (
            <Button
              size="sm"
              variant="ghost"
              type="button"
              onClick={() => setUseCustomModel((v) => !v)}
            >
              {useCustomModel ? 'Use model list' : 'Type custom model id'}
            </Button>
          ) : null}
          {modelsNotice ? (
            <span role="status">
              {modelsNotice}
            </span>
          ) : localBaseUrl.trim() ? (
            <span>
              Enter the server URL, then refresh to load models from{' '}
              <code className="text-xs">/v1/models</code>.
            </span>
          ) : null}
          <Button
            size="sm"
            variant="outline"
            className="cursor-pointer"
            disabled={testing}
            onClick={() => void testLocal()}
          >
            {testing ? 'Testing…' : 'Test local server'}
          </Button>
          <span role="status"
            aria-live="polite">
            Tool calling: {localToolsOk ? 'verified' : 'not verified — run test'}
            {testNotice ? ` — ${testNotice}` : ''}
          </span>
          {testPassed === false ? (
            <span role="status">
              Last test did not pass. Check URL, model name, and server logs.
            </span>
          ) : null}
        </div>
      </div>

      <div
        className="rounded-md border p-3"
        style={{ borderColor: 'var(--gray-6)' }}
      >
        <span className="font-semibold mb-1 block">
          Strategy (main loop)
        </span>
        <span>
          Uses{' '}
          <span className="font-mono">
            {config.strategy_provider ?? 'anthropic'} / {config.strategy_model ?? 'claude-sonnet'}
          </span>{' '}
          from Settings → Agent engine. Cost routing sends read-only tool chains to the local model above.
        </span>
      </div>

      <div className="flex gap-2 items-center flex-wrap">
        <Button size="default" className="cursor-pointer" disabled={saving} onClick={() => void save()}>
          {saving ? 'Saving…' : 'Save local delegate settings'}
        </Button>
      </div>

      {saveNotice ? (
        <div role="status" className="rounded-lg border border-green-500/30 bg-green-500/10 p-3 text-sm text-green-900 dark:text-green-100">{saveNotice}</div>
      ) : null}
    </div>
  )
}
