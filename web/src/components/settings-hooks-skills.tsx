import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { Checkbox } from '@/components/ui/checkbox'
import { Badge } from '@/components/ui/badge'
import { Select, SelectTrigger, SelectContent, SelectItem, SelectValue } from '@/components/ui/select'
import { useEffect, useMemo, useState, type ReactNode } from 'react'
import { SettingsPanelSkeleton } from './skeleton'
import { usePersonaHookSkillPolicy } from '../hooks/use-persona-hook-skill-policy'
import type { HookDefinition, SkillCatalogEntry } from '../types'

const HOOK_LIFECYCLE_EVENTS = [
  'BeforeTurn',
  'PreToolUse',
  'PostToolUse',
  'PostToolBatch',
  'PreStop',
  'PreDelivery',
  'PostDelivery',
] as const

type HookLifecycleEvent = (typeof HOOK_LIFECYCLE_EVENTS)[number]

const HOOK_EVENT_HINTS: Record<HookLifecycleEvent, string> = {
  BeforeTurn: 'Before the agent loop starts',
  PreToolUse: 'Before each tool call',
  PostToolUse: 'After each tool call',
  PostToolBatch: 'After a batch of tool calls',
  PreStop: 'Before the agent stops',
  PreDelivery: 'After PDQE, before persist/send (dense delivery spill)',
  PostDelivery: 'Before PDQE / focus-sync (despite the name)',
}

function hookUpsertPayload(
  hook: HookDefinition,
  overrides: { event_name?: string } = {},
): Record<string, unknown> {
  return {
    id: hook.id,
    name: hook.name,
    event_name: overrides.event_name ?? hook.event_name,
    matcher: hook.matcher ?? null,
    action_type: hook.action_type,
    action_payload_json: hook.action_payload_json,
    scoped_persona_ids: hook.scoped_persona_ids,
    enabled: hook.enabled,
  }
}

type Props = {
  api: <T>(path: string, init?: RequestInit) => Promise<T>
  onError: (message: string) => void
  activePersonaId: number | null
}

function setsEqual<T>(a: Set<T>, b: Set<T>): boolean {
  if (a.size !== b.size) return false
  for (const item of a) {
    if (!b.has(item)) return false
  }
  return true
}

function hookAvailableForPersona(hook: HookDefinition): boolean {
  if (hook.scoped_for_persona === false) return false
  if (hook.allowed_for_persona === false) return false
  return true
}

function skillAvailableForPersona(skill: SkillCatalogEntry): boolean {
  return skill.allowed_for_persona !== false
}

function hookScopeLabel(hook: HookDefinition): string {
  if (hook.is_global || hook.scoped_persona_ids == null) return 'Global'
  if (hook.scoped_persona_ids.length === 0) return 'No personas'
  return hook.scoped_persona_ids.map((id) => `#${id}`).join(', ')
}

function hookIsGlobal(hook: HookDefinition): boolean {
  return hook.is_global || hook.scoped_persona_ids == null
}

function hookStatusLabel(
  hook: HookDefinition,
  activePersonaId: number | null,
  restrictHooks: boolean,
  selectedHookIds: Set<number>,
): { text: string; color: 'gray' | 'green' | 'orange' | 'red' } | null {
  if (activePersonaId == null) return null
  if (!hook.enabled) return { text: 'Disabled', color: 'gray' }
  if (hook.scoped_for_persona === false) {
    return restrictHooks && selectedHookIds.has(hook.id)
      ? { text: 'Pending scope on save', color: 'orange' }
      : { text: 'Wrong persona scope', color: 'red' }
  }
  if (restrictHooks && !selectedHookIds.has(hook.id)) {
    return { text: 'Not in allowlist', color: 'orange' }
  }
  if (!restrictHooks && hook.allowed_for_persona === false) {
    return { text: 'Blocked by policy', color: 'red' }
  }
  if (hook.active_for_persona) return { text: 'Active', color: 'green' }
  if (restrictHooks && selectedHookIds.has(hook.id)) {
    return { text: 'Allowed', color: 'green' }
  }
  return { text: 'Available', color: 'green' }
}

function skillStatusLabel(
  skill: SkillCatalogEntry,
  activePersonaId: number | null,
  restrictSkills: boolean,
  selectedSkillNames: Set<string>,
): { text: string; color: 'green' | 'orange' | 'red' } | null {
  if (activePersonaId == null) return null
  if (!skillAvailableForPersona(skill)) return { text: 'Blocked by policy', color: 'red' }
  if (restrictSkills && !selectedSkillNames.has(skill.name)) {
    return { text: 'Not in allowlist', color: 'orange' }
  }
  if (restrictSkills && selectedSkillNames.has(skill.name)) {
    return { text: 'Allowed', color: 'green' }
  }
  return { text: 'Available', color: 'green' }
}

function statusBucket(color: string | undefined): 'allowed' | 'blocked' | null {
  if (!color) return null
  return color === 'green' ? 'allowed' : 'blocked'
}

const STATUS_BADGE_CLASS: Record<'gray' | 'green' | 'orange' | 'red' | 'blue', string> = {
  gray: 'border-border text-muted-foreground',
  green: 'border-[color:var(--mc-border-soft)] text-[color:var(--mc-text-muted)]',
  orange: 'border-[color:color-mix(in_srgb,var(--mc-accent-brass)_45%,transparent)] text-[color:var(--mc-accent-brass)]',
  red: 'border-destructive/40 text-destructive',
  blue: 'border-[color:color-mix(in_srgb,var(--mc-accent-brass)_45%,transparent)] text-[color:var(--mc-accent-brass)]',
}

function StatusBadge({
  text,
  color,
}: {
  text: string
  color: keyof typeof STATUS_BADGE_CLASS
}) {
  return (
    <Badge variant="outline" className={STATUS_BADGE_CLASS[color]}>
      {text}
    </Badge>
  )
}

function hookPayloadSummary(hook: HookDefinition): string | null {
  const payload = hook.action_payload ?? {}
  const action = hook.action_type.toLowerCase()
  if (action === 'command' && typeof payload.command === 'string') {
    return `command: ${payload.command}`
  }
  if (action === 'prompt' && typeof payload.prompt === 'string') {
    const preview =
      payload.prompt.length > 120 ? `${payload.prompt.slice(0, 120)}…` : payload.prompt
    return `prompt: ${preview}`
  }
  if (action === 'add_context' && typeof payload.additional_context === 'string') {
    const preview =
      payload.additional_context.length > 120
        ? `${payload.additional_context.slice(0, 120)}…`
        : payload.additional_context
    return `context: ${preview}`
  }
  if (action === 'block' && typeof payload.reason === 'string') {
    return `reason: ${payload.reason}`
  }
  if (action.startsWith('builtin_')) {
    return 'Built-in Rust handler'
  }
  return null
}

function formatUpdatedAt(value?: string): string | null {
  if (!value?.trim()) return null
  const parsed = Date.parse(value)
  if (Number.isNaN(parsed)) return value
  return new Date(parsed).toLocaleString()
}

function matchesFilter(text: string, query: string): boolean {
  return text.toLowerCase().includes(query)
}

function hookSearchText(hook: HookDefinition): string {
  const payload = hookPayloadSummary(hook) ?? ''
  return [
    hook.name,
    hook.event_name,
    hook.action_type,
    hook.matcher ?? '',
    payload,
    hookScopeLabel(hook),
  ].join(' ')
}

function skillSearchText(skill: SkillCatalogEntry): string {
  return [
    skill.name,
    skill.description,
    skill.when_to_use ?? '',
    skill.source ?? '',
    (skill.platforms ?? []).join(' '),
    (skill.deps ?? []).join(' '),
  ].join(' ')
}

function FilterBar({
  search,
  onSearch,
  searchPlaceholder,
  filters,
  onClear,
  clearDisabled,
}: {
  search: string
  onSearch: (value: string) => void
  searchPlaceholder: string
  filters: ReactNode
  onClear: () => void
  clearDisabled: boolean
}) {
  return (
    <div className="flex flex-col gap-2">
      <Input
        value={search}
        placeholder={searchPlaceholder}
        onChange={(e) => onSearch(e.target.value)}
      />
      <div className="flex flex-wrap items-center gap-2">
        {filters}
        <Button size="sm" variant="secondary" type="button" disabled={clearDisabled} onClick={onClear}>
          Clear filters
        </Button>
      </div>
    </div>
  )
}

function FilterSelect({
  label,
  value,
  onChange,
  options,
}: {
  label: string
  value: string
  onChange: (value: string) => void
  options: { value: string; label: string }[]
}) {
  return (
    <Select value={value} onValueChange={onChange}>
      <SelectTrigger className="w-[180px]" aria-label={label}>
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        {options.map((option) => (
          <SelectItem key={option.value} value={option.value}>
            {option.label}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  )
}

export function SettingsHooksPanel({ api, onError, activePersonaId }: Props) {
  const { hooks, policy, loading, saving, load, savePolicy } = usePersonaHookSkillPolicy(
    api,
    activePersonaId,
    onError,
  )
  const [restrictHooks, setRestrictHooks] = useState(false)
  const [selectedHookIds, setSelectedHookIds] = useState<Set<number>>(() => new Set())
  const [search, setSearch] = useState('')
  const [eventFilter, setEventFilter] = useState('all')
  const [scopeFilter, setScopeFilter] = useState('all')
  const [statusFilter, setStatusFilter] = useState('all')
  const [showAllPersonas, setShowAllPersonas] = useState(false)
  const [hookEventDrafts, setHookEventDrafts] = useState<Record<number, string>>({})
  const [savingHookId, setSavingHookId] = useState<number | null>(null)

  useEffect(() => {
    const hookRestrict = policy != null && !policy.uses_default_hooks
    setRestrictHooks(hookRestrict)
    setSelectedHookIds(
      new Set(
        hookRestrict && Array.isArray(policy?.allowed_hook_ids)
          ? policy.allowed_hook_ids
          : hooks.map((h) => h.id),
      ),
    )
    setHookEventDrafts({})
  }, [hooks, policy])

  useEffect(() => {
    setShowAllPersonas(false)
    setSearch('')
    setEventFilter('all')
    setScopeFilter('all')
    setStatusFilter('all')
    setHookEventDrafts({})
  }, [activePersonaId])

  function hookEventValue(hook: HookDefinition): string {
    return hookEventDrafts[hook.id] ?? hook.event_name
  }

  function hookEventDirty(hook: HookDefinition): boolean {
    return hookEventValue(hook) !== hook.event_name
  }

  function setHookEventDraft(hookId: number, eventName: string) {
    setHookEventDrafts((prev) => ({ ...prev, [hookId]: eventName }))
  }

  function revertHookEventDraft(hookId: number) {
    setHookEventDrafts((prev) => {
      const next = { ...prev }
      delete next[hookId]
      return next
    })
  }

  const personaFilteredHooks = useMemo(() => {
    if (activePersonaId == null || showAllPersonas) return hooks
    return hooks.filter(hookAvailableForPersona)
  }, [activePersonaId, hooks, showAllPersonas])

  const filteredHooks = useMemo(() => {
    const q = search.trim().toLowerCase()
    return personaFilteredHooks.filter((hook) => {
      if (q && !matchesFilter(hookSearchText(hook), q)) return false
      if (eventFilter !== 'all' && hook.event_name !== eventFilter) return false
      if (scopeFilter === 'global' && !hookIsGlobal(hook)) return false
      if (scopeFilter === 'persona' && hookIsGlobal(hook)) return false
      if (statusFilter !== 'all') {
        const bucket = statusBucket(
          hookStatusLabel(hook, activePersonaId, restrictHooks, selectedHookIds)?.color,
        )
        if (bucket !== statusFilter) return false
      }
      return true
    })
  }, [
    activePersonaId,
    eventFilter,
    personaFilteredHooks,
    restrictHooks,
    scopeFilter,
    search,
    selectedHookIds,
    statusFilter,
  ])

  const hooksAvailableCount = useMemo(
    () => (activePersonaId == null ? hooks.length : hooks.filter(hookAvailableForPersona).length),
    [activePersonaId, hooks],
  )

  const policyDirty = useMemo(() => {
    if (activePersonaId == null || policy == null) return false
    const savedHookRestrict = !policy.uses_default_hooks
    if (restrictHooks !== savedHookRestrict) return true
    const savedHookIds = new Set(
      savedHookRestrict && Array.isArray(policy.allowed_hook_ids)
        ? policy.allowed_hook_ids
        : hooks.map((h) => h.id),
    )
    return !setsEqual(selectedHookIds, savedHookIds)
  }, [activePersonaId, hooks, policy, restrictHooks, selectedHookIds])

  function toggleHookId(id: number, checked: boolean) {
    setSelectedHookIds((prev) => {
      const next = new Set(prev)
      if (checked) next.add(id)
      else next.delete(id)
      return next
    })
  }

  function setAllHookIds(checked: boolean) {
    setSelectedHookIds(checked ? new Set(hooks.map((h) => h.id)) : new Set())
  }

  async function saveHookEvent(hook: HookDefinition) {
    const eventName = hookEventValue(hook)
    if (!HOOK_LIFECYCLE_EVENTS.includes(eventName as HookLifecycleEvent)) {
      onError(`Invalid hook event: ${eventName}`)
      return
    }
    setSavingHookId(hook.id)
    try {
      await api('/api/hooks', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(hookUpsertPayload(hook, { event_name: eventName })),
      })
      await load()
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
    } finally {
      setSavingHookId(null)
    }
  }

  if (loading) {
    return <SettingsPanelSkeleton />
  }

  const filtersActive =
    search.trim() !== '' || eventFilter !== 'all' || scopeFilter !== 'all' || statusFilter !== 'all'

  return (
    <div className="flex flex-col gap-3">
      <span className="font-semibold">
        Hook policy {activePersonaId != null ? `(persona #${activePersonaId})` : ''}
      </span>
      {activePersonaId == null ? (
        <span>Select a persona to edit per-persona hook availability.</span>
      ) : (
        <div className="flex flex-col gap-3">
          <span>
            Default is allow-all. Turn on restriction, pick hooks, then save. Clear all and save to
            block every hook. Hook creation and enable/disable are handled by the agent via the{' '}
            <code>create-hook</code> skill. Saving here does not change the skill allowlist.
          </span>
          <div className="flex flex-col gap-1">
            <div className="flex items-center justify-between gap-2 flex-wrap">
              <label>
                <div className="flex items-center gap-2">
                  <Switch
                    size="sm"
                    checked={restrictHooks}
                    disabled={saving}
                    onCheckedChange={(checked) => {
                      setRestrictHooks(checked)
                      if (checked) setSelectedHookIds(new Set(hooks.map((h) => h.id)))
                    }}
                  />
                  Restrict hooks to selected
                </div>
              </label>
              {restrictHooks ? (
                <div className="flex gap-2">
                  <Button
                    size="sm"
                    variant="secondary"
                    disabled={saving || hooks.length === 0}
                    onClick={() => setAllHookIds(true)}
                  >
                    Select all hooks
                  </Button>
                  <Button size="sm" variant="secondary" disabled={saving} onClick={() => setAllHookIds(false)}>
                    Clear hooks
                  </Button>
                </div>
              ) : null}
            </div>
            {restrictHooks ? (
              <span>
                {selectedHookIds.size} of {hooks.length} hooks allowed
                {hooks.length === 0 ? ' — create hooks via agent first' : ''}
              </span>
            ) : (
              <span>All hooks allowed for this persona.</span>
            )}
          </div>
          <div className="flex gap-2 flex-wrap">
            <Button
              size="sm"
              disabled={saving || (!policyDirty && !restrictHooks)}
              onClick={() =>
                void savePolicy({
                  hooks: { restrict: restrictHooks, selectedIds: Array.from(selectedHookIds) },
                })
              }
            >
              Save hook policy
            </Button>
            <Button
              size="sm"
              variant="secondary"
              disabled={saving}
              onClick={() => void savePolicy({ hooks: { restrict: false, selectedIds: [] } })}
            >
              Allow all hooks
            </Button>
          </div>
          <span>
            Saved: hooks {policy?.uses_default_hooks ? 'allow-all' : 'restricted'}.
            {policyDirty ? ' Unsaved changes.' : ''}
          </span>
        </div>
      )}

      {activePersonaId != null ? (
        <label>
          <div className="flex items-center gap-2">
            <Checkbox
              checked={showAllPersonas}
              disabled={saving}
              onCheckedChange={(checked) => setShowAllPersonas(checked === true)}
            />
            Show all personas (include hooks unavailable for this persona)
          </div>
        </label>
      ) : null}

      <span className="font-semibold">Hooks catalog</span>
      <span>
        {activePersonaId != null
          ? showAllPersonas
            ? `Showing ${filteredHooks.length} of ${hooks.length} hooks (${hooksAvailableCount} available for persona #${activePersonaId})`
            : `Showing ${filteredHooks.length} of ${personaFilteredHooks.length} hooks available for persona #${activePersonaId}`
          : `Showing ${filteredHooks.length} of ${hooks.length} hooks — select a persona to filter by availability`}
        {activePersonaId != null && restrictHooks ? ' · checkboxes when hook restriction is on' : ''}
        . Change lifecycle event per hook below; other fields are still managed via the agent.
      </span>
      <FilterBar
        search={search}
        onSearch={setSearch}
        searchPlaceholder="Filter hooks by name, event, action, matcher, or scope"
        clearDisabled={!filtersActive}
        onClear={() => {
          setSearch('')
          setEventFilter('all')
          setScopeFilter('all')
          setStatusFilter('all')
        }}
        filters={
          <>
            <FilterSelect
              label="Hook event"
              value={eventFilter}
              onChange={setEventFilter}
              options={[
                { value: 'all', label: 'All events' },
                ...HOOK_LIFECYCLE_EVENTS.map((event) => ({ value: event, label: event })),
              ]}
            />
            <FilterSelect
              label="Hook scope"
              value={scopeFilter}
              onChange={setScopeFilter}
              options={[
                { value: 'all', label: 'All scopes' },
                { value: 'global', label: 'Global' },
                { value: 'persona', label: 'Persona-scoped' },
              ]}
            />
            <FilterSelect
              label="Hook status"
              value={statusFilter}
              onChange={setStatusFilter}
              options={[
                { value: 'all', label: 'All statuses' },
                { value: 'allowed', label: 'Allowed' },
                { value: 'blocked', label: 'Blocked' },
              ]}
            />
          </>
        }
      />
      {hooks.length === 0 ? (
        <span>No hooks defined yet.</span>
      ) : filteredHooks.length === 0 ? (
        <span>No hooks match the current filter.</span>
      ) : (
        <div className="flex flex-col gap-2 max-h-[420px] overflow-y-auto">
          {filteredHooks.map((hook) => {
            const payloadSummary = hookPayloadSummary(hook)
            const status = hookStatusLabel(hook, activePersonaId, restrictHooks, selectedHookIds)
            const updated = formatUpdatedAt(hook.updated_at)
            const eventValue = hookEventValue(hook)
            const eventDirty = hookEventDirty(hook)
            const hookSaving = savingHookId === hook.id
            return (
              <div className="flex items-start gap-2 rounded-md border border-[var(--gray-a6)] p-2" key={hook.id}>
                {activePersonaId != null && restrictHooks ? (
                  <Checkbox
                    className="mt-0.5"
                    checked={selectedHookIds.has(hook.id)}
                    disabled={saving}
                    onCheckedChange={(checked) => toggleHookId(hook.id, checked === true)}
                  />
                ) : null}
                <div className="flex flex-col gap-1 min-w-0 flex-1">
                  <div className="flex items-center gap-2 flex-wrap">
                    <span className="font-medium">
                      #{hook.id} {hook.name}
                    </span>
                    {status ? <StatusBadge text={status.text} color={status.color} /> : null}
                    {!hook.enabled ? <Badge variant="outline">Off</Badge> : null}
                    <Badge variant="outline">{hook.is_global ? 'Global scope' : 'Persona scope'}</Badge>
                  </div>
                  <div className="flex items-center gap-2 flex-wrap">
                    <span className="font-medium">Event</span>
                    <Select
                      value={eventValue}
                      disabled={saving || hookSaving}
                      onValueChange={(value) => setHookEventDraft(hook.id, value)}
                    >
                      <SelectTrigger className="min-w-[10rem]">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {HOOK_LIFECYCLE_EVENTS.map((event) => (
                          <SelectItem key={event} value={event} title={HOOK_EVENT_HINTS[event]}>
                            {event}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                    {eventDirty ? (
                      <>
                        <Button size="sm" disabled={saving || hookSaving} onClick={() => void saveHookEvent(hook)}>
                          {hookSaving ? 'Saving…' : 'Save event'}
                        </Button>
                        <Button
                          size="sm"
                          variant="secondary"
                          disabled={saving || hookSaving}
                          onClick={() => revertHookEventDraft(hook.id)}
                        >
                          Revert
                        </Button>
                      </>
                    ) : (
                      <span title={HOOK_EVENT_HINTS[eventValue as HookLifecycleEvent]}>
                        {HOOK_EVENT_HINTS[eventValue as HookLifecycleEvent]}
                      </span>
                    )}
                  </div>
                  <div className="flex gap-2 flex-wrap">
                    <Badge variant="secondary">{hook.action_type}</Badge>
                    {hook.matcher ? <Badge variant="secondary">matcher: {hook.matcher}</Badge> : null}
                  </div>
                  <span>
                    Scope: {hookScopeLabel(hook)}
                    {updated ? ` · Updated ${updated}` : ''}
                  </span>
                  {payloadSummary ? <span className="break-words">{payloadSummary}</span> : null}
                </div>
              </div>
            )
          })}
        </div>
      )}
    </div>
  )
}

export function SettingsSkillsPanel({ api, onError, activePersonaId }: Props) {
  const { skills, skillsTotal, skillsRemoteCount, policy, loading, saving, savePolicy } =
    usePersonaHookSkillPolicy(api, activePersonaId, onError)
  const [restrictSkills, setRestrictSkills] = useState(false)
  const [selectedSkillNames, setSelectedSkillNames] = useState<Set<string>>(() => new Set())
  const [search, setSearch] = useState('')
  const [sourceFilter, setSourceFilter] = useState('all')
  const [statusFilter, setStatusFilter] = useState('all')
  const [platformFilter, setPlatformFilter] = useState('all')
  const [showAllPersonas, setShowAllPersonas] = useState(false)

  useEffect(() => {
    const skillRestrict = policy != null && !policy.uses_default_skills
    setRestrictSkills(skillRestrict)
    setSelectedSkillNames(
      new Set(
        skillRestrict && Array.isArray(policy?.allowed_skill_names)
          ? policy.allowed_skill_names
          : skills.map((s) => s.name),
      ),
    )
  }, [policy, skills])

  useEffect(() => {
    setShowAllPersonas(false)
    setSearch('')
    setSourceFilter('all')
    setStatusFilter('all')
    setPlatformFilter('all')
  }, [activePersonaId])

  const personaFilteredSkills = useMemo(() => {
    if (activePersonaId == null || showAllPersonas) return skills
    return skills.filter(skillAvailableForPersona)
  }, [activePersonaId, showAllPersonas, skills])

  const platforms = useMemo(() => {
    const names = new Set<string>()
    for (const skill of personaFilteredSkills) {
      for (const platform of skill.platforms ?? []) names.add(platform)
    }
    return Array.from(names).sort((a, b) => a.localeCompare(b))
  }, [personaFilteredSkills])

  const filteredSkills = useMemo(() => {
    const q = search.trim().toLowerCase()
    return personaFilteredSkills.filter((skill) => {
      if (q && !matchesFilter(skillSearchText(skill), q)) return false
      if (sourceFilter === 'remote' && !skill.remote) return false
      if (sourceFilter === 'local' && skill.remote) return false
      if (platformFilter !== 'all' && !(skill.platforms ?? []).includes(platformFilter)) return false
      if (statusFilter !== 'all') {
        const bucket = statusBucket(
          skillStatusLabel(skill, activePersonaId, restrictSkills, selectedSkillNames)?.color,
        )
        if (bucket !== statusFilter) return false
      }
      return true
    })
  }, [
    activePersonaId,
    personaFilteredSkills,
    platformFilter,
    restrictSkills,
    search,
    selectedSkillNames,
    sourceFilter,
    statusFilter,
  ])

  const skillsAvailableCount = useMemo(
    () => (activePersonaId == null ? skills.length : skills.filter(skillAvailableForPersona).length),
    [activePersonaId, skills],
  )

  const policyDirty = useMemo(() => {
    if (activePersonaId == null || policy == null) return false
    const savedSkillRestrict = !policy.uses_default_skills
    if (restrictSkills !== savedSkillRestrict) return true
    const savedSkillNames = new Set(
      savedSkillRestrict && Array.isArray(policy.allowed_skill_names)
        ? policy.allowed_skill_names
        : skills.map((s) => s.name),
    )
    return !setsEqual(selectedSkillNames, savedSkillNames)
  }, [activePersonaId, policy, restrictSkills, selectedSkillNames, skills])

  function toggleSkillName(name: string, checked: boolean) {
    setSelectedSkillNames((prev) => {
      const next = new Set(prev)
      if (checked) next.add(name)
      else next.delete(name)
      return next
    })
  }

  function setAllSkillNames(checked: boolean) {
    setSelectedSkillNames(checked ? new Set(skills.map((s) => s.name)) : new Set())
  }

  if (loading) {
    return <SettingsPanelSkeleton />
  }

  const filtersActive =
    search.trim() !== '' || sourceFilter !== 'all' || statusFilter !== 'all' || platformFilter !== 'all'

  return (
    <div className="flex flex-col gap-3">
      <span className="font-semibold">
        Skill policy {activePersonaId != null ? `(persona #${activePersonaId})` : ''}
      </span>
      {activePersonaId == null ? (
        <span>Select a persona to edit per-persona skill availability.</span>
      ) : (
        <div className="flex flex-col gap-3">
          <span>
            Default is allow-all. Turn on restriction, pick skills, then save. Clear all and save to
            block every skill. Saving here does not change the hook allowlist.
          </span>
          <div className="flex flex-col gap-1">
            <div className="flex items-center justify-between gap-2 flex-wrap">
              <label>
                <div className="flex items-center gap-2">
                  <Switch
                    size="sm"
                    checked={restrictSkills}
                    disabled={saving}
                    onCheckedChange={(checked) => {
                      setRestrictSkills(checked)
                      if (checked) setSelectedSkillNames(new Set(skills.map((s) => s.name)))
                    }}
                  />
                  Restrict skills to selected
                </div>
              </label>
              {restrictSkills ? (
                <div className="flex gap-2">
                  <Button
                    size="sm"
                    variant="secondary"
                    disabled={saving || skills.length === 0}
                    onClick={() => setAllSkillNames(true)}
                  >
                    Select all skills
                  </Button>
                  <Button
                    size="sm"
                    variant="secondary"
                    disabled={saving}
                    onClick={() => setAllSkillNames(false)}
                  >
                    Clear skills
                  </Button>
                </div>
              ) : null}
            </div>
            {restrictSkills ? (
              <span>
                {selectedSkillNames.size} of {skills.length} skills allowed
              </span>
            ) : (
              <span>All skills allowed for this persona.</span>
            )}
          </div>
          <div className="flex gap-2 flex-wrap">
            <Button
              size="sm"
              disabled={saving || (!policyDirty && !restrictSkills)}
              onClick={() =>
                void savePolicy({
                  skills: { restrict: restrictSkills, selectedNames: Array.from(selectedSkillNames) },
                })
              }
            >
              Save skill policy
            </Button>
            <Button
              size="sm"
              variant="secondary"
              disabled={saving}
              onClick={() => void savePolicy({ skills: { restrict: false, selectedNames: [] } })}
            >
              Allow all skills
            </Button>
          </div>
          <span>
            Saved: skills {policy?.uses_default_skills ? 'allow-all' : 'restricted'}.
            {policyDirty ? ' Unsaved changes.' : ''}
          </span>
        </div>
      )}

      {activePersonaId != null ? (
        <label>
          <div className="flex items-center gap-2">
            <Checkbox
              checked={showAllPersonas}
              disabled={saving}
              onCheckedChange={(checked) => setShowAllPersonas(checked === true)}
            />
            Show all personas (include skills unavailable for this persona)
          </div>
        </label>
      ) : null}

      <span className="font-semibold">Skills catalog</span>
      <span>
        {activePersonaId != null
          ? showAllPersonas
            ? `Showing ${filteredSkills.length} of ${skillsTotal} skills (${skillsAvailableCount} available for persona #${activePersonaId}, ${skillsRemoteCount} remote)`
            : `Showing ${filteredSkills.length} of ${personaFilteredSkills.length} skills available for persona #${activePersonaId} (${skillsRemoteCount} remote total)`
          : `Showing ${filteredSkills.length} of ${skillsTotal} skills (${skillsRemoteCount} remote — API or other platform)`}
        {activePersonaId != null && restrictSkills ? ' · checkboxes when skill restriction is on' : ''}
        . Skills only under <code>shared/workspace/</code> are not listed — move them to <code>skills/</code>.
      </span>
      <FilterBar
        search={search}
        onSearch={setSearch}
        searchPlaceholder="Filter skills by name, description, platforms, or source"
        clearDisabled={!filtersActive}
        onClear={() => {
          setSearch('')
          setSourceFilter('all')
          setStatusFilter('all')
          setPlatformFilter('all')
        }}
        filters={
          <>
            <FilterSelect
              label="Skill source"
              value={sourceFilter}
              onChange={setSourceFilter}
              options={[
                { value: 'all', label: 'All sources' },
                { value: 'local', label: 'Local' },
                { value: 'remote', label: 'Remote' },
              ]}
            />
            <FilterSelect
              label="Skill status"
              value={statusFilter}
              onChange={setStatusFilter}
              options={[
                { value: 'all', label: 'All statuses' },
                { value: 'allowed', label: 'Allowed' },
                { value: 'blocked', label: 'Blocked' },
              ]}
            />
            <FilterSelect
              label="Skill platform"
              value={platformFilter}
              onChange={setPlatformFilter}
              options={[
                { value: 'all', label: 'All platforms' },
                ...platforms.map((platform) => ({ value: platform, label: platform })),
              ]}
            />
          </>
        }
      />
      {skills.length === 0 ? (
        <span>No skills discovered under workspace/skills.</span>
      ) : filteredSkills.length === 0 ? (
        <span>No skills match the current filter.</span>
      ) : (
        <div className="flex flex-col gap-2 max-h-[420px] overflow-y-auto">
          {filteredSkills.map((skill) => {
            const updated = formatUpdatedAt(skill.updated_at)
            const status = skillStatusLabel(skill, activePersonaId, restrictSkills, selectedSkillNames)
            return (
              <div className="flex items-start gap-2 rounded-md border border-[var(--gray-a6)] p-2" key={skill.name}>
                {activePersonaId != null && restrictSkills ? (
                  <Checkbox
                    className="mt-0.5"
                    checked={selectedSkillNames.has(skill.name)}
                    disabled={saving}
                    onCheckedChange={(checked) => toggleSkillName(skill.name, checked === true)}
                  />
                ) : null}
                <div className="flex flex-col gap-1 min-w-0 flex-1">
                  <div className="flex items-center gap-2 flex-wrap">
                    <span className="font-medium">{skill.name}</span>
                    {status ? <StatusBadge text={status.text} color={status.color} /> : null}
                    {skill.remote ? <Badge variant="outline">Remote</Badge> : null}
                    {skill.version ? <Badge variant="outline">v{skill.version}</Badge> : null}
                  </div>
                  <span>{skill.description}</span>
                  {skill.when_to_use ? <span>When to use: {skill.when_to_use}</span> : null}
                  <div className="flex gap-2 flex-wrap">
                    {skill.source ? <Badge variant="secondary">source: {skill.source}</Badge> : null}
                    {(skill.platforms ?? []).map((platform) => (
                      <Badge key={platform} variant="outline" className={STATUS_BADGE_CLASS.blue}>
                        {platform}
                      </Badge>
                    ))}
                    {(skill.deps ?? []).length > 0 ? (
                      <Badge variant="secondary">deps: {(skill.deps ?? []).join(', ')}</Badge>
                    ) : null}
                  </div>
                  {updated ? <span>Updated {updated}</span> : null}
                </div>
              </div>
            )
          })}
        </div>
      )}
    </div>
  )
}
