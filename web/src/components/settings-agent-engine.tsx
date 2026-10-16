import { Component, useCallback, useEffect, useRef, useState, type ReactNode } from 'react'
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
        <div role="status" className="rounded-lg border border-amber-500/30 bg-amber-500/10 p-3 text-sm text-amber-900 dark:text-amber-100">
            Engine knobs failed to render ({this.state.message}). The engine selection above is
            unchanged — pick another engine or reload Settings.
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

  if (loading) {
    return <SettingsPanelSkeleton />
  }

  const selectedEngine = asEngineId(
    personaEngine?.override ?? personaEngine?.effective ?? runtime?.agent_engine,
  )
  const costRoutingSelected = selectedEngine === 'classic_cost_routing'
  const localConfigured = runtime?.local_delegate_configured === true
  const toolsOk = runtime?.local_delegate_tools_ok === true
  const localReady = runtime?.local_delegate_ready === true
  const busy = savingKey != null

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
          <div role="status" className="rounded-lg border border-green-500/30 bg-green-500/10 p-3 text-sm text-green-900 dark:text-green-100">{saveNotice}</div>
        ) : null}

        {costRoutingSelected && !localReady ? (
          <div role="alert" className="rounded-lg border border-amber-500/30 bg-amber-500/10 p-3 text-sm text-amber-900 dark:text-amber-100">
              {!localConfigured
                ? 'Cost routing is selected but no local URL/model is configured. Runs use the cloud model only until you configure Local delegate below.'
                : !toolsOk
                  ? 'Cost routing is selected but local tool calling is not verified. Runs use the cloud model only until you run Test in Local delegate.'
                  : 'Cost routing is selected but the local delegate is not ready. Runs use the cloud model only.'}
            </div>
        ) : null}

        {selectedEngine === 'cursor' && cursorStatus && !cursorStatus.engine_ready ? (
          <div role="status" className="rounded-lg border border-amber-500/30 bg-amber-500/10 p-3 text-sm text-amber-900 dark:text-amber-100">
              Cursor engine is active but not ready ({cursorStatus.sidecar_reachable ? 'sidecar up' : 'sidecar down'}
              , API key {cursorStatus.api_key_configured ? 'ok' : 'missing'}, health{' '}
              {cursorStatus.sdk_runner_ok ? 'verified' : 'not verified'}). Finish setup in the Cursor
              panel below.
            </div>
        ) : null}

        {selectedEngine === 'gemini_adk' && geminiStatus && !geminiStatus.engine_ready ? (
          <div role="status" className="rounded-lg border border-amber-500/30 bg-amber-500/10 p-3 text-sm text-amber-900 dark:text-amber-100">
              Gemini ADK is active but not ready (API key{' '}
              {geminiStatus.api_key_configured ? 'ok' : 'missing'}). Set GEMINI_API_KEY in .env and
              finish setup in the Gemini ADK panel below.
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
          />
        ) : null}
        {selectedEngine === 'classic_cost_routing' ? (
          <div className="flex flex-col gap-4">
            <SettingsLlmPanel
              api={api}
              onError={onError}
              activePersonaId={activePersonaId}
              includeThinking={false}
            />
            <span>
              The local delegate used by cost routing is in Settings → Engine.
            </span>
          </div>
        ) : null}
        {selectedEngine === 'cursor' ? (
          <SettingsCursorPanel
            api={api}
            onError={onError}
            activePersonaId={activePersonaId}
            scope="model"
          />
        ) : null}
        {selectedEngine === 'gemini_adk' ? (
          <SettingsGeminiAdkPanel
            api={api}
            onError={onError}
            activePersonaId={activePersonaId}
            scope="topology"
          />
        ) : null}
      </ConfigPanelErrorBoundary>
    </div>
  )
}
