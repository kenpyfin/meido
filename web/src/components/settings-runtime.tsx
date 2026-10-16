import { useCallback, useEffect, useState } from 'react'
import { Switch } from '@/components/ui/switch'
import { SettingsPanelSkeleton } from './skeleton'
import type { RuntimeConfigResponse } from '../types'

type Props = {
  api: <T>(path: string, init?: RequestInit) => Promise<T>
  onError: (message: string) => void
}

function sourceLabel(source?: 'env' | 'app_settings'): string {
  return source === 'app_settings' ? 'saved in app' : 'from .env default'
}

export function SettingsRuntimePanel({ api, onError }: Props) {
  const [runtime, setRuntime] = useState<RuntimeConfigResponse | null>(null)
  const [loading, setLoading] = useState(true)
  const [savingKey, setSavingKey] = useState<string | null>(null)

  const load = useCallback(async () => {
    setLoading(true)
    try {
      const data = await api<RuntimeConfigResponse>('/api/runtime')
      setRuntime(data)
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
      setRuntime(null)
    } finally {
      setLoading(false)
    }
  }, [api, onError])

  useEffect(() => {
    void load()
  }, [load])

  async function patchRuntime(body: Record<string, boolean | string>, key: string) {
    setSavingKey(key)
    try {
      const res = await api<RuntimeConfigResponse>('/api/runtime', {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      })
      setRuntime(res)
      if (res.warnings?.length) {
        onError(res.warnings.join(' '))
      }
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e))
      await load()
    } finally {
      setSavingKey(null)
    }
  }

  if (loading) {
    return <SettingsPanelSkeleton />
  }

  const sources = runtime?.sources ?? {}

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex min-w-[200px] flex-1 flex-col gap-1">
          <span className="text-sm font-medium">Verbose pipeline logging</span>
          <span className="text-xs text-muted-foreground">
            When on: verbose shell logs appear in chat (including background-job completion
            messages). When off: full logs are kept for the agent only. Applies immediately (
            {sourceLabel(sources.tool_output_debug)}).
          </span>
        </div>
        <Switch
          checked={runtime?.tool_output_debug ?? false}
          disabled={savingKey != null}
          onCheckedChange={(checked) =>
            void patchRuntime({ tool_output_debug: checked }, 'tool_output_debug')
          }
        />
      </div>

      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex min-w-[200px] flex-1 flex-col gap-1">
          <span className="text-sm font-medium">Post-tool evaluator (PTE)</span>
          <span className="text-xs text-muted-foreground">
            After each tool iteration, ask a sidecar model whether the session goal is fulfilled.
            Can exit early or add latency on tool-heavy runs. Uses the local delegate endpoint when
            configured, else Perplexity. Applies immediately (
            {sourceLabel(sources.post_tool_evaluator_enabled)}).
          </span>
        </div>
        <Switch
          checked={runtime?.post_tool_evaluator_enabled ?? false}
          disabled={savingKey != null}
          onCheckedChange={(checked) =>
            void patchRuntime({ post_tool_evaluator_enabled: checked }, 'post_tool_evaluator_enabled')
          }
        />
      </div>

      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex min-w-[200px] flex-1 flex-col gap-1">
          <span className="text-sm font-medium">Pre-delivery quality (PDQE)</span>
          <span className="text-xs text-muted-foreground">
            Before the user sees a reply, judge the draft against the session goal. On fail with
            sufficient confidence, injects feedback and retries once. Applies immediately (
            {sourceLabel(sources.response_quality_evaluator_enabled)}).
          </span>
        </div>
        <Switch
          checked={runtime?.response_quality_evaluator_enabled ?? false}
          disabled={savingKey != null}
          onCheckedChange={(checked) =>
            void patchRuntime(
              { response_quality_evaluator_enabled: checked },
              'response_quality_evaluator_enabled',
            )
          }
        />
      </div>
    </div>
  )
}
