import { useEffect, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import type { EnvConfigResponse } from '@/types'

type Api = <T>(path: string, init?: RequestInit) => Promise<T>

const STEPS = ['Workspace', 'Web access', 'Vault', 'System LLM', 'Cursor', 'MemPalace and Steel', 'Finish']

export function SetupWizardDialog({
  api,
  open,
  onOpenChange,
  onRestart,
  restartBusy,
}: {
  api: Api
  open: boolean
  onOpenChange: (open: boolean) => void
  onRestart: () => void
  restartBusy: boolean
}) {
  const [step, setStep] = useState(0)
  const [data, setData] = useState<EnvConfigResponse | null>(null)
  const [drafts, setDrafts] = useState<Record<string, string>>({})
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [message, setMessage] = useState('')

  useEffect(() => {
    if (!open) return
    setStep(0)
    setError('')
    void api<EnvConfigResponse>('/api/env-config')
      .then((next) => {
        setData(next)
        const values: Record<string, string> = {}
        for (const field of next.fields) values[field.key] = field.value ?? ''
        if (!values.WORKSPACE_DIR) values.WORKSPACE_DIR = './workspace'
        if (!values.TIMEZONE) values.TIMEZONE = 'UTC'
        if (!values.WEB_HOST) values.WEB_HOST = '127.0.0.1'
        if (!values.WEB_PORT) values.WEB_PORT = '10961'
        setDrafts(values)
      })
      .catch((err: unknown) => setError(err instanceof Error ? err.message : String(err)))
  }, [api, open])

  function setField(key: string, value: string) {
    setDrafts((prev) => ({ ...prev, [key]: value }))
  }

  async function save(keys: string[]) {
    const connectionIds: Record<string, string> = {
      ANTHROPIC_API_KEY: 'anthropic',
      OPENAI_API_KEY: 'openai',
      GEMINI_API_KEY: 'google',
      XAI_API_KEY: 'xai',
      OPENROUTER_API_KEY: 'openrouter',
      DEEPSEEK_API_KEY: 'deepseek',
      CURSOR_API_KEY: 'cursor',
    }
    const values: Record<string, string> = {}
    const connections: Array<{ id: string; secret: string }> = []
    for (const key of keys) {
      const next = drafts[key] ?? ''
      const connectionId = connectionIds[key]
      if (connectionId) {
        if (next.trim()) connections.push({ id: connectionId, secret: next.trim() })
        continue
      }
      const field = data?.fields.find((item) => item.key === key)
      if (field?.secret && next === '') continue
      values[key] = next
    }
    setBusy(true)
    setError('')
    try {
      if (Object.keys(values).length > 0) {
        const next = await api<EnvConfigResponse>('/api/env-config', {
          method: 'PATCH',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ values }),
        })
        setData(next)
      }
      for (const connection of connections) {
        await api('/api/connections', {
          method: 'PATCH',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(connection),
        })
      }
      return true
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
      return false
    } finally {
      setBusy(false)
    }
  }

  async function next() {
    const keysByStep = [
      ['WORKSPACE_DIR', 'TIMEZONE', 'AGENT_DISPLAY_NAME'],
      ['WEB_HOST', 'WEB_PORT', 'WEB_AUTH_TOKEN'],
      ['VAULT_ORIGIN_VAULT_PATH', 'VAULT_VECTOR_DB_PATH', 'VAULT_VECTOR_DB_COLLECTION', 'VAULT_PRINCIPLES_PATH', 'VAULT_EMBEDDING_SERVER_URL', 'VAULT_ORIGIN_VAULT_REPO'],
      ['SYSTEM_LLM_PROVIDER', 'SYSTEM_LLM_MODEL', 'ANTHROPIC_API_KEY', 'OPENAI_API_KEY', 'GEMINI_API_KEY', 'XAI_API_KEY', 'OPENROUTER_API_KEY', 'DEEPSEEK_API_KEY'],
      ['CURSOR_API_KEY'],
      ['TAVILY_API_KEY', 'SEARXNG_URL'],
      [],
    ]
    if (keysByStep[step].length > 0) {
      const ok = await save(keysByStep[step])
      if (!ok) return
    }
    if (step === 5 && (!data?.probes.mempalace_installed || !data?.probes.steel_installed)) {
      setError('MemPalace and Steel Browser are required. Install them before continuing.')
      return
    }
    setStep((current) => Math.min(current + 1, STEPS.length - 1))
  }

  async function action(name: string) {
    setBusy(true)
    setError('')
    try {
      const result = await api<{ message?: string }>('/api/setup/action', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ action: name }),
      })
      setMessage(result.message || 'Done.')
      const next = await api<EnvConfigResponse>('/api/env-config')
      setData(next)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>Set up Meido</DialogTitle>
          <DialogDescription>
            Step {step + 1} of {STEPS.length}: {STEPS[step]}. These values are written to .env. Editing .env by hand updates this screen the next time it loads.
          </DialogDescription>
        </DialogHeader>
        {step === 0 ? (
          <div className="space-y-2">
            <Field label="Workspace directory" value={drafts.WORKSPACE_DIR ?? ''} onChange={(value) => setField('WORKSPACE_DIR', value)} />
            <Field label="Timezone" value={drafts.TIMEZONE ?? ''} onChange={(value) => setField('TIMEZONE', value)} />
            <Field label="Display name" value={drafts.AGENT_DISPLAY_NAME ?? ''} onChange={(value) => setField('AGENT_DISPLAY_NAME', value)} />
          </div>
        ) : null}
        {step === 1 ? (
          <div className="space-y-2">
            <p className="text-xs text-muted-foreground">
              This machine uses 127.0.0.1. Every interface uses 0.0.0.0. A DNS name such as pp.local needs an mDNS advertiser, a hosts entry, or a router name.
              {data?.probes.lan_ipv4 ? ` This machine's LAN address is ${data.probes.lan_ipv4}.` : ''}
            </p>
            <div className="flex flex-wrap gap-2">
              <Button type="button" size="sm" variant="outline" onClick={() => setField('WEB_HOST', '127.0.0.1')}>This machine</Button>
              <Button type="button" size="sm" variant="outline" onClick={() => setField('WEB_HOST', '0.0.0.0')}>Every interface</Button>
            </div>
            <Field label="Web host" value={drafts.WEB_HOST ?? ''} onChange={(value) => setField('WEB_HOST', value)} />
            <Field label="Web port" value={drafts.WEB_PORT ?? ''} onChange={(value) => setField('WEB_PORT', value)} />
            <Field label="Access token (required off this machine)" secret value={drafts.WEB_AUTH_TOKEN ?? ''} onChange={(value) => setField('WEB_AUTH_TOKEN', value)} />
          </div>
        ) : null}
        {step === 2 ? (
          <div className="space-y-2">
            <p className="text-xs text-muted-foreground">Paths inside the workspace can be relative. Paths outside the workspace stay absolute.</p>
            <Field label="Vault notes path" value={drafts.VAULT_ORIGIN_VAULT_PATH ?? ''} onChange={(value) => setField('VAULT_ORIGIN_VAULT_PATH', value)} />
            <Field label="Vector database path" value={drafts.VAULT_VECTOR_DB_PATH ?? ''} onChange={(value) => setField('VAULT_VECTOR_DB_PATH', value)} />
            <Field label="Collection" value={drafts.VAULT_VECTOR_DB_COLLECTION ?? ''} onChange={(value) => setField('VAULT_VECTOR_DB_COLLECTION', value)} />
            <Field label="Principles file" value={drafts.VAULT_PRINCIPLES_PATH ?? ''} onChange={(value) => setField('VAULT_PRINCIPLES_PATH', value)} />
            <Field label="Embedding server URL" value={drafts.VAULT_EMBEDDING_SERVER_URL ?? ''} onChange={(value) => setField('VAULT_EMBEDDING_SERVER_URL', value)} />
            <Field label="Vault git repo" value={drafts.VAULT_ORIGIN_VAULT_REPO ?? ''} onChange={(value) => setField('VAULT_ORIGIN_VAULT_REPO', value)} />
          </div>
        ) : null}
        {step === 3 ? (
          <div className="space-y-2">
            <Field label="Provider (anthropic, openai, gemini, xai, openrouter, deepseek)" value={drafts.SYSTEM_LLM_PROVIDER ?? ''} onChange={(value) => setField('SYSTEM_LLM_PROVIDER', value)} />
            <Field label="Model" value={drafts.SYSTEM_LLM_MODEL ?? ''} onChange={(value) => setField('SYSTEM_LLM_MODEL', value)} />
            <p className="text-xs text-muted-foreground">Provider keys are saved under Settings → Connections.</p>
            <Field label="Anthropic key" secret value={drafts.ANTHROPIC_API_KEY ?? ''} onChange={(value) => setField('ANTHROPIC_API_KEY', value)} />
            <Field label="OpenAI key" secret value={drafts.OPENAI_API_KEY ?? ''} onChange={(value) => setField('OPENAI_API_KEY', value)} />
            <Field label="Gemini key" secret value={drafts.GEMINI_API_KEY ?? ''} onChange={(value) => setField('GEMINI_API_KEY', value)} />
            <Field label="xAI key" secret value={drafts.XAI_API_KEY ?? ''} onChange={(value) => setField('XAI_API_KEY', value)} />
            <Field label="OpenRouter key" secret value={drafts.OPENROUTER_API_KEY ?? ''} onChange={(value) => setField('OPENROUTER_API_KEY', value)} />
            <Field label="DeepSeek key" secret value={drafts.DEEPSEEK_API_KEY ?? ''} onChange={(value) => setField('DEEPSEEK_API_KEY', value)} />
          </div>
        ) : null}
        {step === 4 ? (
          <div className="space-y-2">
            <p className="text-xs text-muted-foreground">
              Cursor engine needs Node.js. node {data?.probes.node_available ? 'is installed' : 'is missing'}. npm {data?.probes.npm_available ? 'is installed' : 'is missing'}.
            </p>
            <p className="text-xs text-muted-foreground">The Cursor key is saved under Settings → Connections.</p>
            <Field label="Cursor API key" secret value={drafts.CURSOR_API_KEY ?? ''} onChange={(value) => setField('CURSOR_API_KEY', value)} />
          </div>
        ) : null}
        {step === 5 ? (
          <div className="space-y-2">
            <p className="text-xs text-muted-foreground">
              MemPalace and Steel Browser are required. An install that is already present is left as it is.
            </p>
            <p className="text-xs">
              MemPalace: {data?.probes.mempalace_installed ? 'installed' : 'not installed'}
            </p>
            <p className="text-xs">
              Steel Browser: {data?.probes.steel_installed ? 'installed' : 'not installed'}
            </p>
            <p className="text-xs text-muted-foreground">
              python {data?.probes.python_available ? 'yes' : 'no'} · docker {data?.probes.docker_available ? 'yes' : 'no'}
            </p>
            <Button
              type="button"
              size="sm"
              disabled={busy || (data?.probes.mempalace_installed && data?.probes.steel_installed)}
              onClick={() => void action('install_mempalace_and_steel')}
            >
              {data?.probes.mempalace_installed && data?.probes.steel_installed
                ? 'MemPalace and Steel are installed'
                : 'Install MemPalace and Steel'}
            </Button>
            <Field label="Tavily key" secret value={drafts.TAVILY_API_KEY ?? ''} onChange={(value) => setField('TAVILY_API_KEY', value)} />
            <Field label="SearXNG URL" value={drafts.SEARXNG_URL ?? ''} onChange={(value) => setField('SEARXNG_URL', value)} />
            <Button type="button" size="sm" variant="outline" disabled={busy} onClick={() => void action('install_gateway')}>Install login service</Button>
          </div>
        ) : null}
        {step === 6 ? (
          <div className="space-y-2 text-sm">
            <p>Workspace, vault, and system LLM values are in .env. Restart Meido when the banner says a restart is required.</p>
            <Button type="button" size="sm" disabled={restartBusy} onClick={onRestart}>Restart Meido</Button>
          </div>
        ) : null}
        {message ? <p className="text-xs text-muted-foreground">{message}</p> : null}
        {error ? <p className="text-xs text-destructive">{error}</p> : null}
        <div className="flex justify-between gap-2">
          <Button type="button" variant="outline" disabled={step === 0 || busy} onClick={() => setStep((current) => current - 1)}>Back</Button>
          {step < STEPS.length - 1 ? (
            <Button type="button" disabled={busy} onClick={() => void next()}>Continue</Button>
          ) : (
            <Button type="button" onClick={() => onOpenChange(false)}>Finish</Button>
          )}
        </div>
      </DialogContent>
    </Dialog>
  )
}

function Field({
  label,
  value,
  onChange,
  secret,
}: {
  label: string
  value: string
  onChange: (value: string) => void
  secret?: boolean
}) {
  return (
    <label className="grid gap-1 text-xs">
      <span className="text-muted-foreground">{label}</span>
      <Input type={secret ? 'password' : 'text'} autoComplete="off" value={value} onChange={(event) => onChange(event.target.value)} />
    </label>
  )
}
