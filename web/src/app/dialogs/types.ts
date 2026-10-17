import type React from 'react'
import type { ParsedAgentHistory } from '../../parse-agent-history'
import type {
  BackgroundJobItem,
  ChannelBinding,
  InstallationStatus,
  Persona,
  QueueItem,
  ScheduleTask,
} from '../../types'

export type Appearance = 'dark' | 'light'

export interface AppDialogsSettingsProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  error: string
  setError: (value: string) => void
  restartNotice: string | null
  installationStatus: InstallationStatus | null
  restartBusy: boolean
  requestRestart: () => Promise<void>
  bindings: ChannelBinding[]
  updateChannelPersonaPolicy: (
    botInstanceId: number,
    mode: 'all' | 'single',
    personaId?: number,
  ) => Promise<void>
  reloadInstallationStatus: () => Promise<void>
}

export interface AppDialogsPersonaProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  error: string
  setError: (value: string) => void
  onPersonaBulletinChanged?: () => void | Promise<void>
  onCreatePersona?: () => void | Promise<void>
  onRenamePersona?: (personaId: number, name: string) => Promise<void>
  onDeletePersona?: (personaId: number) => void
}

export interface AppDialogsQueueProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  showAllPersonas: boolean
  setShowAllPersonas: (v: boolean) => void
  items: QueueItem[]
  /** Wall clock when `items[].elapsed_ms` was sampled. 0 before the first poll. */
  sampledAtMs: number
  stoppingRunIds: string[]
  handleQueueAction: (runId: string, state: string) => Promise<void>
  backgroundJobs: BackgroundJobItem[]
  stoppingBackgroundJobIds: string[]
  isActiveBackgroundJobStatus: (status: string) => boolean
  handleBackgroundJobStop: (jobId: string) => Promise<void>
  onOpenSchedules: () => void
}

export interface AppDialogsSchedulesProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  showArchived: boolean
  setShowArchived: (v: boolean) => void
  schedules: ScheduleTask[]
  filtered: ScheduleTask[]
  newPrompt: string
  setNewPrompt: (v: string) => void
  newType: 'cron' | 'once'
  setNewType: (v: 'cron' | 'once') => void
  newValue: string
  setNewValue: (v: string) => void
  newPersonaId: number | null
  setNewPersonaId: (v: number) => void
  newDelivery: 'runs_only' | 'transcript'
  setNewDelivery: (v: 'runs_only' | 'transcript') => void
  createSchedule: (
    prompt: string,
    scheduleType: 'cron' | 'once',
    scheduleValue: string,
    personaId: number | null,
    outputDelivery?: 'runs_only' | 'transcript',
  ) => Promise<void>
  updateSchedule: (id: number, patch: Partial<ScheduleTask>) => Promise<void>
  openDetail: (task: ScheduleTask) => void
}

export interface AppDialogsInboxProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  unread: {
    personaId: number
    personaName: string
    lastBotMessageAt: string | null
    sessionId: string | null
    sessionTitle: string | null
  }[]
  todos: import('../types').PersonaTodo[]
  loading: boolean
  busyTodoId: number | null
  onRefresh: () => void
  onOpenTarget: (target: { personaId: number; sessionId: string | null }) => void
  onCompleteTodo: (todoId: number) => void
}

export interface AppDialogsScheduleDetailProps {
  task: ScheduleTask | null
  onOpenChange: (open: boolean) => void
  prompt: string
  setPrompt: (v: string) => void
  scheduleType: 'cron' | 'once'
  setScheduleType: (v: 'cron' | 'once') => void
  scheduleValue: string
  setScheduleValue: (v: string) => void
  busy: boolean
  setBusy: (v: boolean) => void
  updateSchedule: (id: number, patch: Partial<ScheduleTask>) => Promise<void>
  close: () => void
}

export interface AppDialogsAgentsMdProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  path: string
  error: string
  setError: (v: string) => void
  setBusy: (v: boolean) => void
  content: string
  setContent: (v: string) => void
  mtimeMs: number | null
  busy: boolean
  load: () => Promise<void>
  save: () => Promise<void>
}

export interface AppDialogsArtifactsProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  onOpenMessage: (id: string) => void
}

export interface AppDialogsMemoryProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  setError: (v: string) => void
  setBusy: (v: boolean) => void
  pathHint: string
  error: string
  content: string
  setContent: (v: string) => void
  mtimeMs: number | null
  busy: boolean
  load: (personaId: number) => Promise<void>
  save: (personaId: number) => Promise<void>
}

export interface AppDialogsAgentHistoryProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  setTab: (tab: 'trace' | 'prompt' | 'evaluators') => void
  setError: (v: string) => void
  setBusy: (v: boolean) => void
  pathHint: string
  filename: string
  mtimeMs: number | null
  busy: boolean
  error: string
  parsed: ParsedAgentHistory | null
  tab: 'trace' | 'prompt' | 'evaluators'
  iterationIdx: number
  setIterationIdx: React.Dispatch<React.SetStateAction<number>>
  raw: string
  optimizeBusy: boolean
  optimizeNotes: string
  setOptimizeNotes: (v: string) => void
  load: (personaId: number) => Promise<void>
  optimize: (personaId: number, operatorNotes?: string) => Promise<void>
}

export interface AppDialogsTerminalProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  error: string
  setError: (v: string) => void
}

export interface AppDialogsProps {
  appearance: Appearance
  api: <T>(path: string, init?: RequestInit) => Promise<T>
  chatId: number | null
  activePersonaId: number | null
  personas: Persona[]
  settings: AppDialogsSettingsProps
  persona: AppDialogsPersonaProps
  queue: AppDialogsQueueProps
  schedules: AppDialogsSchedulesProps
  inbox: AppDialogsInboxProps
  scheduleDetail: AppDialogsScheduleDetailProps
  agentsMd: AppDialogsAgentsMdProps
  artifacts: AppDialogsArtifactsProps
  memory: AppDialogsMemoryProps
  agentHistory: AppDialogsAgentHistoryProps
  terminal: AppDialogsTerminalProps
}

export type SettingsDialogProps = {
  appearance: Appearance
  api: AppDialogsProps['api']
  chatId: number | null
  activePersonaId: number | null
  personas: Persona[]
  settings: AppDialogsSettingsProps
  onRunSetupWizard?: () => void
}

export type PersonaDialogProps = {
  api: AppDialogsProps['api']
  activePersonaId: number | null
  personas: Persona[]
  persona: AppDialogsPersonaProps
}

export type QueueDialogProps = {
  appearance: Appearance
  queue: AppDialogsQueueProps
}

export type InboxDialogProps = {
  appearance: Appearance
  personas: Persona[]
  inbox: AppDialogsInboxProps
}

export type SchedulesDialogProps = {
  appearance: Appearance
  api: AppDialogsProps['api']
  chatId: number | null
  activePersonaId: number | null
  personas: Persona[]
  schedules: AppDialogsSchedulesProps
}

export type ScheduleDetailDialogProps = {
  appearance: Appearance
  api: AppDialogsProps['api']
  chatId: number | null
  personas: Persona[]
  scheduleDetail: AppDialogsScheduleDetailProps
}

export type AgentsMdDialogProps = {
  appearance: Appearance
  agentsMd: AppDialogsAgentsMdProps
}

export type ArtifactsDialogProps = {
  appearance: Appearance
  chatId: number | null
  activePersonaId: number | null
  artifacts: AppDialogsArtifactsProps
}

export type MemoryDialogProps = {
  appearance: Appearance
  activePersonaId: number | null
  memory: AppDialogsMemoryProps
}

export type AgentHistoryDialogProps = {
  appearance: Appearance
  activePersonaId: number | null
  agentHistory: AppDialogsAgentHistoryProps
}

export type TerminalDialogProps = {
  terminal: AppDialogsTerminalProps
}

