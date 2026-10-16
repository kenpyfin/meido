import { useCallback, useEffect, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import type { EnvConfigResponse } from '@/types'

type Api = <T>(path: string, init?: RequestInit) => Promise<T>

const GROUPS: { title: string; keys: string[] }[] = [
  { title: 'Workspace', keys: ['WORKSPACE_DIR', 'TIMEZONE', 'AGENT_DISPLAY_NAME'] },
  { title: 'Web access', keys: ['WEB_HOST', 'WEB_PORT', 'WEB_AUTH_TOKEN'] },
  {
    title: 'ORIGIN vault',
    keys: [
      'VAULT_ORIGIN_VAULT_PATH',
      'VAULT_VECTOR_DB_PATH',
      'VAULT_VECTOR_DB_COLLECTION',
      'VAULT_PRINCIPLES_PATH',
      'VAULT_EMBEDDING_SERVER_URL',
      'VAULT_ORIGIN_VAULT_REPO',
    ],
  },
  { title: 'System LLM', keys: ['SYSTEM_LLM_PROVIDER', 'SYSTEM_LLM_MODEL'] },
  {
    title: 'Integrations',
    keys: [
      'BROWSER_MANAGED',
      'STEEL_API_PORT',
      'STEEL_CDP_PORT',
      'TAVILY_API_KEY',
      'SEARXNG_URL',
    ],
  },
]

export function SettingsEnvironmentPanel({
  api,
  onRunWizard,
  onChanged,
}: {
  api: Api
  onRunWizard: () => void
  onChanged: () => void
}) {
  const [data, setData] = useState<EnvConfigResponse | null>(null)
  const [drafts, setDrafts] = useState<Record<string, string>>({})
  const [busy, setBusy] = useState(false)
  const [message, setMessage] = useState('')
  const [error, setError] = useState('')

  const load = useCallback(async () => {
    const next = await api<EnvConfigResponse>('/api/env-config')
    setData(next)
    const values: Record<string, string> = {}
    for (const field of next.fields) values[field.key] = field.value ?? ''
    setDrafts(values)
  }, [api])

  useEffect(() => {
    void load().catch((err: unknown) => setError(err instanceof Error ? err.message : String(err)))
  }, [load])

  async function save() {
    if (!data) return
    const values: Record<string, string> = {}
    for (const field of data.fields) {
      const next = drafts[field.key] ?? ''
      if (field.secret && next === '') continue
      if (next !== (field.value ?? '')) values[field.key] = next
    }
    if (Object.keys(values).length === 0) {
      setMessage('No changes.')
      return
    }
    setBusy(true)
    setError('')
    try {
      const next = await api<EnvConfigResponse>('/api/env-config', {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ values }),
      })
      setData(next)
      const refreshed: Record<string, string> = {}
      for (const field of next.fields) refreshed[field.key] = field.value ?? ''
      setDrafts(refreshed)
      setMessage(next.requires_restart_for_env_changes ? 'Saved. Restart Meido to apply workspace, vault, or web bind changes.' : 'Saved to .env.')
      onChanged()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  async function runAction(action: string) {
    setBusy(true)
    setError('')
    try {
      const result = await api<{ message?: string }>('/api/setup/action', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ action }),
      })
      setMessage(result.message || 'Done.')
      await load()
      onChanged()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  const byKey = new Map((data?.fields ?? []).map((field) => [field.key, field]))

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap gap-2">
        <Button type="button" size="sm" variant="outline" onClick={onRunWizard}>
          Run Setup Wizard
        </Button>
        <Button type="button" size="sm" variant="outline" disabled={busy} onClick={() => void load()}>
          Reload from .env
        </Button>
        <Button type="button" size="sm" disabled={busy || !data} onClick={() => void save()}>
          Save .env
        </Button>
      </div>
      {data?.probes ? (
        <p className="text-xs text-muted-foreground">
          node {data.probes.node_available ? 'yes' : 'no'} · npm {data.probes.npm_available ? 'yes' : 'no'} · python {data.probes.python_available ? 'yes' : 'no'} · docker {data.probes.docker_available ? 'yes' : 'no'} · tmux {data.probes.tmux_available ? 'yes' : 'no'}
          {data.probes.lan_ipv4 ? ` · LAN ${data.probes.lan_ipv4}` : ''}
        </p>
      ) : null}
      {GROUPS.map((group) => (
        <section key={group.title} className="space-y-2">
          <h3 className="text-sm font-medium">{group.title}</h3>
          {group.keys.map((key) => {
            const field = byKey.get(key)
            if (!field) return null
            return (
              <label key={key} className="grid gap-1 text-xs">
                <span className="text-muted-foreground">
                  {key}
                  {field.secret && field.set ? ' (set)' : ''}
                </span>
                <Input
                  type={field.secret ? 'password' : 'text'}
                  autoComplete="off"
                  placeholder={field.secret && field.set ? 'Leave blank to keep the saved value' : ''}
                  value={drafts[key] ?? ''}
                  onChange={(event) => setDrafts((prev) => ({ ...prev, [key]: event.target.value }))}
                />
              </label>
            )
          })}
        </section>
      ))}
      <div className="space-y-2">
        <p className="text-xs">
          MemPalace: {data?.probes.mempalace_installed ? 'installed' : 'not installed'}
          {' · '}
          Steel Browser: {data?.probes.steel_installed ? 'installed' : 'not installed'}
        </p>
        <div className="flex flex-wrap gap-2">
          <Button
            type="button"
            size="sm"
            variant="outline"
            disabled={busy || (data?.probes.mempalace_installed && data?.probes.steel_installed)}
            onClick={() => void runAction('install_mempalace_and_steel')}
          >
            {data?.probes.mempalace_installed && data?.probes.steel_installed
              ? 'MemPalace and Steel are installed'
              : 'Install MemPalace and Steel'}
          </Button>
          <Button type="button" size="sm" variant="outline" disabled={busy} onClick={() => void runAction('install_gateway')}>
            Install login service
          </Button>
        </div>
      </div>
      {message ? <p className="text-xs text-muted-foreground">{message}</p> : null}
      {error ? <p className="text-xs text-destructive">{error}</p> : null}
    </div>
  )
}
