import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import { Switch } from '@/components/ui/switch'
import { Select } from '@/components/ui/select'
import { Dialog, DialogContent, DialogTitle, DialogDescription, DialogClose } from '@/components/ui/dialog'
import { Tabs, TabsList, TabsTrigger, TabsContent } from '@/components/ui/tabs'
import React from 'react'

import { InitialRunPromptView } from '../../components/initial-run-prompt-view'
import { SettingsPanelSkeleton } from '../../components/skeleton'
import {
  AgentHistoryEvaluatorsPanel,
  AgentHistoryMarkdownBody,
  AgentHistoryEngineBadge,
  AgentHistoryTierBadge,
} from './shared'

import type { AgentHistoryDialogProps } from './types'

export function AgentHistoryDialog(props: AgentHistoryDialogProps) {
const { appearance, activePersonaId, agentHistory } = props
  const agentHistoryDialogOpen = agentHistory.open
  const setAgentHistoryDialogOpen = agentHistory.onOpenChange
  const setAgentHistoryTab = agentHistory.setTab
  const setAgentHistoryError = agentHistory.setError
  const setAgentHistoryBusy = agentHistory.setBusy
  const agentHistoryPathHint = agentHistory.pathHint
  const agentHistoryFilename = agentHistory.filename
  const agentHistoryMtimeMs = agentHistory.mtimeMs
  const agentHistoryBusy = agentHistory.busy
  const agentHistoryError = agentHistory.error
  const agentHistoryParsed = agentHistory.parsed
  const agentHistoryTab = agentHistory.tab
  const agentHistoryIterationIdx = agentHistory.iterationIdx
  const setAgentHistoryIterationIdx = agentHistory.setIterationIdx
  const agentHistoryRaw = agentHistory.raw
  const agentHistoryOptimizeBusy = agentHistory.optimizeBusy
  const agentHistoryOptimizeNotes = agentHistory.optimizeNotes
  const setAgentHistoryOptimizeNotes = agentHistory.setOptimizeNotes
  const loadAgentHistoryLatest = agentHistory.load
  const optimizeAgentHistoryLatest = agentHistory.optimize
  return (
    <Dialog
      open={agentHistoryDialogOpen}
      onOpenChange={agentHistory.onOpenChange}
    >
      <DialogContent className="max-w-[960px]">
        <DialogTitle>Agent run debug</DialogTitle>
        <DialogDescription className="mb-3">
          Latest saved run for this persona: tool/iteration trace and the system prompt plus messages
          sent on the first LLM call (new runs only). On Run trace, use Prev/Next or ← → to step
          through iterations.
        </DialogDescription>
    
        {agentHistoryPathHint ? (
          <span className="mb-1 block">
            {agentHistoryPathHint}
          </span>
        ) : null}
        {agentHistoryFilename ? (
          <span className="mb-2 block">
            File: {agentHistoryFilename}
            {agentHistoryMtimeMs != null ? ` · mtime: ${agentHistoryMtimeMs}` : ''}
          </span>
        ) : null}
    
        {agentHistoryBusy ? (
          <SettingsPanelSkeleton />
        ) : null}
    
        {agentHistoryError ? (
          <div role="status" className="rounded-lg border border-amber-500/30 bg-amber-500/10 p-3 text-sm text-amber-900 dark:text-amber-100 mb-2">{agentHistoryError}</div>
        ) : null}
    
        {!agentHistoryBusy && !agentHistoryError && agentHistoryParsed != null ? (
          <Tabs
            value={agentHistoryTab}
            onValueChange={(v) =>
              setAgentHistoryTab(v === 'prompt' ? 'prompt' : v === 'evaluators' ? 'evaluators' : 'trace')
            }
          >
            <TabsList className="mb-3 h-auto flex-wrap">
              <TabsTrigger value="trace">Run trace</TabsTrigger>
              <TabsTrigger value="evaluators">Evaluators</TabsTrigger>
              <TabsTrigger value="prompt" disabled={agentHistoryParsed.initialPromptJson == null}>
                First-turn prompt
              </TabsTrigger>
            </TabsList>
            <TabsContent value="trace">
              <>
                {agentHistoryParsed.engine ? (
                  <div className="mb-2">
                    <AgentHistoryEngineBadge engine={agentHistoryParsed.engine} />
                  </div>
                ) : null}
                {agentHistoryParsed.runHeader.trim() ? (
                  <div
                    className={
                      appearance === 'dark'
                        ? 'mb-3 max-h-32 overflow-auto rounded-md border border-[color:var(--mc-border-soft)] bg-[color:var(--mc-bg-panel)] p-2'
                        : 'mb-3 max-h-32 overflow-auto rounded-md border border-[color:var(--mc-border-strong)] bg-[color:var(--mc-surface-main)] p-2'
                    }
                  >
                    <AgentHistoryMarkdownBody markdown={agentHistoryParsed.runHeader} />
                  </div>
                ) : null}
    
                {agentHistoryParsed.iterations.length > 0 ? (
                  <>
                    <div className="flex justify-between items-center mb-2 flex-wrap gap-2">
                      <div className="flex flex-col gap-1">
                        <span>
                          Iteration {agentHistoryIterationIdx + 1} of{' '}
                          {agentHistoryParsed.iterations.length}
                        </span>
                        {agentHistoryParsed.iterations[agentHistoryIterationIdx]?.tier ? (
                          <AgentHistoryTierBadge
                            tier={
                              agentHistoryParsed.iterations[agentHistoryIterationIdx]!.tier!
                            }
                          />
                        ) : null}
                      </div>
                      <div className="flex gap-2">
                        <Button
                          size="sm"
                          variant="secondary"
                          disabled={agentHistoryIterationIdx <= 0}
                          onClick={() =>
                            setAgentHistoryIterationIdx((i) => Math.max(0, i - 1))
                          }
                        >
                          Prev
                        </Button>
                        <Button
                          size="sm"
                          variant="secondary"
                          disabled={
                            agentHistoryIterationIdx >= agentHistoryParsed.iterations.length - 1
                          }
                          onClick={() =>
                            setAgentHistoryIterationIdx((i) =>
                              Math.min(agentHistoryParsed.iterations.length - 1, i + 1),
                            )
                          }
                        >
                          Next
                        </Button>
                      </div>
                    </div>
                    <span className="mb-2 block">
                      Keyboard: ← →
                    </span>
                    <div
                      className={
                        appearance === 'dark'
                          ? 'max-h-[420px] overflow-auto rounded-md border border-[color:var(--mc-border-soft)] bg-[color:var(--mc-bg-panel)] p-3'
                          : 'max-h-[420px] overflow-auto rounded-md border border-[color:var(--mc-border-strong)] bg-[color:var(--mc-surface-elevated)] p-3'
                      }
                    >
                      <AgentHistoryMarkdownBody
                        markdown={
                          agentHistoryParsed.iterations[agentHistoryIterationIdx]?.body ?? ''
                        }
                      />
                    </div>
                  </>
                ) : (
                  <div
                    className={
                      appearance === 'dark'
                        ? 'max-h-[420px] overflow-auto rounded-md border border-[color:var(--mc-border-soft)] bg-[color:var(--mc-bg-panel)] p-3'
                        : 'max-h-[420px] overflow-auto rounded-md border border-[color:var(--mc-border-strong)] bg-[color:var(--mc-surface-elevated)] p-3'
                    }
                  >
                    <AgentHistoryMarkdownBody markdown={agentHistoryRaw} />
                  </div>
                )}
              </>
            </TabsContent>
            <TabsContent value="evaluators">
              <div
                className={
                  appearance === 'dark'
                    ? 'max-h-[420px] overflow-auto rounded-md border border-[color:var(--mc-border-soft)] bg-[color:var(--mc-bg-panel)] p-3'
                    : 'max-h-[420px] overflow-auto rounded-md border border-[color:var(--mc-border-strong)] bg-[color:var(--mc-surface-elevated)] p-3'
                }
              >
                <AgentHistoryEvaluatorsPanel parsed={agentHistoryParsed} />
              </div>
            </TabsContent>
            <TabsContent value="prompt">
              {agentHistoryParsed.initialPromptJson ? (
                <InitialRunPromptView
                  jsonText={agentHistoryParsed.initialPromptJson}
                  appearance={appearance}
                />
              ) : (
                <div role="status" className="rounded-lg border border-border bg-muted/50 p-3 text-sm">
                    No first-turn prompt snapshot in this file. Run a new agent turn after upgrading
                    the gateway; older history files only contain the iteration trace.
                  </div>
              )}
            </TabsContent>
          </Tabs>
        ) : null}
    
        <div className="flex flex-col gap-1 mt-3 mb-2">
          <span>
            Optional guidance for Learn &amp; optimize (combined with PDQE feedback from this run).
          </span>
          <Textarea
            rows={3}
            placeholder="e.g. Focus on vault search habits and reducing repeated bash calls…"
            value={agentHistoryOptimizeNotes}
            onChange={(e) => setAgentHistoryOptimizeNotes(e.target.value)}
            disabled={agentHistoryBusy || agentHistoryOptimizeBusy || activePersonaId == null}
          />
        </div>
    
        <div className="flex justify-end mt-3 gap-2 flex-wrap">
          <Button
            size="sm"
            variant="secondary"
            onClick={() => {
              if (activePersonaId != null) {
                void optimizeAgentHistoryLatest(
                  activePersonaId,
                  agentHistoryOptimizeNotes.trim() || undefined,
                )
              }
            }}
            disabled={
              agentHistoryBusy
              || agentHistoryOptimizeBusy
              || activePersonaId == null
              || !agentHistoryFilename
            }
          >
            {agentHistoryOptimizeBusy ? 'Queuing…' : 'Learn & optimize'}
          </Button>
          <Button
            size="sm"
            variant="secondary"
            onClick={() => {
              if (activePersonaId != null) void loadAgentHistoryLatest(activePersonaId)
            }}
            disabled={agentHistoryBusy || activePersonaId == null}
          >
            Reload
          </Button>
          <DialogClose>
            <Button size="sm" variant="secondary">Close</Button>
          </DialogClose>
        </div>
      </DialogContent>
    </Dialog>
    
  )
}
