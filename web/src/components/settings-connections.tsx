import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { ModelSelect } from './model-select'
import { LedgerSection, ServiceNote } from './ledger-chrome'
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
    if (!localModels.includes(model)) {
      onError('Pick a model from the list.')
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
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoadingLocalModels(false)
    }
  }

  async function testLocal() {
    const url = localUrl.trim()
    const model = localModel.trim()
    if (!url || !localModels.includes(model)) {
      onError('Pick a model from the list before testing.')
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

  const cursorDirty = cursor != null && (drafts.cursor ?? '') !== cursorMask(rows)

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-start justify-between gap-2">
        <ServiceNote summary="Save a provider key here when .env does not already set it. The next call uses that key.">
          The installer writes one starting key. A value already in .env is the one Meido uses, and a key saved here is not shown again. The Cursor bridge key can still be replaced below. The sidecar reads that key the next time it starts.
        </ServiceNote>
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
        <LedgerSection title="API keys" scope="live" className="flex flex-col gap-3">
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
        </LedgerSection>
      ) : null}

      {cursor ? (
        <LedgerSection title="Bridge agent" scope="restart" dirty={cursorDirty} className="flex flex-col gap-3">
          <ServiceNote summary="Replace the Cursor bridge key here. The sidecar uses it the next time it starts.">
            A key already in .env is shown masked. Saving writes over that value. Meido itself keeps running; only the bridge sidecar needs to start again.
          </ServiceNote>
          <KeyCard
            row={cursor}
            draft={drafts.cursor ?? ''}
            busy={savingId === 'cursor'}
            allowEnvOverride
            onDraft={(value) => setDrafts((current) => ({ ...current, cursor: value }))}
            onSave={() => void saveSecret(cursor)}
            onClear={() => void clearSaved(cursor)}
          />
        </LedgerSection>
      ) : null}

      <LedgerSection title="Local model server" scope="live" className="flex flex-col gap-2">
        <ServiceNote summary="Save the shared local server. System LLM uses it on the next local call.">
          The server must speak the OpenAI API. Cost routing keeps a separate URL on the Persona page.
        </ServiceNote>
        <Input
          value={localUrl}
          placeholder="http://127.0.0.1:8080/v1"
          onChange={(e) => setLocalUrl(e.target.value)}
          aria-label="Local model server URL"
        />
        <div className="flex flex-wrap items-center gap-2">
          <ModelSelect
            ariaLabel="Local model"
            value={localModel}
            options={localModels.map((id) => ({ id }))}
            onChange={setLocalModel}
            disabled={loadingLocalModels}
          />
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
      </LedgerSection>

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
    <div className="flex flex-col gap-2 rounded-md border p-3" style={{ borderColor: 'var(--mc-border-soft)' }}>
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
