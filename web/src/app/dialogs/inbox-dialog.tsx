import React from 'react'
import { InboxPanel } from '../../components/inbox-panel'
import type { InboxDialogProps } from './types'

export function InboxDialog({ appearance, personas, inbox }: InboxDialogProps) {
  return (
    <InboxPanel
      appearance={appearance}
      open={inbox.open}
      onOpenChange={inbox.onOpenChange}
      unread={inbox.unread}
      todos={inbox.todos}
      personas={personas}
      loading={inbox.loading}
      busyTodoId={inbox.busyTodoId}
      onRefresh={inbox.onRefresh}
      onOpenTarget={inbox.onOpenTarget}
      onCompleteTodo={inbox.onCompleteTodo}
    />
  )
}
