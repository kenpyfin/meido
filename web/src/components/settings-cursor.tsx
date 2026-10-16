import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { Select, SelectTrigger, SelectContent, SelectItem, SelectValue } from '@/components/ui/select'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { SettingsPanelSkeleton } from './skeleton'
import type {
  CursorEngineConfigResponse,
  CursorModelCatalogEntry,
  CursorModelParam,
  CursorModelParameterDef,
} from '../types'

type Props = {
  api: <T>(path: string, init?: RequestInit) => Promise<T>
  onError: (message: string) => void
  activePersonaId?: number | null
}

function statusColor(ok: boolean | undefined): 'green' | 'orange' | 'gray' {
  if (ok === true) return 'green'
  if (ok === false) return 'orange'
  return 'gray'
}

function parameterLabel(param: CursorModelParameterDef): string {
  const display = param.display_name?.trim()
  if (display) return display
  const id = param.id.toLowerCase()
  if (id === 'thinking' || id === 'reasoning' || id === 'effort') return 'Thinking effort'
  if (id === 'context') return 'Context window'
  if (id === 'fast') return 'Fast mode'
  if (id === 'max_mode' || id === 'maxmode') return 'Max mode'
  return param.id
}

function defaultParamsForModel(model: CursorModelCatalogEntry): CursorModelParam[] {
  const defaultVariant =
    model.variants?.find((variant) => variant.is_default) ?? model.variants?.[0]
  if (defaultVariant?.params?.length) {
    return defaultVariant.params
  }
  return (model.parameters ?? [])
    .map((param) => ({
      id: param.id,
      value: param.values[0]?.value ?? '',
    }))
    .filter((param) => param.value)
}

function normalizeModelParams(
  model: CursorModelCatalogEntry | undefined,
  saved: CursorModelParam[],
): CursorModelParam[] {
  if (!model?.parameters?.length) return []
  const defaults = defaultParamsForModel(model)
  const savedById = new Map(saved.map((param) => [param.id, param.value]))
  return model.parameters.map((param) => {
    const savedValue = savedById.get(param.id)
    const allowed = new Set(param.values.map((value) => value.value))
    if (savedValue && allowed.has(savedValue)) {
      return { id: param.id, value: savedValue }
    }
    const fallback = defaults.find((item) => item.id === param.id)?.value ?? param.values[0]?.value
    return fallback ? { id: param.id, value: fallback } : null
  }).filter((param): param is CursorModelParam => param !== null)
}

function parseModelCatalog(
  models: Array<CursorModelCatalogEntry | string | { id: string }>,
): CursorModelCatalogEntry[] {
  return models
    .map((model) => {
      if (typeof model === 'string') return { id: model }
      if ('parameters' in model || 'variants' in model || 'display_name' in model) {
        return model as CursorModelCatalogEntry
      }
      return { id: model.id }
    })
    .filter((model) => Boolean(model.id))
}

export function SettingsCursorPanel({ api, onError, activePersonaId = null }: Props) {
  const [config, setConfig] = useState<CursorEngineConfigResponse | null>(null)
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [loadingModels, setLoadingModels] = useState(false)
  const [saveNotice, setSaveNotice] = useState<string | null>(null)
  const [modelCatalog, setModelCatalog] = useState<CursorModelCatalogEntry[]>([])
  const [modelsNotice, setModelsNotice] = useState<string | null>(null)
  const [useCustomSdkModel, setUseCustomSdkModel] = useState(false)
  const autoLoadedModelsRef = useRef(false)

  const [sdkModel, setSdkModel] = useState('')
  const [sdkModelParams, setSdkModelParams] = useState<CursorModelParam[]>([])
  const [cliPath, setCliPath] = useState('')
  const [cliModel, setCliModel] = useState('')
  const [cliRunnerUrl, setCliRunnerUrl] = useState('')
  const [timeoutSecs, setTimeoutSecs] = useState('3600')
  const [interactiveTimeoutSecs, setInteractiveTimeoutSecs] = useState('900')
  const [tmuxEnabled, setTmuxEnabled] = useState(true)
  const [mcpToolsEnabled, setMcpToolsEnabled] = useState(true)
  const [mcpExposeSendMessage, setMcpExposeSendMessage] = useState(false)
  const [delegationSlimPrompt, setDelegationSlimPrompt] = useState(true)

  const load = useCallback(async () => {
    setLoading(true)
    setSaveNotice(null)
    try {
      const path =
        activePersonaId != null
          ? `/api/cursor-engine?persona_id=${activePersonaId}`
          : '/api/cursor-engine'
      const data = await api<CursorEngineConfigResponse>(path)
      setConfig(data)
      setSdkModel(data.sdk_model ?? '')
      setSdkModelParams(data.sdk_model_params ?? [])
      setCliPath(data.cli_path ?? '')
      setCliModel(data.cli_model ?? '')
      setCliRunnerUrl(data.cli_runner_url ?? '')
      setTimeoutSecs(String(data.timeout_secs ?? 3600))
      setInteractiveTimeoutSecs(String(data.interactive_timeout_secs ?? 900))
      setTmuxEnabled(data.tmux_enabled ?? true)
      setMcpToolsEnabled(data.mcp_tools_enabled ?? true)
      setMcpExposeSendMessage(data.mcp_expose_send_message ?? false)
      setDelegationSlimPrompt(data.delegation_slim_prompt ?? true)
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
      setConfig(null)
    } finally {
      setLoading(false)
    }
  }, [activePersonaId, api, onError])

  useEffect(() => {
    void load()
  }, [load])

  const engineReady = config?.engine_ready === true
  const sidecarManaged = config?.sidecar_managed === true
  const canLoadModels =
    config?.sidecar_reachable === true && config?.api_key_configured === true

  const selectedModelEntry = useMemo(
    () => modelCatalog.find((model) => model.id === sdkModel),
    [modelCatalog, sdkModel],
  )

  const readinessSummary = useMemo(() => {
    if (!config) return ''
    if (engineReady) return 'Cursor SDK is ready (sidecar auto-started with the bot).'
    if (!config.sidecar_reachable) return 'Sidecar not reachable yet — check bot logs on startup.'
    if (!config.api_key_configured) {
      return 'Add CURSOR_API_KEY to repo-root .env and restart the bot.'
    }
    return 'Sidecar is up; waiting for health verification.'
  }, [config, engineReady])

  const loadModels = useCallback(
    async (opts?: { silent?: boolean }) => {
      if (!canLoadModels) {
        if (!opts?.silent) {
          if (config?.sidecar_reachable !== true) {
            onError('Sidecar is not reachable. Restart the bot and check logs.')
          } else {
            onError(
              'CURSOR_API_KEY is not set on the sidecar host. Add it to repo-root .env and restart the bot.',
            )
          }
        }
        return
      }

      setLoadingModels(true)
      setModelsNotice(null)
      try {
        const res = await api<{ models?: Array<CursorModelCatalogEntry | string | { id: string }> }>(
          '/api/cursor-engine/models',
        )
        const catalog = parseModelCatalog(res.models ?? [])
        if (catalog.length === 0) {
          const msg = 'No models returned from Cursor API.'
          setModelsNotice(msg)
          if (!opts?.silent) onError(msg)
          return
        }
        setModelCatalog(catalog)
        const current = sdkModel.trim()
        const currentEntry = catalog.find((model) => model.id === current)
        if (currentEntry) {
          setUseCustomSdkModel(false)
          setSdkModelParams((prev) => normalizeModelParams(currentEntry, prev))
        } else if (current) {
          setUseCustomSdkModel(true)
        } else if (catalog[0]) {
          setSdkModel(catalog[0].id)
          setSdkModelParams(defaultParamsForModel(catalog[0]))
          setUseCustomSdkModel(false)
        }
        setModelsNotice(`${catalog.length} model${catalog.length === 1 ? '' : 's'} loaded from Cursor.`)
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e)
        setModelsNotice(null)
        if (!opts?.silent) onError(msg)
      } finally {
        setLoadingModels(false)
      }
    },
    [api, canLoadModels, config?.sidecar_reachable, onError, sdkModel],
  )

  useEffect(() => {
    if (!canLoadModels || autoLoadedModelsRef.current) return
    autoLoadedModelsRef.current = true
    void loadModels({ silent: true })
  }, [canLoadModels, loadModels])

  useEffect(() => {
    if (!selectedModelEntry) return
    setSdkModelParams((prev) => normalizeModelParams(selectedModelEntry, prev))
  }, [selectedModelEntry])

  function onSdkModelChange(nextModel: string) {
    setSdkModel(nextModel)
    const entry = modelCatalog.find((model) => model.id === nextModel)
    if (entry) {
      setSdkModelParams(defaultParamsForModel(entry))
    } else {
      setSdkModelParams([])
    }
  }

  function onModelParamChange(paramId: string, value: string) {
    setSdkModelParams((prev) => {
      const next = prev.filter((param) => param.id !== paramId)
      if (value) next.push({ id: paramId, value })
      return next
    })
  }

  async function save() {
    setSaving(true)
    setSaveNotice(null)
    try {
      const secs = Number.parseInt(timeoutSecs, 10)
      const interactiveSecs = Number.parseInt(interactiveTimeoutSecs, 10)
      const res = await api<CursorEngineConfigResponse>('/api/cursor-engine', {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          ...(activePersonaId != null ? { persona_id: activePersonaId } : {}),
          sdk_model: sdkModel,
          sdk_model_params: sdkModelParams,
          cli_path: cliPath,
          cli_model: cliModel,
          cli_runner_url: cliRunnerUrl,
          timeout_secs: Number.isFinite(secs) ? secs : 3600,
          interactive_timeout_secs: Number.isFinite(interactiveSecs) ? interactiveSecs : 900,
          tmux_enabled: tmuxEnabled,
          mcp_tools_enabled: mcpToolsEnabled,
          mcp_expose_send_message: mcpExposeSendMessage,
          delegation_slim_prompt: delegationSlimPrompt,
        }),
      })
      setSaveNotice(res.message ?? 'Saved.')
      setConfig(res)
      setSdkModelParams(res.sdk_model_params ?? sdkModelParams)
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
    } finally {
      setSaving(false)
    }
  }

  if (loading) {
    return <SettingsPanelSkeleton />
  }

  if (!config?.ok) {
    return (
      <span>
        Could not load Cursor configuration.
      </span>
    )
  }

  return (
    <div className="flex flex-col gap-3">
      <div role="status" className="rounded-lg border border-border bg-muted/50 p-3 text-sm">{readinessSummary}</div>

      <div className="flex gap-2 flex-wrap items-center">
        <span>
          Sidecar: {config.sidecar_reachable ? 'reachable' : 'down'}
        </span>
        <span>
          CURSOR_API_KEY: {config.api_key_configured ? 'set' : 'missing in .env'}
        </span>
        <span>
          Runner: {config.sdk_runner_url ?? '—'}
          {sidecarManaged ? ' (managed by bot)' : ''}
        </span>
        <span>
          MCP bridge: {config.mcp_bridge_ready ? 'ready' : 'off or web disabled'}
        </span>
      </div>

      <div className="rounded-md border p-3" style={{ borderColor: 'var(--gray-6)' }}>
        <span className="font-semibold mb-1 block">
          Cursor SDK engine
        </span>
        <span className="mb-2 block">
          SDK model and parameters are saved for this persona. Sidecar URL, MCP, and timeouts stay
          shared. The sidecar starts automatically when the bot starts. The bot installs{' '}
          <code>@cursor/sdk</code> into a runtime Node prefix on first boot (no{' '}
          <code>cursor-sdk-bridge</code> subprocess). The only required setup is{' '}
          <code>CURSOR_API_KEY</code> in repo-root <code>.env</code> (never commit the value).
          Node 20+ and npm must be on PATH.
        </span>
        <div className="flex flex-col gap-2">
          <div className="flex gap-2 flex-wrap items-center">
            {!useCustomSdkModel &&
            modelCatalog.length > 0 &&
            sdkModel &&
            modelCatalog.some((model) => model.id === sdkModel) ? (
              <Select value={sdkModel} onValueChange={onSdkModelChange}>
                <SelectTrigger placeholder="Select model" style={{ flex: 1, minWidth: 160 }} ><SelectValue /></SelectTrigger>
                <SelectContent>
                  {modelCatalog
                    .filter((model) => Boolean(model.id))
                    .map((model) => (
                    <SelectItem key={model.id} value={model.id}>
                      {model.display_name?.trim() ? `${model.display_name} (${model.id})` : model.id}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            ) : (
              <Input
                placeholder="composer-2.5"
                value={sdkModel}
                onChange={(e) => setSdkModel(e.target.value)}
                style={{ flex: 1, minWidth: 160 }}
              />
            )}
            <Button
              size="sm"
              variant="outline"
              disabled={loadingModels || !canLoadModels}
              onClick={() => void loadModels()}
            >
              {loadingModels ? 'Loading…' : 'Refresh models'}
            </Button>
          </div>
          {modelCatalog.length > 0 ? (
            <Button
              size="sm"
              variant="ghost"
              type="button"
              onClick={() => setUseCustomSdkModel((value) => !value)}
            >
              {useCustomSdkModel ? 'Use model list' : 'Type custom model id'}
            </Button>
          ) : null}
          {selectedModelEntry?.parameters && selectedModelEntry.parameters.length > 0 ? (
            <div className="flex flex-col gap-2">
              {selectedModelEntry.parameters.map((param) => {
                const allowed = param.values.map((value) => value.value).filter(Boolean)
                const savedValue = sdkModelParams.find((item) => item.id === param.id)?.value ?? ''
                const currentValue = allowed.includes(savedValue)
                  ? savedValue
                  : (allowed[0] ?? '')
                if (!currentValue) return null
                return (
                  <div className="flex flex-col gap-1" key={param.id}>
                    <span className="font-medium">
                      {parameterLabel(param)}
                    </span>
                    <Select
                      value={currentValue}
                      onValueChange={(value) => onModelParamChange(param.id, value)}
                    >
                      <SelectTrigger placeholder={`Select ${parameterLabel(param).toLowerCase()}`} ><SelectValue /></SelectTrigger>
                      <SelectContent>
                        {param.values
                          .filter((value) => Boolean(value.value))
                          .map((value) => (
                          <SelectItem key={`${param.id}:${value.value}`} value={value.value}>
                            {value.display_name?.trim() || value.value}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </div>
                )
              })}
            </div>
          ) : sdkModel.trim() && canLoadModels ? (
            <span>
              Refresh models to load thinking effort and context window options for the selected model.
            </span>
          ) : null}
          {!canLoadModels ? (
            <span>
              Model list requires a reachable sidecar and <code>CURSOR_API_KEY</code> in{' '}
              <code>.env</code>. Without the key, the sidecar returns HTTP 503.
            </span>
          ) : modelsNotice ? (
            <span role="status">
              {modelsNotice}
            </span>
          ) : null}
        </div>
      </div>

      <div className="rounded-md border p-3" style={{ borderColor: 'var(--gray-6)' }}>
        <span className="font-semibold mb-1 block">
          Bot tools (MCP)
        </span>
        <span className="mb-2 block">
          Exposes the bot ToolRegistry to the Cursor SDK agent via loopback MCP at{' '}
          <code>{config.mcp_endpoint_url ?? '/internal/cursor-mcp'}</code>. Requires Web UI
          enabled. Recursive <code>cursor_agent</code> tools are always denied.
        </span>
        <div className="flex flex-col gap-2">
          <div className="flex items-center justify-between gap-3">
            <span>Expose bot tools to Cursor (MCP)</span>
            <Switch
              size="default"
              checked={mcpToolsEnabled}
              onCheckedChange={setMcpToolsEnabled}
            />
          </div>
          <div className="flex items-center justify-between gap-3">
            <span>Allow send_message via MCP</span>
            <Switch
              size="default"
              checked={mcpExposeSendMessage}
              disabled={!mcpToolsEnabled}
              onCheckedChange={setMcpExposeSendMessage}
            />
          </div>
          <div className="flex items-center justify-between gap-3">
            <span>Slim sidecar prompt (strip tool catalog when MCP on)</span>
            <Switch
              size="default"
              checked={delegationSlimPrompt}
              disabled={!mcpToolsEnabled}
              onCheckedChange={setDelegationSlimPrompt}
            />
          </div>
          <span>
            Interactive chat turns use the shorter timeout below; scheduled/background jobs use the
            CLI timeout field.
          </span>
          <span>Interactive turn timeout (seconds)</span>
          <Input
            placeholder="900"
            value={interactiveTimeoutSecs}
            onChange={(e) => setInteractiveTimeoutSecs(e.target.value)}
          />
        </div>
      </div>

      <div className="rounded-md border p-3" style={{ borderColor: 'var(--gray-6)' }}>
        <span className="font-semibold mb-1 block">
          cursor_agent CLI tool
        </span>
        <span className="mb-2 block">
          Optional settings for the <code>cursor_agent</code> tool (separate from the Cursor engine).
          CLI on PATH: {config.cli_on_path ? 'yes' : 'no'}.
        </span>
        <div className="flex flex-col gap-2">
          <Input
            placeholder="cursor-agent"
            value={cliPath}
            onChange={(e) => setCliPath(e.target.value)}
          />
          <Input
            placeholder="Model override (optional)"
            value={cliModel}
            onChange={(e) => setCliModel(e.target.value)}
          />
          <Input
            placeholder="CLI runner URL (Docker host, optional)"
            value={cliRunnerUrl}
            onChange={(e) => setCliRunnerUrl(e.target.value)}
          />
          <Input
            placeholder="3600"
            value={timeoutSecs}
            onChange={(e) => setTimeoutSecs(e.target.value)}
          />
          <div className="flex items-center justify-between gap-3">
            <span>Tmux detach mode</span>
            <Switch size="default" checked={tmuxEnabled} onCheckedChange={setTmuxEnabled} />
          </div>
        </div>
      </div>

      <div className="flex gap-2 items-center flex-wrap">
        <Button size="default" className="cursor-pointer" disabled={saving} onClick={() => void save()}>
          {saving ? 'Saving…' : 'Save Cursor settings'}
        </Button>
      </div>

      {saveNotice ? (
        <div role="status" className="rounded-lg border border-green-500/30 bg-green-500/10 p-3 text-sm text-green-900 dark:text-green-100">{saveNotice}</div>
      ) : null}
    </div>
  )
}
