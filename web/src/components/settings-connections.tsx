import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { useCallback, useEffect, useState } from 'react'

type ConnectionRow = {
  id: string
  label: string
  kind: 'secret' | 'url'
  used_by?: string
  env_var?: string | null
  provider_id?: string | null
  source?: 'env' | 'saved' | 'missing'
  url?: string | null
  model?: string | null
  /** Fixed mask when a Cursor key exists. Never the key itself. */
  masked_secret?: string | null
}

type ConnectionsResponse = {
  ok?: boolean
  message?: string | null
  connections?: ConnectionRow[]
}

type Props = {
  api: <T>(path: string, init?: RequestInit) => Promise<T>
  onError: (message: string) => void
}

function sourceLabel(source: ConnectionRow['source']): string {
  if (source === 'env') return 'Set in .env'
  if (source === 'saved') return 'Saved in app'
  return 'Not set'
}

/** Matches `CURSOR_KEY_MASK` in `src/web.rs`. Not a secret. */
const CURSOR_KEY_MASK = '••••••••••••••••'

function cursorMask(rows: ConnectionRow[]): string {
  const cursor = rows.find((row) => row.id === 'cursor')
  if (!cursor) return ''
  if (cursor.masked_secret) return cursor.masked_secret
  if (cursor.source === 'env' || cursor.source === 'saved') return CURSOR_KEY_MASK
  return ''
}

export function SettingsConnectionsPanel({ api, onError }: Props) {
  const [rows, setRows] = useState<ConnectionRow[]>([])
  const [loading, setLoading] = useState(true)
  const [drafts, setDrafts] = useState<Record<string, string>>({})
  const [localUrl, setLocalUrl] = useState('')
  const [localModel, setLocalModel] = useState('')
  const [localModels, setLocalModels] = useState<string[]>([])
  const [localListed, setLocalListed] = useState(false)
  const [savingId, setSavingId] = useState<string | null>(null)
  const [loadingLocalModels, setLoadingLocalModels] = useState(false)
  const [testing, setTesting] = useState(false)
  const [testNotice, setTestNotice] = useState<string | null>(null)
  const [notice, setNotice] = useState<string | null>(null)
  const [reloading, setReloading] = useState(false)

  const applyRows = useCallback((next: ConnectionRow[]) => {
    setRows(next)
    const local = next.find((row) => row.id === 'local_server')
    setLocalUrl(local?.url ?? '')
    setLocalModel(local?.model ?? '')
  }, [])

  const fetchConnections = useCallback(async () => {
    const connections = await api<ConnectionsResponse>('/api/connections')
    const next = connections.connections ?? []
    applyRows(next)
    setDrafts({ cursor: cursorMask(next) })
  }, [api, applyRows])

  const load = useCallback(async () => {
    setLoading(true)
    try {
      await fetchConnections()
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
      setRows([])
    } finally {
      setLoading(false)
    }
  }, [fetchConnections, onError])

  async function reloadFromEnv() {
    setReloading(true)
    setNotice(null)
    try {
      await fetchConnections()
      setNotice('Reloaded from .env.')
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
    } finally {
      setReloading(false)
    }
  }

  useEffect(() => {
    void load()
  }, [load])

  async function saveLocal() {
    const url = localUrl.trim()
    const model = localModel.trim()
    if (!url) {
      onError('Enter the local server URL.')
      return
    }
    if (!model) {
      onError('Enter a local model name.')
      return
    }
    setSavingId('local_server')
    setNotice(null)
    try {
      const data = await api<ConnectionsResponse>('/api/connections', {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ id: 'local_server', url, model }),
      })
      applyRows(data.connections ?? [])
      setNotice(data.message ?? 'Local model saved.')
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
    } finally {
      setSavingId(null)
    }
  }

  async function refreshLocalModels() {
    const baseUrl = localUrl.trim()
    if (!baseUrl) {
      onError('Enter the local server URL before loading models.')
      return
    }
    setLoadingLocalModels(true)
    try {
      const res = await api<{ models?: Array<{ id: string } | string> }>(
        `/api/multimodel/models?base_url=${encodeURIComponent(baseUrl)}`,
      )
      const ids = (res.models ?? [])
        .map((model) => (typeof model === 'string' ? model : model.id))
        .filter((id): id is string => Boolean(id))
      if (ids.length === 0) {
        onError('No models returned from the local server.')
        return
      }
      setLocalModels(ids)
      setLocalListed(true)
      if (!ids.includes(localModel)) setLocalModel(ids[0] ?? '')
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoadingLocalModels(false)
    }
  }

  async function testLocal() {
    const url = localUrl.trim()
    const model = localModel.trim()
    if (!url || !model) {
      onError('Enter the server URL and model before testing.')
      return
    }
    setTesting(true)
    setTestNotice(null)
    try {
      const res = await api<{ message?: string; tools_ok?: boolean }>('/api/multimodel/test', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ tier: 'local', base_url: url, model }),
      })
      setTestNotice(res.message ?? (res.tools_ok === true ? 'Local server responded.' : 'Test finished.'))
    } catch (e) {
      setTestNotice(null)
      onError(e instanceof Error ? e.message : String(e))
    } finally {
      setTesting(false)
    }
  }

  async function saveSecret(row: ConnectionRow) {
    setSavingId(row.id)
    setNotice(null)
    try {
      const data = await api<ConnectionsResponse>('/api/connections', {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ id: row.id, secret: drafts[row.id] ?? '' }),
      })
      const next = data.connections ?? []
      applyRows(next)
      setDrafts((current) => ({
        ...current,
        [row.id]: row.id === 'cursor' ? cursorMask(next) : '',
      }))
      setNotice(data.message ?? 'Connection saved.')
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
    } finally {
      setSavingId(null)
    }
  }

  async function clearSaved(row: ConnectionRow) {
    setSavingId(row.id)
    setNotice(null)
    try {
      const data = await api<ConnectionsResponse>('/api/connections', {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ id: row.id, clear: true }),
      })
      const next = data.connections ?? []
      applyRows(next)
      setDrafts((current) => ({
        ...current,
        [row.id]: row.id === 'cursor' ? cursorMask(next) : '',
      }))
      setNotice(data.message ?? 'Saved key removed.')
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
    } finally {
      setSavingId(null)
    }
  }

  if (loading) {
    return <span className="text-sm text-muted-foreground">Loading connections…</span>
  }

  const apiKeys = rows.filter((row) => row.kind === 'secret' && row.id !== 'cursor')
  const cursor = rows.find((row) => row.id === 'cursor')

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-start justify-between gap-2">
        <p className="text-sm text-muted-foreground">
          Connectors this install supports. The installer writes one starting key; the others are set here. A value in{' '}
          <code className="text-xs">.env</code> is used when it is set. Otherwise a key saved here is used. Saved keys
          stay on this machine and are not shown again. The Cursor bridge key can be replaced below even when it is
          already in <code className="text-xs">.env</code>.
        </p>
        <Button
          type="button"
          size="sm"
          variant="outline"
          disabled={reloading}
          onClick={() => void reloadFromEnv()}
        >
          {reloading ? 'Reloading…' : 'Reload from .env'}
        </Button>
      </div>

      {apiKeys.length > 0 ? (
        <section className="flex flex-col gap-3">
          <span className="font-medium">API keys</span>
          {apiKeys.map((row) => (
            <KeyCard
              key={row.id}
              row={row}
              draft={drafts[row.id] ?? ''}
              busy={savingId === row.id}
              onDraft={(value) => setDrafts((current) => ({ ...current, [row.id]: value }))}
              onSave={() => void saveSecret(row)}
              onClear={() => void clearSaved(row)}
            />
          ))}
        </section>
      ) : null}

      {cursor ? (
        <section className="flex flex-col gap-3">
          <span className="font-medium">Bridge agent</span>
          <span className="text-xs text-muted-foreground">
            Cursor runs as the bridge agent. A key already in .env is shown masked. Replace it and save to override
            that value. The sidecar uses the new key the next time it starts.
          </span>
          <KeyCard
            row={cursor}
            draft={drafts.cursor ?? ''}
            busy={savingId === 'cursor'}
            allowEnvOverride
            onDraft={(value) => setDrafts((current) => ({ ...current, cursor: value }))}
            onSave={() => void saveSecret(cursor)}
            onClear={() => void clearSaved(cursor)}
          />
        </section>
      ) : null}

      <section className="flex flex-col gap-2 rounded-md border p-3" style={{ borderColor: 'var(--gray-6)' }}>
        <span className="font-medium">Local model server</span>
        <span className="text-xs text-muted-foreground">
          OpenAI-compatible server. The system LLM can use a model from this server. Cost routing uses it when that
          engine is selected.
        </span>
        <Input
          value={localUrl}
          placeholder="http://127.0.0.1:8080/v1"
          onChange={(e) => setLocalUrl(e.target.value)}
          aria-label="Local model server URL"
        />
        <div className="flex flex-wrap items-center gap-2">
          {localListed && localModels.length > 0 ? (
            <Select value={localModel || undefined} onValueChange={setLocalModel}>
              <SelectTrigger className="min-w-[220px] flex-1" aria-label="Local model">
                <SelectValue placeholder="Select a model" />
              </SelectTrigger>
              <SelectContent>
                {localModels.map((id) => (
                  <SelectItem key={id} value={id}>
                    {id}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          ) : (
            <Input
              value={localModel}
              placeholder="Model name"
              onChange={(e) => setLocalModel(e.target.value)}
              className="min-w-[220px] flex-1"
              aria-label="Local model"
            />
          )}
          <Button
            size="sm"
            variant="outline"
            type="button"
            disabled={loadingLocalModels || !localUrl.trim()}
            onClick={() => void refreshLocalModels()}
          >
            {loadingLocalModels ? 'Loading…' : 'Refresh models'}
          </Button>
          <Button
            size="sm"
            variant="outline"
            type="button"
            disabled={testing || !localUrl.trim() || !localModel.trim()}
            onClick={() => void testLocal()}
          >
            {testing ? 'Testing…' : 'Test'}
          </Button>
          <Button size="sm" type="button" disabled={savingId === 'local_server'} onClick={() => void saveLocal()}>
            {savingId === 'local_server' ? 'Saving…' : 'Save'}
          </Button>
        </div>
        {testNotice ? (
          <span className="text-xs text-muted-foreground" role="status">
            {testNotice}
          </span>
        ) : null}
      </section>

      {notice ? (
        <p className="text-xs text-muted-foreground" role="status">
          {notice}
        </p>
      ) : null}
    </div>
  )
}

function KeyCard({
  row,
  draft,
  busy,
  allowEnvOverride = false,
  onDraft,
  onSave,
  onClear,
}: {
  row: ConnectionRow
  draft: string
  busy: boolean
  allowEnvOverride?: boolean
  onDraft: (value: string) => void
  onSave: () => void
  onClear: () => void
}) {
  return (
    <div className="flex flex-col gap-2 rounded-md border p-3" style={{ borderColor: 'var(--gray-6)' }}>
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <span className="font-medium">{row.label}</span>
        <span className="text-xs text-muted-foreground">{sourceLabel(row.source)}</span>
      </div>
      <span className="text-xs text-muted-foreground">
        {row.used_by}
        {row.env_var ? ` · ${row.env_var}` : ''}
      </span>
      <SecretFields
        row={row}
        draft={draft}
        busy={busy}
        allowEnvOverride={allowEnvOverride}
        onDraft={onDraft}
        onSave={onSave}
        onClear={onClear}
      />
    </div>
  )
}

function SecretFields({
  row,
  draft,
  busy,
  allowEnvOverride = false,
  onDraft,
  onSave,
  onClear,
}: {
  row: ConnectionRow
  draft: string
  busy: boolean
  allowEnvOverride?: boolean
  onDraft: (value: string) => void
  onSave: () => void
  onClear: () => void
}) {
  if (row.source === 'env' && !allowEnvOverride) {
    return (
      <span className="text-xs text-muted-foreground">
        This key is set in .env, so a key saved here would not be used.
      </span>
    )
  }
  const unchangedMask =
    (Boolean(row.masked_secret) && draft === row.masked_secret) ||
    (allowEnvOverride &&
      draft === CURSOR_KEY_MASK &&
      (row.source === 'env' || row.source === 'saved'))
  const canRemove = row.source === 'saved' || (allowEnvOverride && row.source === 'env')
  return (
    <div className="flex flex-wrap items-center gap-2">
      <Input
        type="password"
        value={draft}
        placeholder={row.source === 'saved' && !draft ? 'Key saved — enter a new one to replace it' : 'API key'}
        autoComplete="off"
        onFocus={(e) => {
          if (unchangedMask) e.currentTarget.select()
        }}
        onChange={(e) => onDraft(e.target.value)}
        className="min-w-[220px] flex-1"
        aria-label={`${row.label} API key`}
      />
      <Button size="sm" type="button" disabled={busy || !draft.trim() || unchangedMask} onClick={onSave}>
        {busy ? 'Saving…' : 'Save'}
      </Button>
      {canRemove ? (
        <Button size="sm" variant="outline" type="button" disabled={busy} onClick={onClear}>
          Remove
        </Button>
      ) : null}
    </div>
  )
}
