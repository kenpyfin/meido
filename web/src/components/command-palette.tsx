import React, { useCallback, useEffect, useMemo, useState } from 'react'
import {
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandSeparator,
  CommandShortcut,
} from '@/components/ui/command'
import type { ChatSession, Persona, SideChatSummary } from '@/types'
import {
  Gauge,
  Inbox,
  Keyboard,
  MessageSquarePlus,
  Moon,
  PanelLeft,
  Settings,
  Sun,
  Terminal,
} from 'lucide-react'

export type CommandPaletteActions = {
  onOpenSettings: () => void
  onOpenInbox: () => void
  onOpenOps?: () => void
  onOpenTerminal?: () => void
  terminalAvailable?: boolean
  onToggleAppearance: () => void
  appearance: 'dark' | 'light'
  onToggleSidebar: () => void
  onToggleCockpit: () => void
  onShowShortcuts: () => void
  onCreateSession?: () => void
  onSelectPersona: (personaName: string) => void
  onSelectSession: (sessionId: string | null) => void
  onSelectSideChat?: (sideChatId: string) => void
}

export type CommandPaletteProps = {
  open: boolean
  onOpenChange: (open: boolean) => void
  personas: Persona[]
  chatSessions: ChatSession[]
  sideChats?: SideChatSummary[]
  actions: CommandPaletteActions
}

export function CommandPalette({
  open,
  onOpenChange,
  personas,
  chatSessions,
  sideChats = [],
  actions,
}: CommandPaletteProps) {
  const run = useCallback(
    (fn: () => void) => {
      onOpenChange(false)
      fn()
    },
    [onOpenChange],
  )

  const recentSessions = useMemo(() => chatSessions.slice(0, 8), [chatSessions])
  const recentSideChats = useMemo(() => sideChats.slice(0, 6), [sideChats])

  return (
    <CommandDialog open={open} onOpenChange={onOpenChange} title="Command palette" description="Jump to sessions, personas, and actions">
      <CommandInput placeholder="Search personas, sessions, actions…" />
      <CommandList>
        <CommandEmpty>No matches.</CommandEmpty>
        <CommandGroup heading="Actions">
          <CommandItem onSelect={() => run(actions.onOpenSettings)}>
            <Settings />
            Open Settings
            <CommandShortcut>S</CommandShortcut>
          </CommandItem>
          <CommandItem onSelect={() => run(actions.onOpenInbox)}>
            <Inbox />
            Open Inbox
          </CommandItem>
          {actions.onOpenOps ? (
            <CommandItem onSelect={() => run(actions.onOpenOps!)}>
              <Gauge />
              Open Ops
            </CommandItem>
          ) : null}
          {actions.terminalAvailable && actions.onOpenTerminal ? (
            <CommandItem onSelect={() => run(actions.onOpenTerminal!)}>
              <Terminal />
              Open Terminal
            </CommandItem>
          ) : null}
          <CommandItem onSelect={() => run(actions.onToggleCockpit)}>
            <Gauge />
            Toggle Cockpit
          </CommandItem>
          <CommandItem onSelect={() => run(actions.onToggleSidebar)}>
            <PanelLeft />
            Toggle Sidebar
          </CommandItem>
          <CommandItem onSelect={() => run(actions.onToggleAppearance)}>
            {actions.appearance === 'dark' ? <Sun /> : <Moon />}
            Switch to {actions.appearance === 'dark' ? 'light' : 'dark'} mode
          </CommandItem>
          <CommandItem onSelect={() => run(actions.onShowShortcuts)}>
            <Keyboard />
            Keyboard shortcuts
            <CommandShortcut>?</CommandShortcut>
          </CommandItem>
          {actions.onCreateSession ? (
            <CommandItem onSelect={() => run(actions.onCreateSession!)}>
              <MessageSquarePlus />
              New session
            </CommandItem>
          ) : null}
        </CommandGroup>
        <CommandSeparator />
        <CommandGroup heading="Personas">
          {personas.map((p) => (
            <CommandItem key={p.id} value={`persona ${p.name}`} onSelect={() => run(() => actions.onSelectPersona(p.name))}>
              {p.name}
            </CommandItem>
          ))}
        </CommandGroup>
        {recentSessions.length > 0 ? (
          <>
            <CommandSeparator />
            <CommandGroup heading="Sessions">
              {recentSessions.map((s) => (
                <CommandItem
                  key={s.id}
                  value={`session ${s.title ?? s.id}`}
                  onSelect={() => run(() => actions.onSelectSession(s.id))}
                >
                  {s.title?.trim() || s.id.slice(0, 8)}
                </CommandItem>
              ))}
            </CommandGroup>
          </>
        ) : null}
        {recentSideChats.length > 0 && actions.onSelectSideChat ? (
          <>
            <CommandSeparator />
            <CommandGroup heading="Side chats">
              {recentSideChats.map((s) => (
                <CommandItem
                  key={s.id}
                  value={`sidechat ${s.title ?? s.id}`}
                  onSelect={() => run(() => actions.onSelectSideChat!(s.id))}
                >
                  {s.title?.trim() || s.id.slice(0, 8)}
                </CommandItem>
              ))}
            </CommandGroup>
          </>
        ) : null}
      </CommandList>
    </CommandDialog>
  )
}

/** Global ⌘K / Ctrl+K listener that opens the palette when not typing in a field. */
export function useCommandPaletteShortcut(onOpen: () => void, disabled?: boolean): void {
  useEffect(() => {
    if (disabled) return
    const onKeyDown = (e: KeyboardEvent) => {
      if (!(e.key === 'k' || e.key === 'K')) return
      if (!(e.metaKey || e.ctrlKey)) return
      const target = e.target
      if (target instanceof HTMLElement) {
        const tag = target.tagName
        if (tag === 'INPUT' || tag === 'TEXTAREA' || target.isContentEditable) {
          // Still allow ⌘K from inputs — standard command palette behavior
        }
      }
      e.preventDefault()
      onOpen()
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [disabled, onOpen])
}

export function useCommandPaletteOpenState() {
  const [open, setOpen] = useState(false)
  const openPalette = useCallback(() => setOpen(true), [])
  useCommandPaletteShortcut(openPalette)
  return { open, setOpen, openPalette }
}
