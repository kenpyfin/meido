import { Component, useCallback, useEffect, useRef, useState, type ReactNode } from 'react'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { ConfirmDialog } from './confirm-dialog'
import { asEngineId, engineLabel, ENGINE_OPTIONS, type EngineId } from '../lib/agent-engine'
import { LedgerJump, LedgerSection, LedgerStatusRow, ServiceNote } from './ledger-chrome'
import { SettingsPanelSkeleton } from './skeleton'
import { LocalModelFields } from './llm-model-fields'
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

async function engineCardLabel(
  api: Api,
  personaId: number | null,
  engine: EngineId,
): Promise<'Ready' | 'Needs setup'> {
  if (personaId == null) return 'Needs setup'
  try {
    const gap = await missingModelNotice(api, personaId, engine)
    if (gap) return 'Needs setup'
    if (engine === 'cursor') {
      const cursor = await api<CursorEngineConfigResponse>(`/api/cursor-engine?persona_id=${personaId}`)
      if (!cursor.engine_ready) return 'Needs setup'
    }
    if (engine === 'gemini_adk') {
      const gemini = await api<GeminiAdkConfigResponse>('/api/gemini-adk')
      if (!gemini.engine_ready) return 'Needs setup'
    }
    return 'Ready'
  } catch {
    return 'Needs setup'
  }
}

type Props = {
  api: <T>(path: string, init?: RequestInit) => Promise<T>
  onError: (message: string) => void
  activePersonaId: number | null
  personas: Persona[]
  onPersonaBulletinChanged?: () => void | Promise<void>
  onOpenConnections?: () => void
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
  onOpenConnections,
}: Props) {
  const [runtime, setRuntime] = useState<RuntimeConfigResponse | null>(null)
  const [cursorStatus, setCursorStatus] = useState<CursorEngineConfigResponse | null>(null)
  const [geminiStatus, setGeminiStatus] = useState<GeminiAdkConfigResponse | null>(null)
  const [personaEngine, setPersonaEngine] = useState<PersonaAgentEngineInfo | null>(null)
  const [loading, setLoading] = useState(true)
  const [savingKey, setSavingKey] = useState<string | null>(null)
  const [saveNotice, setSaveNotice] = useState<string | null>(null)
  const [modelGap, setModelGap] = useState<string | null>(null)
  const [cards, setCards] = useState<Partial<Record<EngineId, 'Ready' | 'Needs setup'>>>({})
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
        const labels = await Promise.all(
          ENGINE_OPTIONS.map(async (opt) => [opt.id, await engineCardLabel(api, activePersonaId, opt.id)] as const),
        )
        setCards(Object.fromEntries(labels))
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
  const cursorMissing = selectedEngine === 'cursor' && cursorStatus != null && !cursorStatus.engine_ready
  const geminiMissing = selectedEngine === 'gemini_adk' && geminiStatus != null && !geminiStatus.engine_ready
  const engineMissing = cursorMissing || geminiMissing
  const engineOwed = activePersonaId == null || engineMissing || modelGap != null
  const cursorDetail = cursorMissing
    ? `${cursorStatus?.sidecar_reachable ? 'Sidecar up' : 'Sidecar down'}, API key ${cursorStatus?.api_key_configured ? 'set' : 'missing'}, health ${cursorStatus?.sdk_runner_ok ? 'verified' : 'not verified'}. Finish setup in the Cursor panel below.`
    : undefined
  const geminiDetail = geminiMissing ? (
    <>
      {`API key ${geminiStatus?.api_key_configured ? 'set' : 'missing'}. `}
      {onOpenConnections ? (
        <LedgerJump onClick={onOpenConnections}>Open Connections</LedgerJump>
      ) : (
        'Add the Gemini key under Settings → Connections'
      )}
      {'. Then finish setup in the Gemini ADK panel below.'}
    </>
  ) : undefined

  const refreshModelGap = useCallback(() => {
    void (async () => {
      try {
        setModelGap(await missingModelNotice(api, activePersonaId, selectedEngine))
      } catch {
        setModelGap(null)
      }
      const labels = await Promise.all(
        ENGINE_OPTIONS.map(async (opt) => [opt.id, await engineCardLabel(api, activePersonaId, opt.id)] as const),
      )
      setCards(Object.fromEntries(labels))
    })()
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

      <LedgerSection title="Agent engine" owed={engineOwed} scope="live" className="flex flex-col gap-2">
        <div className="mc-ledger-rows">
          {activePersonaId == null ? (
            <LedgerStatusRow
              label="Persona"
              state="Missing"
              detail="Select a persona in the sidebar."
            />
          ) : (
            <LedgerStatusRow
              label="Engine"
              state={engineMissing ? 'Missing' : 'Ready'}
              detail={cursorMissing ? cursorDetail : geminiDetail}
            />
          )}
          {modelGap ? <LedgerStatusRow label="Model" state="Missing" detail={modelGap} /> : null}
        </div>
        {activePersonaId == null ? (
          <ServiceNote summary="Select a persona in the sidebar to set its engine.">
            Choosing an engine saves it and shows its settings on the card below.
          </ServiceNote>
        ) : (
          <ServiceNote summary={`Choose an engine for ${personaName}. The choice saves immediately, and the next turn uses it.`}>
            A new persona starts on the engine chosen in the create dialog. The conversation window can change it later. Settings for the chosen engine are on the card below.
          </ServiceNote>
        )}
        <div className="mc-engine-grid">
          {ENGINE_OPTIONS.map((opt) => {
            const label = cards[opt.id] ?? 'Needs setup'
            const ready = label === 'Ready'
            return (
              <button
                key={opt.id}
                type="button"
                disabled={busy || activePersonaId == null}
                aria-pressed={selectedEngine === opt.id}
                className={
                  selectedEngine === opt.id
                    ? 'mc-engine-option mc-engine-option--active'
                    : 'mc-engine-option'
                }
                onClick={() => void patchPersonaEngine(opt.id)}
              >
                <span className="mc-engine-option-name">{opt.label}</span>
                <span className="mc-engine-option-summary">{opt.subtitle}</span>
                <span className={ready ? 'mc-ledger-state mc-ledger-state--settled' : 'mc-ledger-state mc-ledger-state--owed'}>
                  {label}
                </span>
              </button>
            )
          })}
        </div>

        {saveNotice ? (
          <span className="text-xs text-muted-foreground" role="status">{saveNotice}</span>
        ) : null}

      </LedgerSection>

      <div className="mc-engine-config">
        <h3 className="mc-ledger-section-title">{engineLabel(selectedEngine)}</h3>
      <ConfigPanelErrorBoundary key={selectedEngine}>
        {selectedEngine === 'classic' ? (
          <div className="flex flex-col gap-3">
            <SettingsLlmPanel
              api={api}
              onError={onError}
              activePersonaId={activePersonaId}
              includeThinking={false}
              onSaved={refreshModelGap}
            />
            <AdvancedSingleTurnDialog api={api} onError={onError} />
          </div>
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
  const [listedModels, setListedModels] = useState<string[]>([])
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
    if (!listedModels.includes(model.trim())) {
      onError('Pick a model from the list.')
      return
    }
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
    if (!listedModels.includes(model.trim())) {
      onError('Pick a model from the list.')
      return
    }
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
    <LedgerSection title="Local delegate" owed={!ready} scope="live" className="flex flex-col gap-2">
      <div className="mc-ledger-rows">
        <LedgerStatusRow
          label="Delegate"
          state={ready ? 'Ready' : 'Missing'}
          detail={ready ? 'Tool calling is verified.' : 'Run Test after saving.'}
        />
      </div>
      <span className="text-xs text-muted-foreground">
        This persona’s local server. The turn stops until the URL, model, and tool-call check are set.
      </span>
      <LocalModelFields
        api={api}
        baseUrl={baseUrl}
        model={model}
        onBaseUrl={setBaseUrl}
        onModel={setModel}
        onListed={setListedModels}
      />
      <div className="flex gap-2">
        <Button type="button" size="sm" disabled={busy || personaId == null} onClick={() => void save()}>
          {busy ? 'Saving…' : 'Save local delegate'}
        </Button>
        <Button type="button" size="sm" variant="outline" disabled={busy || personaId == null} onClick={() => void test()}>
          Test tool calling
        </Button>
      </div>
      {notice ? <span className="text-xs text-muted-foreground" role="status">{notice}</span> : null}
    </LedgerSection>
  )
}

function AdvancedSingleTurnDialog({
  api,
  onError,
}: {
  api: <T>(path: string, init?: RequestInit) => Promise<T>
  onError: (message: string) => void
}) {
  const [open, setOpen] = useState(false)
  const [discardOpen, setDiscardOpen] = useState(false)
  const [draft, setDraft] = useState('')
  const [saved, setSaved] = useState<number | null>(null)
  const [floor, setFloor] = useState(1)
  const [ceiling, setCeiling] = useState(500)
  const [tokensDraft, setTokensDraft] = useState('')
  const [tokensSaved, setTokensSaved] = useState<number | null>(null)
  const [tokensMin, setTokensMin] = useState(1024)
  const [tokensMax, setTokensMax] = useState(131072)
  const [busy, setBusy] = useState(false)
  const [notice, setNotice] = useState<string | null>(null)

  const load = useCallback(async () => {
    const data = await api<RuntimeConfigResponse>('/api/runtime')
    const current = data.max_tool_iterations ?? 100
    const nextFloor = data.max_tool_iterations_floor ?? 1
    const nextCeiling = data.max_tool_iterations_ceiling ?? 500
    setSaved(current)
    setDraft(String(current))
    setFloor(nextFloor)
    setCeiling(nextCeiling)
    const tokens = data.max_tokens ?? 32768
    setTokensSaved(tokens)
    setTokensDraft(String(tokens))
    setTokensMin(data.max_tokens_min ?? 1024)
    setTokensMax(data.max_tokens_max ?? 131072)
  }, [api])

  useEffect(() => {
    if (!open) return
    setNotice(null)
    void load().catch((e: unknown) => {
      onError(e instanceof Error ? e.message : String(e))
    })
  }, [load, onError, open])

  const parsed = Number.parseInt(draft, 10)
  const valid = Number.isFinite(parsed) && parsed >= floor && parsed <= ceiling
  const turnsDirty = saved != null && parsed !== saved
  const parsedTokens = Number.parseInt(tokensDraft, 10)
  const tokensValid =
    Number.isFinite(parsedTokens) && parsedTokens >= tokensMin && parsedTokens <= tokensMax
  const tokensDirty = tokensSaved != null && parsedTokens !== tokensSaved
  const dirty = turnsDirty || tokensDirty

  function requestClose(next: boolean) {
    if (next) {
      setOpen(true)
      return
    }
    if (dirty) {
      setDiscardOpen(true)
      return
    }
    setOpen(false)
  }

  async function save() {
    if (!valid) {
      onError(`Max turns must be from ${floor} to ${ceiling}.`)
      return
    }
    if (!tokensValid) {
      onError(`Max tokens must be from ${tokensMin} to ${tokensMax}.`)
      return
    }
    setBusy(true)
    setNotice(null)
    try {
      const patch: { max_tool_iterations?: number; max_tokens?: number } = {}
      if (turnsDirty) patch.max_tool_iterations = parsed
      if (tokensDirty) patch.max_tokens = parsedTokens
      const res = await api<RuntimeConfigResponse>('/api/runtime', {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(patch),
      })
      const current = res.max_tool_iterations ?? parsed
      setSaved(current)
      setDraft(String(current))
      if (res.max_tool_iterations_floor != null) setFloor(res.max_tool_iterations_floor)
      if (res.max_tool_iterations_ceiling != null) setCeiling(res.max_tool_iterations_ceiling)
      const tokens = res.max_tokens ?? parsedTokens
      setTokensSaved(tokens)
      setTokensDraft(String(tokens))
      if (res.warnings?.length) onError(res.warnings.join(' '))
      setNotice(res.message ?? 'Saved.')
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  const boundNote =
    floor > 1
      ? `This process keeps the limit at ${floor} or more.`
      : ceiling < 500
        ? `This process keeps the limit at ${ceiling} or less.`
        : 'From 1 to 500. The default is 100.'

  return (
    <>
      <Button type="button" size="sm" variant="outline" onClick={() => setOpen(true)}>
        Advanced
      </Button>
      <Dialog open={open} onOpenChange={requestClose}>
        <DialogContent className="max-w-[420px]">
          <DialogTitle>Advanced</DialogTitle>
          <DialogDescription>
            Max turns and max tokens are shared by every persona, including Cost routing. The next
            turn uses the saved limits. Close does not save.
          </DialogDescription>
          <label className="mc-pipeline-field">
            <span>Max turns</span>
            <Input
              type="number"
              min={floor}
              max={ceiling}
              inputMode="numeric"
              value={draft}
              disabled={busy || saved == null}
              onChange={(e) => setDraft(e.target.value)}
            />
          </label>
          <span className="text-xs text-muted-foreground">
            Tool rounds one turn may take. {boundNote}
          </span>
          <label className="mc-pipeline-field">
            <span>Max tokens</span>
            <Input
              type="number"
              min={tokensMin}
              max={tokensMax}
              inputMode="numeric"
              value={tokensDraft}
              disabled={busy || tokensSaved == null}
              onChange={(e) => setTokensDraft(e.target.value)}
            />
          </label>
          <span className="text-xs text-muted-foreground">
            Output limit for one model reply, thinking included. From{' '}
            {tokensMin.toLocaleString('en-US')} to {tokensMax.toLocaleString('en-US')}. The default
            is 32,768.
          </span>
          <Button
            type="button"
            size="sm"
            disabled={busy || !dirty || !valid || !tokensValid}
            onClick={() => void save()}
          >
            {busy ? 'Saving…' : 'Save'}
          </Button>
          {notice ? (
            <span className="text-xs text-muted-foreground" role="status">
              {notice}
            </span>
          ) : null}
        </DialogContent>
      </Dialog>
      <ConfirmDialog
        open={discardOpen}
        onOpenChange={setDiscardOpen}
        title="Discard changes?"
        description="The unsaved limits will be lost."
        confirmLabel="Discard draft"
        onConfirm={() => {
          setDiscardOpen(false)
          setOpen(false)
        }}
      />
    </>
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
