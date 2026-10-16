import { Component, useCallback, useEffect, useRef, useState, type ReactNode } from 'react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { asEngineId, engineLabel, ENGINE_OPTIONS, type EngineId } from '../lib/agent-engine'
import { SettingsPanelSkeleton } from './skeleton'
import { SettingsLlmPanel } from './settings-llm'
import { SettingsCursorPanel } from './settings-cursor'
import { SettingsGeminiAdkPanel } from './settings-gemini-adk'
import { SettingsRunContextPanel } from './settings-run-context'
import type {
  CursorEngineConfigResponse,
  GeminiAdkConfigResponse,
  Persona,
  PersonaAgentEngineInfo,
  RuntimeConfigResponse,
} from '../types'

type Api = <T>(path: string, init?: RequestInit) => Promise<T>

async function missingModelNotice(
  api: Api,
  personaId: number | null,
  engine: EngineId,
): Promise<string | null> {
  if (personaId == null) return null
  if (engine === 'classic' || engine === 'classic_cost_routing') {
    const llm = await api<{
      model?: string
      provider?: { id?: string }
      base_url?: string | null
    }>(`/api/llm?persona_id=${personaId}`)
    const provider = llm.provider?.id?.trim() ?? ''
    const model = llm.model?.trim() ?? ''
    if (!provider || !model) {
      return 'Set this persona’s provider and model below before the next turn.'
    }
    const local = provider === 'ollama' || provider === 'llama' || provider === 'llamacpp'
    if (local && !llm.base_url?.trim()) {
      return 'Set this persona’s server URL below before the next turn.'
    }
    if (engine === 'classic_cost_routing') {
      const delegate = await api<{ base_url?: string; model?: string; tools_ok?: boolean }>(
        `/api/personas/local-delegate?persona_id=${personaId}`,
      )
      if (!delegate.base_url?.trim() || !delegate.model?.trim() || delegate.tools_ok !== true) {
        return 'Set this persona’s local URL, model, and tool-call check below before the next turn.'
      }
    }
    return null
  }
  if (engine === 'cursor') {
    const cursor = await api<{ sdk_model?: string }>(`/api/cursor-engine?persona_id=${personaId}`)
    if (!cursor.sdk_model?.trim()) {
      return 'Set this persona’s Cursor model below before the next turn.'
    }
    return null
  }
  if (engine === 'gemini_adk') {
    const topo = await api<{ profile?: { default_model?: string } | null }>(
      `/api/gemini-adk/topology?persona_id=${personaId}`,
    )
    if (!topo.profile?.default_model?.trim()) {
      return 'Set this persona’s Gemini default model below before the next turn.'
    }
    return null
  }
  return null
}

type Props = {
  api: <T>(path: string, init?: RequestInit) => Promise<T>
  onError: (message: string) => void
  activePersonaId: number | null
  personas: Persona[]
  onPersonaBulletinChanged?: () => void | Promise<void>
}

class ConfigPanelErrorBoundary extends Component<{ children: ReactNode }, { message: string | null }> {
  state = { message: null as string | null }

  static getDerivedStateFromError(error: Error) {
    return { message: error.message || 'This settings panel crashed.' }
  }

  render() {
    if (this.state.message) {
      return (
        <div role="status" className="mc-notice-owed">
            Engine knobs failed to render ({this.state.message}). The engine selection above is
            unchanged. Pick another engine or reload the Persona page.
          </div>
      )
    }
    return this.props.children
  }
}

export function SettingsAgentEnginePanel({
  api,
  onError,
  activePersonaId,
  personas,
  onPersonaBulletinChanged,
}: Props) {
  const [runtime, setRuntime] = useState<RuntimeConfigResponse | null>(null)
  const [cursorStatus, setCursorStatus] = useState<CursorEngineConfigResponse | null>(null)
  const [geminiStatus, setGeminiStatus] = useState<GeminiAdkConfigResponse | null>(null)
  const [personaEngine, setPersonaEngine] = useState<PersonaAgentEngineInfo | null>(null)
  const [loading, setLoading] = useState(true)
  const [savingKey, setSavingKey] = useState<string | null>(null)
  const [saveNotice, setSaveNotice] = useState<string | null>(null)
  const [modelGap, setModelGap] = useState<string | null>(null)
  const loadOnceRef = useRef(false)

  const activePersona = personas.find((p) => p.id === activePersonaId) ?? null
  const personaName = activePersona?.name?.trim() || (activePersonaId != null ? `#${activePersonaId}` : '')

  const refreshCursorStatus = useCallback(async () => {
    if (activePersonaId == null) {
      setCursorStatus(null)
      return
    }
    try {
      const cursor = await api<CursorEngineConfigResponse>(
        `/api/cursor-engine?persona_id=${activePersonaId}`,
      )
      setCursorStatus(cursor)
    } catch {
      setCursorStatus(null)
    }
  }, [activePersonaId, api])

  const refreshGeminiStatus = useCallback(async () => {
    try {
      const gemini = await api<GeminiAdkConfigResponse>('/api/gemini-adk')
      setGeminiStatus(gemini)
    } catch {
      setGeminiStatus(null)
    }
  }, [api])

  const load = useCallback(
    async (opts?: { silent?: boolean }) => {
      if (!opts?.silent) setLoading(true)
      try {
        const data = await api<RuntimeConfigResponse>('/api/runtime')
        setRuntime(data)

        let effective = asEngineId(data.agent_engine)
        if (activePersonaId != null) {
          const bulletin = await api<{
            agent_engine?: PersonaAgentEngineInfo | null
            agent_engine_override?: string | null
            agent_engine_global?: string
            agent_engine_effective?: string
          }>(`/api/personas/${activePersonaId}/bulletin`)
          const info: PersonaAgentEngineInfo =
            bulletin.agent_engine && typeof bulletin.agent_engine.effective === 'string'
              ? bulletin.agent_engine
              : {
                  override: bulletin.agent_engine_override ?? null,
                  global: bulletin.agent_engine_global ?? data.agent_engine ?? 'classic',
                  effective:
                    bulletin.agent_engine_override ??
                    bulletin.agent_engine_effective ??
                    'classic',
                  uses_default: false,
                }
          setPersonaEngine(info)
          effective = asEngineId(info.override ?? info.effective)
        } else {
          setPersonaEngine(null)
        }

        if (effective === 'cursor') {
          void refreshCursorStatus()
        } else {
          setCursorStatus(null)
        }
        if (effective === 'gemini_adk') {
          void refreshGeminiStatus()
        } else {
          setGeminiStatus(null)
        }
        try {
          setModelGap(await missingModelNotice(api, activePersonaId, effective))
        } catch {
          setModelGap(null)
        }
      } catch (e) {
        onError(e instanceof Error ? e.message : String(e))
        if (!opts?.silent) {
          setRuntime(null)
          setCursorStatus(null)
          setGeminiStatus(null)
          setPersonaEngine(null)
        }
      } finally {
        setLoading(false)
      }
    },
    [activePersonaId, api, onError, refreshCursorStatus, refreshGeminiStatus],
  )

  useEffect(() => {
    const silent = loadOnceRef.current
    loadOnceRef.current = true
    void load({ silent })
  }, [load])

  async function patchPersonaEngine(engine: EngineId) {
    if (activePersonaId == null) {
      onError('Select a persona in the sidebar to set its agent engine.')
      return
    }
    const previous = personaEngine
    setSavingKey('agent_engine')
    setSaveNotice(null)
    setPersonaEngine({
      override: engine,
      global: asEngineId(runtime?.agent_engine),
      effective: engine,
      uses_default: false,
    })
    try {
      await api(`/api/personas/${activePersonaId}/bulletin`, {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ agent_engine_override: engine }),
      })
      setSaveNotice(`Saved. ${personaName || 'This persona'} uses ${engineLabel(engine)}.`)
      void load({ silent: true })
    } catch (e) {
      setPersonaEngine(previous)
      onError(e instanceof Error ? e.message : String(e))
      await load({ silent: true })
    } finally {
      setSavingKey(null)
    }
  }

  const selectedEngine = asEngineId(
    personaEngine?.override ?? personaEngine?.effective ?? runtime?.agent_engine,
  )
  const busy = savingKey != null

  const refreshModelGap = useCallback(() => {
    void missingModelNotice(api, activePersonaId, selectedEngine)
      .then(setModelGap)
      .catch(() => setModelGap(null))
  }, [activePersonaId, api, selectedEngine])

  if (loading) {
    return <SettingsPanelSkeleton />
  }

  return (
    <div className="flex flex-col gap-4">
      <SettingsRunContextPanel
        api={api}
        onError={onError}
        activePersonaId={activePersonaId}
        personaName={personaName}
        onPersonaBulletinChanged={onPersonaBulletinChanged}
      />

      <div className="flex flex-col gap-2">
        <span className="font-medium">
          Agent engine
        </span>
        {activePersonaId == null ? (
          <span>
            Select a persona in the sidebar to set its engine. Choosing an engine saves it and shows
            its settings below.
          </span>
        ) : (
          <span>
            Engine for <span className="font-medium">{personaName}</span>. Click to save —
            settings for that engine appear below. A new persona starts on the engine chosen in
            the create dialog, and the conversation window can change it later.
          </span>
        )}
        <div className="flex gap-2 flex-wrap">
          {ENGINE_OPTIONS.map((opt) => (
            <button
              key={opt.id}
              type="button"
              disabled={busy || activePersonaId == null}
              className={
                selectedEngine === opt.id
                  ? 'mc-engine-option mc-engine-option--active'
                  : 'mc-engine-option'
              }
              title={opt.subtitle}
              onClick={() => void patchPersonaEngine(opt.id)}
            >
              {opt.label}
            </button>
          ))}
        </div>

        {saveNotice ? (
          <span className="text-xs text-muted-foreground" role="status">{saveNotice}</span>
        ) : null}

        {modelGap ? (
          <div role="status" className="mc-notice-owed">{modelGap}</div>
        ) : null}

        {selectedEngine === 'cursor' && cursorStatus && !cursorStatus.engine_ready ? (
          <div role="status" className="mc-notice-owed">
              Cursor engine is active but not ready ({cursorStatus.sidecar_reachable ? 'sidecar up' : 'sidecar down'}
              , API key {cursorStatus.api_key_configured ? 'ok' : 'missing'}, health{' '}
              {cursorStatus.sdk_runner_ok ? 'verified' : 'not verified'}). Finish setup in the Cursor
              panel below.
            </div>
        ) : null}

        {selectedEngine === 'gemini_adk' && geminiStatus && !geminiStatus.engine_ready ? (
          <div role="status" className="mc-notice-owed">
              Gemini ADK is active but not ready (API key{' '}
              {geminiStatus.api_key_configured ? 'ok' : 'missing'}). Add the Gemini key under
              Settings → Connections, then finish setup in the Gemini ADK panel below.
            </div>
        ) : null}
      </div>

      <ConfigPanelErrorBoundary key={selectedEngine}>
        {selectedEngine === 'classic' ? (
          <SettingsLlmPanel
            api={api}
            onError={onError}
            activePersonaId={activePersonaId}
            includeThinking={false}
            onSaved={refreshModelGap}
          />
        ) : null}
        {selectedEngine === 'classic_cost_routing' ? (
          <div className="flex flex-col gap-4">
            <SettingsLlmPanel
              api={api}
              onError={onError}
              activePersonaId={activePersonaId}
              includeThinking={false}
              onSaved={refreshModelGap}
            />
            <PersonaLocalDelegate
              api={api}
              onError={onError}
              personaId={activePersonaId}
              onSaved={refreshModelGap}
            />
          </div>
        ) : null}
        {selectedEngine === 'cursor' ? (
          <div className="flex flex-col gap-3">
            <SettingsCursorPanel
              api={api}
              onError={onError}
              activePersonaId={activePersonaId}
              scope="model"
              onSaved={refreshModelGap}
            />
            <AdvancedCursorDialog api={api} onError={onError} />
          </div>
        ) : null}
        {selectedEngine === 'gemini_adk' ? (
          <SettingsGeminiAdkPanel
            api={api}
            onError={onError}
            activePersonaId={activePersonaId}
            scope="topology"
            onSaved={refreshModelGap}
          />
        ) : null}
      </ConfigPanelErrorBoundary>
    </div>
  )
}

function PersonaLocalDelegate({
  api,
  onError,
  personaId,
  onSaved,
}: {
  api: <T>(path: string, init?: RequestInit) => Promise<T>
  onError: (message: string) => void
  personaId: number | null
  onSaved?: () => void
}) {
  const [baseUrl, setBaseUrl] = useState('')
  const [model, setModel] = useState('')
  const [toolsOk, setToolsOk] = useState(false)
  const [busy, setBusy] = useState(false)
  const [notice, setNotice] = useState<string | null>(null)

  const load = useCallback(async () => {
    if (personaId == null) return
    try {
      const data = await api<{ base_url?: string; model?: string; tools_ok?: boolean }>(
        `/api/personas/local-delegate?persona_id=${personaId}`,
      )
      setBaseUrl(data.base_url ?? '')
      setModel(data.model ?? '')
      setToolsOk(data.tools_ok === true)
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
    }
  }, [api, onError, personaId])

  useEffect(() => {
    void load()
  }, [load])

  async function save() {
    if (personaId == null) return
    setBusy(true)
    setNotice(null)
    try {
      const data = await api<{ message?: string; tools_ok?: boolean }>('/api/personas/local-delegate', {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ persona_id: personaId, base_url: baseUrl.trim(), model: model.trim() }),
      })
      setToolsOk(data.tools_ok === true)
      setNotice(data.message ?? 'Saved.')
      onSaved?.()
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  async function test() {
    if (personaId == null) return
    setBusy(true)
    setNotice(null)
    try {
      const data = await api<{ message?: string; tools_ok?: boolean }>('/api/multimodel/test', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          tier: 'local',
          base_url: baseUrl.trim(),
          model: model.trim(),
          persona_id: personaId,
        }),
      })
      setToolsOk(data.tools_ok === true)
      setNotice(data.message ?? 'Tool calling verified.')
      onSaved?.()
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
      setToolsOk(false)
    } finally {
      setBusy(false)
    }
  }

  const ready = baseUrl.trim().length > 0 && model.trim().length > 0 && toolsOk
  return (
    <div className="flex flex-col gap-2 rounded-md border p-3" style={{ borderColor: 'var(--mc-border-soft)' }}>
      <span className="font-medium">Local delegate</span>
      <span className="text-xs text-muted-foreground">
        This persona’s local server. The turn stops until the URL, model, and tool-call check are set.
        {ready ? ' Tool calling is verified.' : ' Run Test after saving.'}
      </span>
      <Input value={baseUrl} placeholder="http://127.0.0.1:8080/v1" aria-label="Local server URL" onChange={(e) => setBaseUrl(e.target.value)} />
      <Input value={model} placeholder="Model id" aria-label="Local model" onChange={(e) => setModel(e.target.value)} />
      <div className="flex gap-2">
        <Button type="button" size="sm" disabled={busy || personaId == null} onClick={() => void save()}>
          {busy ? 'Saving…' : 'Save local delegate'}
        </Button>
        <Button type="button" size="sm" variant="outline" disabled={busy || personaId == null} onClick={() => void test()}>
          Test tool calling
        </Button>
      </div>
      {notice ? <span className="text-xs text-muted-foreground" role="status">{notice}</span> : null}
    </div>
  )
}

function AdvancedCursorDialog({
  api,
  onError,
}: {
  api: <T>(path: string, init?: RequestInit) => Promise<T>
  onError: (message: string) => void
}) {
  const [open, setOpen] = useState(false)
  return (
    <>
      <Button type="button" size="sm" variant="outline" onClick={() => setOpen(true)}>
        Advanced Cursor Configuration
      </Button>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="max-w-[720px]">
          <DialogTitle>Advanced Cursor Configuration</DialogTitle>
          <DialogDescription>
            Shared sidecar, CLI, and timeouts. The model this persona runs is saved above. Close does not save.
          </DialogDescription>
          <SettingsCursorPanel api={api} onError={onError} scope="shared" />
        </DialogContent>
      </Dialog>
    </>
  )
}
