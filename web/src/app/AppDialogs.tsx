import React from 'react'
import type { AppDialogsProps } from './dialogs/types'
export type {
  AppDialogsSettingsProps,
  AppDialogsPersonaProps,
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
import { PersonaDialog } from './dialogs/persona-dialog'
import { QueueDialog } from './dialogs/queue-dialog'
import { InboxDialog } from './dialogs/inbox-dialog'
import { SchedulesDialog } from './dialogs/schedules-dialog'
import { ScheduleDetailDialog } from './dialogs/schedule-detail-dialog'
import { AgentsMdDialog } from './dialogs/agents-md-dialog'
import { ArtifactsDialog } from './dialogs/artifacts-dialog'
import { MemoryDialog } from './dialogs/memory-dialog'
import { AgentHistoryDialog } from './dialogs/agent-history-dialog'
import { TerminalDialog } from './dialogs/terminal-dialog'
import { SetupWizardDialog } from '../components/setup-wizard-dialog'

export function setupWizardShouldOpen(
  status: { needs_initialization?: boolean; workspace_configured?: boolean } | null | undefined,
  sessionDismissed: boolean,
): boolean {
  if (!status) return false
  if (status.workspace_configured === false) return true
  return status.needs_initialization === true && !sessionDismissed
}

function readSetupWizardDismissed(): boolean {
  try {
    return sessionStorage.getItem('meido_setup_wizard_dismissed') === '1'
  } catch {
    return true
  }
}

export function AppDialogs(props: AppDialogsProps) {
  const {
    appearance,
    api,
    chatId,
    activePersonaId,
    personas,
    settings,
    onConfigurePersona,
    persona,
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
  const [wizardOpen, setWizardOpen] = React.useState(false)
  const wizardDismissed = React.useRef(false)
  React.useEffect(() => {
    if (wizardDismissed.current) return
    const status = settings.installationStatus
    if (!setupWizardShouldOpen(status, readSetupWizardDismissed())) return
    setWizardOpen(true)
  }, [settings.installationStatus])

  function setWizard(open: boolean) {
    setWizardOpen(open)
    if (!open) {
      wizardDismissed.current = true
      try {
        sessionStorage.setItem('meido_setup_wizard_dismissed', '1')
      } catch {
        /* session storage can be unavailable */
      }
    }
  }

  return (
    <>
      <SettingsDialog
        appearance={appearance}
        api={api}
        chatId={chatId}
        activePersonaId={activePersonaId}
        personas={personas}
        settings={settings}
        onRunSetupWizard={() => setWizardOpen(true)}
        onConfigurePersona={onConfigurePersona}
      />
      <SetupWizardDialog
        api={api}
        open={wizardOpen}
        onOpenChange={setWizard}
        onRestart={() => void settings.requestRestart()}
        restartBusy={settings.restartBusy}
      />
      <PersonaDialog
        api={api}
        activePersonaId={activePersonaId}
        personas={personas}
        persona={persona}
      />
      <QueueDialog appearance={appearance} queue={queue} />
      <InboxDialog appearance={appearance} personas={personas} inbox={inbox} />
      <SchedulesDialog
        appearance={appearance}
        api={api}
        chatId={chatId}
        activePersonaId={activePersonaId}
        personas={personas}
        schedules={schedules}
      />
      <ScheduleDetailDialog
        appearance={appearance}
        api={api}
        chatId={chatId}
        personas={personas}
        scheduleDetail={scheduleDetail}
      />
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
