import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { Switch } from '@/components/ui/switch'
import { Select } from '@/components/ui/select'
import { useCallback, useEffect, useState } from 'react'
import { SettingsPanelSkeleton } from './skeleton'
import type {
  AdkAgent,
  AdkAgentKind,
  AdkTopologyProfile,
  GeminiAdkConfigResponse,
  GeminiAdkTopologyResponse,
} from '../types'

type Props = {
  api: <T>(path: string, init?: RequestInit) => Promise<T>
  onError: (message: string) => void
  activePersonaId: number | null
}

const AGENT_KINDS: AdkAgentKind[] = ['llm', 'sequential', 'parallel', 'loop']

function emptyAgent(id: string): AdkAgent {
  return {
    id,
    label: id,
    enabled: true,
    kind: 'llm',
    model: '',
    instruction: '',
    allowed_tools: null,
    sub_agents: [],
    output_key: null,
    max_iterations: 40,
  }
}

export function SettingsGeminiAdkPanel({ api, onError, activePersonaId }: Props) {
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [engine, setEngine] = useState<GeminiAdkConfigResponse | null>(null)
  const [profile, setProfile] = useState<AdkTopologyProfile | null>(null)
  const [defaults, setDefaults] = useState<AdkTopologyProfile | null>(null)
  const [usesDefault, setUsesDefault] = useState(true)
  const [saveNotice, setSaveNotice] = useState<string | null>(null)
  const [expandedAgent, setExpandedAgent] = useState<string | null>(null)
  const [defaultModelDraft, setDefaultModelDraft] = useState('')

  const load = useCallback(async () => {
    setLoading(true)
    setSaveNotice(null)
    try {
      const status = await api<GeminiAdkConfigResponse>('/api/gemini-adk')
      setEngine(status)
      setDefaultModelDraft(status.default_model ?? '')

      const q =
        activePersonaId != null
          ? `/api/gemini-adk/topology?persona_id=${activePersonaId}`
          : '/api/gemini-adk/topology'
      const topo = await api<GeminiAdkTopologyResponse>(q)
      if (topo.profile) setProfile(topo.profile)
      if (topo.defaults) setDefaults(topo.defaults)
      setUsesDefault(topo.uses_default === true || activePersonaId == null)
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
      setProfile(null)
    } finally {
      setLoading(false)
    }
  }, [activePersonaId, api, onError])

  useEffect(() => {
    void load()
  }, [load])

  function updateProfile(mutator: (p: AdkTopologyProfile) => AdkTopologyProfile) {
    setProfile((prev) => (prev ? mutator(structuredClone(prev)) : prev))
    if (activePersonaId != null) setUsesDefault(false)
  }

  function updateAgent(index: number, patch: Partial<AdkAgent>) {
    updateProfile((p) => {
      p.agents[index] = { ...p.agents[index], ...patch }
      return p
    })
  }

  async function saveEngineSettings() {
    setSaving(true)
    setSaveNotice(null)
    try {
      const res = await api<GeminiAdkConfigResponse>('/api/gemini-adk', {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ default_model: defaultModelDraft }),
      })
      setEngine(res)
      setSaveNotice(res.message ?? 'Engine settings saved.')
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
    } finally {
      setSaving(false)
    }
  }

  async function saveTopology(opts?: {
    resetDefaults?: boolean
    clearPersonaOverride?: boolean
  }) {
    if (activePersonaId == null && !opts?.resetDefaults && !profile) {
      onError('Select a persona to edit topology, or reset the global default.')
      return
    }
    setSaving(true)
    setSaveNotice(null)
    try {
      const body: Record<string, unknown> = {}
      if (activePersonaId != null) body.persona_id = activePersonaId
      if (opts?.clearPersonaOverride) {
        body.clear_persona_override = true
      } else if (opts?.resetDefaults) {
        body.reset_defaults = true
      } else {
        body.profile = profile
      }
      const res = await api<GeminiAdkTopologyResponse>('/api/gemini-adk/topology', {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      })
      if (res.profile) setProfile(res.profile)
      setUsesDefault(res.uses_default === true)
      setSaveNotice(res.message ?? 'Topology saved.')
      await load()
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
    } finally {
      setSaving(false)
    }
  }

  if (loading) {
    return <SettingsPanelSkeleton />
  }

  if (!profile) {
    return <span>Could not load Gemini ADK topology.</span>
  }

  const agentIds = profile.agents.map((a) => a.id)

  return (
    <div className="flex flex-col gap-4">
      {engine && !engine.api_key_configured ? (
        <div role="status" className="rounded-lg border border-amber-500/30 bg-amber-500/10 p-3 text-sm text-amber-900 dark:text-amber-100">
            GEMINI_API_KEY / GOOGLE_API_KEY is not set in .env. The Gemini ADK engine cannot run
            until a key is configured.
          </div>
      ) : null}

      {saveNotice ? (
        <div role="status" className="rounded-lg border border-green-500/30 bg-green-500/10 p-3 text-sm text-green-900 dark:text-green-100">{saveNotice}</div>
      ) : null}

      <section className="mc-pipeline-section">
        <span className="font-semibold mb-2 block">
          Engine
        </span>
        <div className="flex gap-2 flex-wrap items-end">
          <label className="mc-pipeline-field">
            <span>Default Gemini model</span>
            <Input
              size="default"
              value={defaultModelDraft}
              onChange={(e) => setDefaultModelDraft(e.target.value)}
              placeholder="gemini-2.5-flash"
            />
          </label>
          <Button size="default" disabled={saving} onClick={() => void saveEngineSettings()}>
            Save model
          </Button>
        </div>
        <span className="mt-2 block">
          Max agent iterations (engine): {engine?.max_iterations ?? '—'} · Schema v
          {engine?.schema_version ?? profile.version}
        </span>
      </section>

      <section className="mc-pipeline-section">
        <span className="font-semibold mb-2 block">
          Topology {activePersonaId != null ? '(this persona)' : '(global default)'}
        </span>
        <span className="mb-3 block">
          {activePersonaId == null
            ? 'Select a persona to edit a per-persona topology. Saving without a persona updates the global default.'
            : usesDefault
              ? 'This persona inherits the global topology. Saving creates a persona override.'
              : 'This persona has a custom topology override.'}
        </span>

        <div className="flex gap-2 flex-wrap mb-3">
          <Button size="default" disabled={saving} onClick={() => void saveTopology()}>
            {saving ? 'Saving…' : 'Save topology'}
          </Button>
          <Button
            size="default"
            variant="secondary"
            disabled={saving}
            onClick={() => void saveTopology({ resetDefaults: true })}
          >
            Reset to defaults
          </Button>
          {activePersonaId != null && !usesDefault ? (
            <Button
              size="default"
              variant="secondary"
              disabled={saving}
              onClick={() => void saveTopology({ clearPersonaOverride: true })}
            >
              Clear persona override
            </Button>
          ) : null}
          <Button
            size="default"
            variant="ghost"
            disabled={saving}
            onClick={() => {
              const id = `agent_${profile.agents.length + 1}`
              updateProfile((p) => {
                p.agents.push(emptyAgent(id))
                return p
              })
              setExpandedAgent(id)
            }}
          >
            Add agent
          </Button>
        </div>

        <label className="mc-pipeline-field mb-3">
          <span>Root agent</span>
          <select
            className="mc-pipeline-select"
            value={profile.root_agent_id}
            onChange={(e) => updateProfile((p) => ({ ...p, root_agent_id: e.target.value }))}
          >
            {agentIds.map((id) => (
              <option key={id} value={id}>
                {id}
              </option>
            ))}
          </select>
        </label>

        <label className="mc-pipeline-field mb-3">
          <span>Topology default model</span>
          <Input
            size="sm"
            value={profile.default_model}
            onChange={(e) =>
              updateProfile((p) => ({ ...p, default_model: e.target.value }))
            }
          />
        </label>

        <div className="flex flex-col gap-3">
          {profile.agents.map((agent, index) => (
            <div key={agent.id} className="mc-pipeline-phase-card">
              <div className="flex items-center justify-between gap-2 flex-wrap">
                <div className="flex items-center gap-2">
                  <Switch
                    size="sm"
                    checked={agent.enabled}
                    onCheckedChange={(checked) => updateAgent(index, { enabled: checked })}
                  />
                  <span className="font-medium">
                    {agent.label || agent.id}
                  </span>
                  <span>
                    ({agent.kind})
                  </span>
                </div>
                <div className="flex gap-2">
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={() =>
                      setExpandedAgent((cur) => (cur === agent.id ? null : agent.id))
                    }
                  >
                    {expandedAgent === agent.id ? 'Collapse' : 'Expand'}
                  </Button>
                  <Button
                    size="sm"
                    variant="destructive"
                    disabled={profile.agents.length <= 1}
                    onClick={() =>
                      updateProfile((p) => {
                        p.agents = p.agents.filter((_, i) => i !== index)
                        if (p.root_agent_id === agent.id && p.agents[0]) {
                          p.root_agent_id = p.agents[0].id
                        }
                        return p
                      })
                    }
                  >
                    Remove
                  </Button>
                </div>
              </div>

              {expandedAgent === agent.id ? (
                <div className="flex flex-col gap-2 mt-3">
                  <div className="flex gap-2 flex-wrap">
                    <label className="mc-pipeline-field">
                      <span>Id</span>
                      <Input
                        size="sm"
                        value={agent.id}
                        onChange={(e) => updateAgent(index, { id: e.target.value })}
                      />
                    </label>
                    <label className="mc-pipeline-field">
                      <span>Label</span>
                      <Input
                        size="sm"
                        value={agent.label}
                        onChange={(e) => updateAgent(index, { label: e.target.value })}
                      />
                    </label>
                    <label className="mc-pipeline-field">
                      <span>Kind</span>
                      <select
                        className="mc-pipeline-select"
                        value={agent.kind}
                        onChange={(e) =>
                          updateAgent(index, { kind: e.target.value as AdkAgentKind })
                        }
                      >
                        {AGENT_KINDS.map((k) => (
                          <option key={k} value={k}>
                            {k}
                          </option>
                        ))}
                      </select>
                    </label>
                    <label className="mc-pipeline-field">
                      <span>Model (empty = topology default)</span>
                      <Input
                        size="sm"
                        value={agent.model}
                        onChange={(e) => updateAgent(index, { model: e.target.value })}
                      />
                    </label>
                    <label className="mc-pipeline-field">
                      <span>Max iterations</span>
                      <Input
                        size="sm"
                        type="number"
                        value={String(agent.max_iterations ?? '')}
                        onChange={(e) =>
                          updateAgent(index, {
                            max_iterations: e.target.value
                              ? Number(e.target.value)
                              : null,
                          })
                        }
                      />
                    </label>
                    <label className="mc-pipeline-field">
                      <span>Output key</span>
                      <Input
                        size="sm"
                        value={agent.output_key ?? ''}
                        onChange={(e) =>
                          updateAgent(index, {
                            output_key: e.target.value.trim() || null,
                          })
                        }
                      />
                    </label>
                  </div>
                  <label className="mc-pipeline-field">
                    <span>
                      Sub-agents (comma-separated ids)
                    </span>
                    <Input
                      size="sm"
                      value={agent.sub_agents.join(', ')}
                      onChange={(e) =>
                        updateAgent(index, {
                          sub_agents: e.target.value
                            .split(',')
                            .map((s) => s.trim())
                            .filter(Boolean),
                        })
                      }
                    />
                  </label>
                  <label className="mc-pipeline-field">
                    <span>Instruction</span>
                    <Textarea
                      rows={4}
                      value={agent.instruction}
                      onChange={(e) => updateAgent(index, { instruction: e.target.value })}
                    />
                  </label>
                </div>
              ) : null}
            </div>
          ))}
        </div>
      </section>

      {defaults ? (
        <span>
          Defaults available via Reset to defaults. Schema version {profile.version}.
        </span>
      ) : null}
    </div>
  )
}
