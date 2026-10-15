import React from 'react'
import type { AppDialogsProps } from './dialogs/types'
export type {
  AppDialogsSettingsProps,
  AppDialogsQueueProps,
  AppDialogsSchedulesProps,
  AppDialogsInboxProps,
  AppDialogsScheduleDetailProps,
  AppDialogsAgentsMdProps,
  AppDialogsArtifactsProps,
  AppDialogsMemoryProps,
  AppDialogsAgentHistoryProps,
  AppDialogsTerminalProps,
  AppDialogsProps,
  Appearance,
} from './dialogs/types'
import { SettingsDialog } from './dialogs/settings-dialog'
import { QueueDialog } from './dialogs/queue-dialog'
import { InboxDialog } from './dialogs/inbox-dialog'
import { SchedulesDialog } from './dialogs/schedules-dialog'
import { ScheduleDetailDialog } from './dialogs/schedule-detail-dialog'
import { AgentsMdDialog } from './dialogs/agents-md-dialog'
import { ArtifactsDialog } from './dialogs/artifacts-dialog'
import { MemoryDialog } from './dialogs/memory-dialog'
import { AgentHistoryDialog } from './dialogs/agent-history-dialog'
import { TerminalDialog } from './dialogs/terminal-dialog'

export function AppDialogs(props: AppDialogsProps) {
  const {
    appearance,
    api,
    chatId,
    activePersonaId,
    personas,
    settings,
    queue,
    schedules,
    inbox,
    scheduleDetail,
    agentsMd,
    artifacts,
    memory,
    agentHistory,
    terminal,
  } = props

  return (
    <>
      <SettingsDialog
        appearance={appearance}
        api={api}
        chatId={chatId}
        activePersonaId={activePersonaId}
        personas={personas}
        settings={settings}
      />
      <QueueDialog appearance={appearance} queue={queue} />
      <InboxDialog appearance={appearance} personas={personas} inbox={inbox} />
      <SchedulesDialog
        appearance={appearance}
        activePersonaId={activePersonaId}
        personas={personas}
        schedules={schedules}
      />
      <ScheduleDetailDialog appearance={appearance} personas={personas} scheduleDetail={scheduleDetail} />
      <AgentsMdDialog appearance={appearance} agentsMd={agentsMd} />
      <ArtifactsDialog
        appearance={appearance}
        chatId={chatId}
        activePersonaId={activePersonaId}
        artifacts={artifacts}
      />
      <MemoryDialog appearance={appearance} activePersonaId={activePersonaId} memory={memory} />
      <AgentHistoryDialog appearance={appearance} activePersonaId={activePersonaId} agentHistory={agentHistory} />
      <TerminalDialog terminal={terminal} />
    </>
  )
}
