import { useCallback, useEffect, useState } from 'react'
import type { HookDefinition, PersonaHookSkillPolicy, SkillCatalogEntry } from '../types'

type Api = <T>(path: string, init?: RequestInit) => Promise<T>

export type PolicySaveInput = {
  hooks?: { restrict: boolean; selectedIds: number[] }
  skills?: { restrict: boolean; selectedNames: string[] }
}

function hookUpsertPayload(hook: HookDefinition): Record<string, unknown> {
  return {
    id: hook.id,
    name: hook.name,
    event_name: hook.event_name,
    matcher: hook.matcher ?? null,
    action_type: hook.action_type,
    action_payload_json: hook.action_payload_json,
    scoped_persona_ids: hook.scoped_persona_ids,
    enabled: hook.enabled,
  }
}

export function usePersonaHookSkillPolicy(
  api: Api,
  activePersonaId: number | null,
  onError: (message: string) => void,
) {
  const [hooks, setHooks] = useState<HookDefinition[]>([])
  const [skills, setSkills] = useState<SkillCatalogEntry[]>([])
  const [skillsTotal, setSkillsTotal] = useState(0)
  const [skillsRemoteCount, setSkillsRemoteCount] = useState(0)
  const [policy, setPolicy] = useState<PersonaHookSkillPolicy | null>(null)
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)

  const load = useCallback(async () => {
    setLoading(true)
    try {
      const hooksPath =
        activePersonaId != null ? `/api/hooks?persona_id=${activePersonaId}` : '/api/hooks'
      const hooksRes = await api<{ hooks?: HookDefinition[] }>(hooksPath)
      const hookList = Array.isArray(hooksRes.hooks) ? hooksRes.hooks : []
      setHooks(hookList)

      const skillsPath =
        activePersonaId != null ? `/api/skills?persona_id=${activePersonaId}` : '/api/skills'
      const skillsRes = await api<{
        skills?: SkillCatalogEntry[]
        total?: number
        remote_count?: number
      }>(skillsPath)
      const skillList = Array.isArray(skillsRes.skills) ? skillsRes.skills : []
      setSkills(skillList)
      setSkillsTotal(typeof skillsRes.total === 'number' ? skillsRes.total : skillList.length)
      setSkillsRemoteCount(
        typeof skillsRes.remote_count === 'number'
          ? skillsRes.remote_count
          : skillList.filter((s) => s.remote).length,
      )

      if (activePersonaId != null) {
        const policyRes = await api<PersonaHookSkillPolicy>(
          `/api/personas/${activePersonaId}/policy`,
        )
        setPolicy(policyRes)
      } else {
        setPolicy(null)
      }
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [activePersonaId, api, onError])

  useEffect(() => {
    void load()
  }, [load])

  const savePolicy = useCallback(
    async (input: PolicySaveInput) => {
      if (activePersonaId == null || policy == null) return
      setSaving(true)
      try {
        if (input.hooks?.restrict) {
          const selected = new Set(input.hooks.selectedIds)
          const hookIdsToEnableScope = hooks.filter(
            (hook) =>
              selected.has(hook.id) &&
              hook.scoped_for_persona === false &&
              Array.isArray(hook.scoped_persona_ids),
          )
          for (const hook of hookIdsToEnableScope) {
            if (!Array.isArray(hook.scoped_persona_ids)) continue
            const nextScope = Array.from(
              new Set([...hook.scoped_persona_ids, activePersonaId]),
            ).sort((a, b) => a - b)
            await api('/api/hooks', {
              method: 'POST',
              headers: { 'Content-Type': 'application/json' },
              body: JSON.stringify({
                ...hookUpsertPayload(hook),
                scoped_persona_ids: nextScope,
              }),
            })
          }
        }

        const allowedHookIds = input.hooks
          ? input.hooks.restrict
            ? input.hooks.selectedIds
            : null
          : policy?.uses_default_hooks
            ? null
            : (policy?.allowed_hook_ids ?? null)
        const allowedSkillNames = input.skills
          ? input.skills.restrict
            ? input.skills.selectedNames
            : null
          : policy?.uses_default_skills
            ? null
            : (policy?.allowed_skill_names ?? null)

        await api(`/api/personas/${activePersonaId}/policy`, {
          method: 'PATCH',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            allowed_hook_ids: allowedHookIds,
            allowed_skill_names: allowedSkillNames,
          }),
        })
        await load()
      } catch (e) {
        onError(e instanceof Error ? e.message : String(e))
      } finally {
        setSaving(false)
      }
    },
    [activePersonaId, api, hooks, load, onError, policy],
  )

  return {
    hooks,
    skills,
    skillsTotal,
    skillsRemoteCount,
    policy,
    loading,
    saving,
    load,
    savePolicy,
  }
}
