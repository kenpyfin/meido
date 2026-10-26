import { ensureArtifactSession } from '../api/artifacts'
import { Button } from '@/components/ui/button'
import React, { lazy, Suspense, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import type { ChatModelAdapter, ChatModelRunOptions, ChatModelRunResult } from '@assistant-ui/react'
import '../aui-layout.css'
import '../styles.css'
import { toast } from 'sonner'
import { api, makeHeaders } from '../api/client'
import { BulletinStrip } from '../components/bulletin-strip'
import { CockpitBar } from '../components/cockpit-bar'
import { CommandPalette, useCommandPaletteOpenState } from '../components/command-palette'
import { SessionSidebar } from '../components/session-sidebar'
import { readSidebarMode, SIDEBAR_MODE_TITLES, writeSidebarMode, type SidebarMode } from './sidebar-mode'
import { GUEST_MAX_PER_TURN, GUEST_TURN_CAP_NOTICE, parseGuestMentions } from '../lib/guest-mention'
import { SubthreadSidePane } from '../components/subthread-side-pane'
import { SideChatOverlay, useNarrowViewport } from '../components/side-chat-overlay'
import { ThreadPane, type ComposerNotice } from '../components/thread-pane'
import { TopicFocusView } from '../components/topic-focus-view'
import { TopicSessionAction } from '../components/topic-session-action'
import { TopicOutlineRail } from '../components/topic-outline-rail'
import { MobileSheet } from '../components/mobile-sheet'
import { useConfirmDialog } from '../components/confirm-dialog'
import { NewPersonaDialog } from '../components/new-persona-dialog'
import { ErrorBanner } from '../components/error-banner'
import { StatusRegion } from '../components/status-region'
import { SessionPicker } from '../components/session-picker'
import { SideChatsPicker } from '../components/side-chats-picker'
import { MobileOpsSheet, ShortcutsDialog } from '../components/ops-ui'
import { Toaster } from '../components/ui/sonner'
import { TooltipProvider } from '../components/ui/tooltip'
import {
  ResizableHandle,
  ResizablePanel,
  ResizablePanelGroup,
} from '../components/ui/resizable'
import { AppHeader } from './AppHeader'
import { AppDialogs } from './AppDialogs'
import { AuthDialog } from '../context/AuthContext'
import { useChatHistory } from '../hooks/use-chat-history'
import { useSideChats } from '../hooks/use-side-chats'
import { findMessageElement } from '../lib/reveal-message'
import { useDocumentVisible } from '../hooks/use-document-visible'
import { useVisualViewportFrame } from '../hooks/use-visual-viewport-frame'
import { useKeyboardShortcuts } from '../hooks/use-keyboard-shortcuts'
import { useOperatorOps } from '../hooks/use-operator-ops'
import { useOpsPoll } from '../hooks/use-ops-poll'
import { useConversationTopics } from '../hooks/use-conversation-topics'
import { useTopicDossier } from '../hooks/use-topic-dossier'
import { usePersonaSession } from '../hooks/use-persona-session'
import {
  dataUrlToFile,
  splitDataUrl,
  uploadAttachmentFile,
  type SendAttachmentRef,
} from '../lib/attachments'
import { HISTORY_PAGE_SIZE } from './constants'
import { resolveStoredSessionId } from '../lib/persona-storage'
import {
  activeDossierTopic,
  dossierCursorKey,
  readThreadSurface,
  sideChatHost,
  writeThreadSurface,
  type DossierCursor,
  type ThreadSurface,
} from '../lib/topic-focus'
import {
  parseAgentHistoryMarkdown,
  parsePdqeSteps,
  parsePteDecisions,
  splitAgentHistoryRaw,
  type ParsedAgentHistory,
} from '../parse-agent-history'
import {
  OPERATOR_MEMO_MAX_CHARS,
  type AgentHistoryOptimizeRequest,
  type AgentHistoryOptimizeResponse,
  type BackgroundJobItem,
  type BackendMessage,
  type ChannelBinding,
  type InstallationStatus,
  type QueueItem,
  type ScheduleTask,
  type PersonaTodo,
} from '../types'
import type { PersonaLedgerPage, SettingsLedgerPage } from './dialogs/types'

const GroupsWorkspace = lazy(() =>
  import('./workspaces/groups-workspace').then((mod) => ({ default: mod.GroupsWorkspace })),
)
const CoordinatorWorkspace = lazy(() =>
  import('./workspaces/coordinator-workspace').then((mod) => ({ default: mod.CoordinatorWorkspace })),
)

type Appearance = 'dark' | 'light'

type PendingRun = { runId: string; personaId: number }
type UiTheme =
  | 'lit'
  | 'lit-sakura'
  | 'lit-matcha'
  | 'lit-salon'
  | 'lit-amber'
  | 'lit-wisteria'

const UI_THEME_OPTIONS: { key: UiTheme; label: string; color: string; canvas: string }[] = [
  { key: 'lit', label: 'Lit Parlor', color: '#e25b78', canvas: '#0c0a10' },
  { key: 'lit-sakura', label: 'Sakura Gaslight', color: '#f472b6', canvas: '#120a0f' },
  { key: 'lit-matcha', label: 'Matcha Tea Room', color: '#4ade80', canvas: '#08100c' },
  { key: 'lit-salon', label: 'Velvet Salon', color: '#7aa2f7', canvas: '#080b12' },
  { key: 'lit-amber', label: 'Amber Hearth', color: '#f59e42', canvas: '#110c08' },
  { key: 'lit-wisteria', label: 'Wisteria Room', color: '#b48ef7', canvas: '#0d0914' },
]

function readAppearance(): Appearance {
  const saved = localStorage.getItem('meido_appearance')
  return saved === 'light' ? 'light' : 'dark'
}

function saveAppearance(value: Appearance): void {
  localStorage.setItem('meido_appearance', value)
}

function readUiTheme(): UiTheme {
  const saved = localStorage.getItem('meido_ui_theme') as UiTheme | null
  return UI_THEME_OPTIONS.some((t) => t.key === saved) ? (saved as UiTheme) : 'lit'
}

function saveUiTheme(value: UiTheme): void {
  localStorage.setItem('meido_ui_theme', value)
}

const DESKTOP_SIDEBAR_OPEN_KEY = 'meido_desktop_sidebar_open'
const DESKTOP_SIDEBAR_WIDTH_KEY = 'meido_desktop_sidebar_width'
const DESKTOP_SIDEBAR_DEFAULT_WIDTH = 320
const DESKTOP_SIDEBAR_MIN_WIDTH = 260
const DESKTOP_SIDEBAR_MAX_WIDTH = 520
const DESKTOP_MAIN_PANEL_MIN_WIDTH = 480

function readDesktopSidebarOpen(): boolean {
  if (typeof window === 'undefined') return true
  try {
    return localStorage.getItem(DESKTOP_SIDEBAR_OPEN_KEY) !== '0'
  } catch {
    return true
  }
}

function saveDesktopSidebarOpen(open: boolean): void {
  if (typeof window === 'undefined') return
  try {
    localStorage.setItem(DESKTOP_SIDEBAR_OPEN_KEY, open ? '1' : '0')
  } catch {
    /* ignore */
  }
}

function desktopSidebarViewportMax(viewportWidth: number): number {
  return Math.min(
    DESKTOP_SIDEBAR_MAX_WIDTH,
    Math.max(DESKTOP_SIDEBAR_MIN_WIDTH, Math.round(viewportWidth - DESKTOP_MAIN_PANEL_MIN_WIDTH)),
  )
}

function clampDesktopSidebarWidth(value: number, viewportWidth?: number): number {
  const fallback = Number.isFinite(value) ? value : DESKTOP_SIDEBAR_DEFAULT_WIDTH
  const hardClamped = Math.min(
    DESKTOP_SIDEBAR_MAX_WIDTH,
    Math.max(DESKTOP_SIDEBAR_MIN_WIDTH, Math.round(fallback)),
  )
  if (typeof viewportWidth !== 'number') return hardClamped
  return Math.min(hardClamped, desktopSidebarViewportMax(viewportWidth))
}

function readDesktopSidebarWidth(): number {
  if (typeof window === 'undefined') return DESKTOP_SIDEBAR_DEFAULT_WIDTH
  try {
    const raw = localStorage.getItem(DESKTOP_SIDEBAR_WIDTH_KEY)
    const parsed = raw == null ? DESKTOP_SIDEBAR_DEFAULT_WIDTH : Number(raw)
    return clampDesktopSidebarWidth(parsed, window.innerWidth)
  } catch {
    return DESKTOP_SIDEBAR_DEFAULT_WIDTH
  }
}

function saveDesktopSidebarWidth(width: number): void {
  if (typeof window === 'undefined') return
  try {
    localStorage.setItem(DESKTOP_SIDEBAR_WIDTH_KEY, String(clampDesktopSidebarWidth(width)))
  } catch {
    /* ignore */
  }
}

if (typeof document !== 'undefined') {
  document.documentElement.classList.toggle('dark', readAppearance() === 'dark')
  document.documentElement.setAttribute('data-ui-theme', readUiTheme())
}

type ExtractAttachmentOptions = {
  chatId: number | null
  signal?: AbortSignal
  onUploadProgress?: (message: string) => void
}

async function extractAttachmentFromUnknown(
  part: unknown,
  opts: ExtractAttachmentOptions,
): Promise<SendAttachmentRef | null> {
  if (!part || typeof part !== 'object') return null
  const obj = part as Record<string, unknown>

  const fileVal = obj.file
  if (fileVal instanceof File) {
    return uploadAttachmentFile(fileVal, opts.chatId, {
      signal: opts.signal,
      onProgress: opts.onUploadProgress,
    })
  }

  const candidateData =
    (typeof obj.data === 'string' ? obj.data : null) ||
    (typeof obj.url === 'string' && String(obj.url).startsWith('data:') ? String(obj.url) : null) ||
    (typeof obj.image === 'string' && String(obj.image).startsWith('data:') ? String(obj.image) : null) ||
    (typeof obj.source === 'string' && String(obj.source).startsWith('data:') ? String(obj.source) : null)

  if (!candidateData) return null

  const filename = typeof obj.filename === 'string' ? obj.filename : undefined
  const mediaType =
    (typeof obj.mediaType === 'string' ? obj.mediaType : undefined) ||
    (typeof obj.mimeType === 'string' ? obj.mimeType : undefined) ||
    (typeof obj.contentType === 'string' ? obj.contentType : undefined) ||
    splitDataUrl(candidateData)?.mimeType

  const file = dataUrlToFile(candidateData, filename)
  if (mediaType && !file.type) {
    return uploadAttachmentFile(
      new File([file], file.name, { type: mediaType }),
      opts.chatId,
      { signal: opts.signal, onProgress: opts.onUploadProgress },
    )
  }
  return uploadAttachmentFile(file, opts.chatId, {
    signal: opts.signal,
    onProgress: opts.onUploadProgress,
  })
}

async function extractLatestUserInput(
  messages: readonly ChatModelRunOptions['messages'][number][],
  opts: ExtractAttachmentOptions,
): Promise<{ text: string; attachments: SendAttachmentRef[]; messageId?: string }> {
  for (let i = messages.length - 1; i >= 0; i -= 1) {
    const message = messages[i]
    if (message.role !== 'user') continue

    const messageId =
      typeof (message as { id?: unknown }).id === 'string'
        ? (message as { id: string }).id
        : undefined

    // Runtime may supply string or non-array shapes; library types are array-only for user messages.
    const content = message.content as unknown
    let text = ''
    const attachments: SendAttachmentRef[] = []

    if (typeof content === 'string') {
      text = content.trim()
    } else if (Array.isArray(content)) {
      const textParts = content
        .map((part) => {
          if (part && typeof part === 'object' && part.type === 'text' && 'text' in part) {
            return typeof (part as { text?: unknown }).text === 'string' ? (part as { text: string }).text : ''
          }
          return ''
        })
      text = textParts.join('\n').trim()
      for (const part of content) {
        const att = await extractAttachmentFromUnknown(part, opts)
        if (att) attachments.push(att)
      }
    } else if (content && typeof content === 'object' && !Array.isArray(content)) {
      // Single part object: { type: 'text', text: '...' }
      const part = content as { type?: string; text?: unknown }
      if (part.type === 'text' && typeof part.text === 'string') {
        text = part.text.trim()
      } else {
        const att = await extractAttachmentFromUnknown(content, opts)
        if (att) attachments.push(att)
      }
    }

    const extraAttachments = (message as { attachments?: unknown }).attachments
    if (Array.isArray(extraAttachments)) {
      for (const part of extraAttachments) {
        const att = await extractAttachmentFromUnknown(part, opts)
        if (att) attachments.push(att)
      }
    }

    if (text.length > 0 || attachments.length > 0) {
      if (import.meta.env?.DEV && typeof console !== 'undefined' && console.debug) {
        console.debug(
          '[extractLatestUserInput]',
          text.slice(0, 80) + (text.length > 80 ? '…' : ''),
          `attachments=${attachments.length}`,
        )
      }
      return { text, attachments, messageId }
    }
  }
  return { text: '', attachments: [] }
}

type LoadHistoryFn = (
  cid?: number | null,
  personaId?: number | null,
  day?: string | null,
  opts?: {
    force?: boolean
    limitOverride?: number
    sessionId?: string | null
    aroundId?: string
    aroundBefore?: number
    aroundAfter?: number
    beforeId?: string
    afterId?: string
    replace?: boolean
  },
) => Promise<void>


export function App({
  onAuthErrorRef,
}: {
  onAuthErrorRef?: React.MutableRefObject<(message: string) => void>
}) {
  const { requestConfirm, confirmDialog } = useConfirmDialog()
  useVisualViewportFrame()
  const [appearance, setAppearance] = useState<Appearance>(readAppearance())
  const [uiTheme, setUiTheme] = useState<UiTheme>(readUiTheme())
  const [chatId, setChatId] = useState<number | null>(null)
  const [error, setError] = useState<string>('')
  const [statusText, setStatusText] = useState<string>('Idle')
  const [sidebarMode, setSidebarMode] = useState<SidebarMode>(readSidebarMode)
  const [hideScheduled, setHideScheduled] = useState(false)

  useEffect(() => {
    if (onAuthErrorRef) onAuthErrorRef.current = setError
  }, [onAuthErrorRef])

  useEffect(() => {
    const onHash = () => setSidebarMode(readSidebarMode())
    window.addEventListener('hashchange', onHash)
    return () => window.removeEventListener('hashchange', onHash)
  }, [])

  const setMode = useCallback((mode: SidebarMode) => {
    writeSidebarMode(mode)
    setSidebarMode(mode)
  }, [])

  const loadHistoryRef = useRef<LoadHistoryFn | null>(null)
  const loadPersonaBulletinRef = useRef<((pid: number) => Promise<void>) | null>(null)
  const resetHistoryPaginationRef = useRef<(() => void) | null>(null)
  const setHistoryLoadingRef = useRef<(loading: boolean) => void>(() => {})

  const persona = usePersonaSession({
    chatId,
    setHistoryLoading: (loading) => setHistoryLoadingRef.current(loading),
    loadHistory: (...args) => {
      const fn = loadHistoryRef.current
      return fn ? fn(...args) : Promise.resolve()
    },
    resetHistoryPagination: () => resetHistoryPaginationRef.current?.(),
    loadPersonaBulletin: (pid) => {
      const fn = loadPersonaBulletinRef.current
      return fn ? fn(pid) : Promise.resolve()
    },
    requestConfirm,
    setError,
    setStatusText,
  })

  const ops = useOperatorOps({
    activePersonaId: persona.activePersonaId,
    setError,
    setStatusText,
  })

  const chat = useChatHistory({
    chatId,
    activePersonaId: persona.activePersonaId,
    activeSessionId: persona.activeSessionId,
    setError,
    setStatusText,
    requestConfirm,
    setPersonaBookmarks: ops.setPersonaBookmarks,
    hideScheduled,
  })

  useLayoutEffect(() => {
    loadHistoryRef.current = chat.loadHistory
    loadPersonaBulletinRef.current = ops.loadPersonaBulletin
    resetHistoryPaginationRef.current = chat.resetHistoryPagination
    setHistoryLoadingRef.current = chat.setHistoryLoading
  }, [
    chat.loadHistory,
    chat.resetHistoryPagination,
    chat.setHistoryLoading,
    ops.loadPersonaBulletin,
  ])

  const {
    personas,
    setPersonas,
    activePersonaId,
    setActivePersonaId,
    activeSessionId,
    setActiveSessionId,
    activeSessionIdRef,
    chatSessions,
    activePersonaName,
    personaHasNew,
    markPersonaRead,
    loadPersonas,
    loadSessions,
    switchPersona,
    handleSelectSession,
    handleCreateSession,
    handleDeleteSession,
    onCreatePersona,
    onRenamePersona,
    onDeletePersona,
    newSchedulePersonaId,
    setNewSchedulePersonaId,
  } = persona
  const personasRef = useRef(personas)
  personasRef.current = personas

  const {
    bulletinFocus,
    personaBookmarks,
    bulletinHistorySuffix,
    bulletinOperatorMemo,
    loadPersonaBulletin,
    reloadPersonaBulletin,
    removePersonaBookmark,
    toggleMessageBookmark,
  } = ops

  const {
    historySeed,
    historyMode,
    historyHasOlder,
    historyHasNewer,
    newMessagesPending,
    historyLoadingMore,
    historyLoadingNewer,
    historyLoading,
    setHistoryLoading,
    loadHistory,
    loadOlder,
    loadNewer,
    jumpToLatest,
    loadWindowAround,
    resetHistoryPagination,
    handleReplyToMessage,
    handleDismissPendingReply,
    handleDeleteMessage,
    handleSaveMessageEdit,
    activeDraftText,
    activePendingReply,
    handleDraftTextChange,
    pendingReplyRef,
    formatReplyForSend,
    setDraftByThreadKey,
    setPendingReplyByThreadKey,
  } = chat

  const [composerAlert, setComposerAlert] = useState<string | null>(null)
  const alertDraftRef = useRef<string | null>(null)
  const showComposerAlert = useCallback((text: string, draft: string) => {
    alertDraftRef.current = draft
    setComposerAlert(text)
  }, [])
  const onComposerDraftChange = useCallback((text: string) => {
    if (alertDraftRef.current != null && text !== alertDraftRef.current) {
      alertDraftRef.current = null
      setComposerAlert(null)
    }
    handleDraftTextChange(text)
  }, [handleDraftTextChange])
  const composerNotice = useMemo<ComposerNotice | null>(
    () => (composerAlert ? { tone: 'alert', text: composerAlert } : null),
    [composerAlert],
  )
  useEffect(() => {
    alertDraftRef.current = null
    setComposerAlert(null)
  }, [activePersonaId])

  const side = useSideChats({
    activePersonaId,
    activeSessionId,
    chatId,
    setError,
    setStatusText,
  })

  const {
    sideChats,
    sideChatsLoading,
    activeSideChat,
    activeTurns,
    setActiveTurns,
    activeDraft,
    setActiveDraft,
    setActiveDraftLocal,
    openSideChatForAnchor,
    openSideChatById,
    closeSideChatPane,
    deleteSideChat,
    refreshAfterSend,
  } = side

  const [editingMessageId, setEditingMessageId] = useState<string | null>(null)
  const [rewrittenMessages, setRewrittenMessages] = useState<
    Record<string, { original: string; rewritten: string }>
  >({})
  const [threadSurface, setThreadSurface] = useState<ThreadSurface>(() =>
    readThreadSurface(typeof localStorage === 'undefined' ? null : localStorage),
  )
  const [dossierTopicId, setDossierTopicId] = useState<string | null>(null)
  const [dossierCursor, setDossierCursor] = useState<DossierCursor>({ kind: 'latest' })
  const [dossierHighlightId, setDossierHighlightId] = useState<string | null>(null)
  const [dossierPageTopicId, setDossierPageTopicId] = useState<string | null>(null)
  const threadSurfaceRef = useRef(threadSurface)
  threadSurfaceRef.current = threadSurface
  const sideChatOpenerRef = useRef<HTMLElement | null>(null)
  const narrowViewport = useNarrowViewport()
  const chooseThreadSurface = useCallback((next: ThreadSurface) => {
    setThreadSurface(next)
    writeThreadSurface(next, typeof localStorage === 'undefined' ? null : localStorage)
  }, [])
  const rememberSideChatOpener = useCallback(() => {
    sideChatOpenerRef.current =
      document.activeElement instanceof HTMLElement ? document.activeElement : null
  }, [])
  const closeSideChat = useCallback(() => {
    closeSideChatPane()
    const opener = sideChatOpenerRef.current
    sideChatOpenerRef.current = null
    if (opener && opener.isConnected) {
      requestAnimationFrame(() => opener.focus())
    }
  }, [closeSideChatPane])
  // Armed message id for a bookmark/anchor jump. ThreadPane performs the centered
  // scroll (with auto-scroll-to-bottom suppressed) once the element is mounted.
  const [targetScrollMessageId, setTargetScrollMessageId] = useState<string | null>(null)
  // Suppress auto-scroll while history is loading for a jump (before target is armed).
  const [jumpSuppressAutoScroll, setJumpSuppressAutoScroll] = useState(false)

  const handleTargetScrollHandled = useCallback(
    (found: boolean) => {
      setTargetScrollMessageId(null)
      setJumpSuppressAutoScroll(false)
      if (found) {
        setStatusText('Jumped to message')
      } else {
        setError('Could not find that message in the chat window')
        setStatusText('Idle')
      }
    },
    [setError, setStatusText],
  )

  const revealMessageInThread = useCallback(
    async (messageId: string) => {
      if (!messageId) return
      if (findMessageElement(messageId)) {
        setJumpSuppressAutoScroll(true)
        setTargetScrollMessageId(messageId)
        return
      }
      setJumpSuppressAutoScroll(true)
      setStatusText('Loading message…')
      try {
        const meta = await api<{
          message?: BackendMessage
          messages_from?: number
        }>(
          `/api/personas/${activePersonaId}/messages/${encodeURIComponent(messageId)}`,
        )
        const sessionId =
          typeof meta.message?.session_id === 'string' && meta.message.session_id.trim()
            ? meta.message.session_id.trim()
            : null
        if (sessionId !== (activeSessionId ?? null)) {
          await handleSelectSession(sessionId)
        }
        if (findMessageElement(messageId)) {
          setTargetScrollMessageId(messageId)
          return
        }
        const { ok } = await loadWindowAround(messageId, sessionId)
        if (!ok) {
          setJumpSuppressAutoScroll(false)
          setStatusText('Idle')
          return
        }
        // Arm after the window is loaded so the scroll effect does not time out mid-fetch.
        setTargetScrollMessageId(messageId)
      } catch (e) {
        setJumpSuppressAutoScroll(false)
        setTargetScrollMessageId(null)
        const msg = e instanceof Error ? e.message : String(e)
        if (/not found/i.test(msg)) {
          setStatusText('Idle')
          requestConfirm({
            title: 'Bookmarked message missing',
            description:
              'This bookmarked message was deleted. Remove the bookmark from your list?',
            confirmLabel: 'Remove bookmark',
            destructive: true,
            onConfirm: async () => {
              await removePersonaBookmark(messageId)
            },
          })
        } else {
          setError(msg)
          setStatusText('Idle')
        }
      }
    },
    [
      activePersonaId,
      activeSessionId,
      handleSelectSession,
      loadWindowAround,
      removePersonaBookmark,
      requestConfirm,
      setError,
      setStatusText,
    ],
  )

  const handleOpenSubthread = useCallback(
    async (messageId: string) => {
      rememberSideChatOpener()
      await openSideChatForAnchor(messageId)
      if (threadSurfaceRef.current !== 'focus') {
        void revealMessageInThread(messageId)
      }
    },
    [openSideChatForAnchor, rememberSideChatOpener, revealMessageInThread],
  )

  const handleSelectSideChat = useCallback(
    async (sideChatId: string) => {
      rememberSideChatOpener()
      const sc = await openSideChatById(sideChatId)
      if (!sc) return
      const targetSession = sc.session_id ?? null
      if (targetSession !== (activeSessionId ?? null)) {
        await handleSelectSession(targetSession)
      }
      if (threadSurfaceRef.current !== 'focus') {
        void revealMessageInThread(sc.anchor_message_id)
      }
    },
    [activeSessionId, handleSelectSession, openSideChatById, rememberSideChatOpener, revealMessageInThread],
  )

  const handleAddSideChatTurnToMain = useCallback(
    async (turnId: string) => {
      if (activePersonaId == null || !activeSideChat) {
        throw new Error('No active side chat')
      }
      const data = await api<{ message_id?: string }>(
        `/api/personas/${activePersonaId}/side_chats/${encodeURIComponent(activeSideChat.id)}/promote_turn`,
        {
          method: 'POST',
          body: JSON.stringify({ turn_id: turnId }),
        },
      )
      const messageId = typeof data.message_id === 'string' ? data.message_id.trim() : ''
      if (!messageId) {
        throw new Error('Promote succeeded but no message_id returned')
      }
      await loadHistory(chatId, activePersonaId, null, {
        force: true,
        limitOverride: HISTORY_PAGE_SIZE,
      })
      await revealMessageInThread(messageId)
    },
    [activePersonaId, activeSideChat, chatId, loadHistory, revealMessageInThread],
  )

  useEffect(() => {
    setEditingMessageId(null)
  }, [activePersonaId, activeSessionId, chatId])

  const [replayNotice, setReplayNotice] = useState<string>('')
  const [schedules, setSchedules] = useState<ScheduleTask[]>([])
  const [schedulesDialogOpen, setSchedulesDialogOpen] = useState<boolean>(false)
  const [schedulesShowArchived, setSchedulesShowArchived] = useState(false)
  const [inboxDialogOpen, setInboxDialogOpen] = useState(false)
  const [inboxTodos, setInboxTodos] = useState<PersonaTodo[]>([])
  const [inboxLoading, setInboxLoading] = useState(false)
  const [inboxBusyTodoId, setInboxBusyTodoId] = useState<number | null>(null)
  const [boardFocusTodoId, setBoardFocusTodoId] = useState<number | null>(null)
  const [memoryDialogOpen, setMemoryDialogOpen] = useState<boolean>(false)
  const [artifactsDialogOpen, setArtifactsDialogOpen] = useState<boolean>(false)
  useEffect(() => {
    const establish = () => { void ensureArtifactSession().catch(() => {}) }
    establish()
    window.addEventListener('focus', establish)
    const timer = setInterval(establish, 25 * 60 * 1000)
    return () => { window.removeEventListener('focus', establish); clearInterval(timer) }
  }, [])
  const [terminalDialogOpen, setTerminalDialogOpen] = useState<boolean>(false)
  const [terminalError, setTerminalError] = useState<string>('')
  const [memoryContent, setMemoryContent] = useState<string>('')
  const [memoryMtimeMs, setMemoryMtimeMs] = useState<number | null>(null)
  const [memoryPathHint, setMemoryPathHint] = useState<string>('')
  const [memoryBusy, setMemoryBusy] = useState<boolean>(false)
  const [memoryError, setMemoryError] = useState<string>('')
  const [agentHistoryDialogOpen, setAgentHistoryDialogOpen] = useState(false)
  const [agentHistoryTab, setAgentHistoryTab] = useState<'trace' | 'prompt' | 'evaluators'>('trace')
  const [agentHistoryBusy, setAgentHistoryBusy] = useState(false)
  const [agentHistoryError, setAgentHistoryError] = useState('')
  const [agentHistoryRaw, setAgentHistoryRaw] = useState('')
  const [agentHistoryParsed, setAgentHistoryParsed] = useState<ParsedAgentHistory | null>(null)
  const [agentHistoryPathHint, setAgentHistoryPathHint] = useState('')
  const [agentHistoryFilename, setAgentHistoryFilename] = useState('')
  const [agentHistoryMtimeMs, setAgentHistoryMtimeMs] = useState<number | null>(null)
  const [agentHistoryIterationIdx, setAgentHistoryIterationIdx] = useState(0)
  const [agentHistoryOptimizeBusy, setAgentHistoryOptimizeBusy] = useState(false)
  const [agentHistoryOptimizeNotes, setAgentHistoryOptimizeNotes] = useState('')
  const [newSchedulePrompt, setNewSchedulePrompt] = useState('')
  const [newScheduleType, setNewScheduleType] = useState<'cron' | 'once'>('cron')
  const [newScheduleValue, setNewScheduleValue] = useState('0 9 * * *')
  const [newScheduleDelivery, setNewScheduleDelivery] = useState<'runs_only' | 'transcript'>('runs_only')
  const [bindings, setBindings] = useState<ChannelBinding[]>([])
  const [pendingRuns, setPendingRuns] = useState<PendingRun[]>([])
  const [stoppingRunIds, setStoppingRunIds] = useState<string[]>([])
  const [stoppingBackgroundJobIds, setStoppingBackgroundJobIds] = useState<string[]>([])
  const [queueDialogOpen, setQueueDialogOpen] = useState(false)
  const [queueShowAllPersonas, setQueueShowAllPersonas] = useState(false)
  const [scheduleDetailTask, setScheduleDetailTask] = useState<ScheduleTask | null>(null)
  const [scheduleDetailPrompt, setScheduleDetailPrompt] = useState('')
  const [scheduleDetailScheduleType, setScheduleDetailScheduleType] = useState<'cron' | 'once'>('cron')
  const [scheduleDetailScheduleValue, setScheduleDetailScheduleValue] = useState('')
  const [scheduleDetailBusy, setScheduleDetailBusy] = useState(false)
  const [agentsMdOpen, setAgentsMdOpen] = useState(false)
  const [agentsMdContent, setAgentsMdContent] = useState('')
  const [agentsMdMtimeMs, setAgentsMdMtimeMs] = useState<number | null>(null)
  const [agentsMdPath, setAgentsMdPath] = useState('')
  const [agentsMdBusy, setAgentsMdBusy] = useState(false)
  const [agentsMdError, setAgentsMdError] = useState('')
  const [settingsDialogOpen, setSettingsDialogOpen] = useState(false)
  const [settingsError, setSettingsError] = useState('')
  const [settingsPage, setSettingsPage] = useState<SettingsLedgerPage>('overview')
  const [settingsReveal, setSettingsReveal] = useState(0)
  const [personaDialogOpen, setPersonaDialogOpen] = useState(false)
  const [personaError, setPersonaError] = useState('')
  const [personaPage, setPersonaPage] = useState<PersonaLedgerPage>('profile')
  const [personaReveal, setPersonaReveal] = useState(0)
  const [installationStatus, setInstallationStatus] = useState<InstallationStatus | null>(null)
  const [restartBusy, setRestartBusy] = useState(false)
  const [restartNotice, setRestartNotice] = useState<string | null>(null)
  const [mobileNavOpen, setMobileNavOpen] = useState(false)
  const [mobileOpsOpen, setMobileOpsOpen] = useState(false)
  const [shortcutsOpen, setShortcutsOpen] = useState(false)
  const { open: commandPaletteOpen, setOpen: setCommandPaletteOpen } = useCommandPaletteOpenState()
  const [newPersonaOpen, setNewPersonaOpen] = useState(false)
  const [cockpitExpanded, setCockpitExpanded] = useState(false)
  const [desktopSidebarOpen, setDesktopSidebarOpen] = useState<boolean>(readDesktopSidebarOpen)
  const [desktopSidebarWidth, setDesktopSidebarWidth] = useState<number>(readDesktopSidebarWidth)
  const [desktopSidebarResizing, setDesktopSidebarResizing] = useState(false)
  const [onboardingDismissed, setOnboardingDismissed] = useState(() => {
    if (typeof sessionStorage === 'undefined') return false
    try {
      return sessionStorage.getItem('meido_onboarding_banner_dismissed') === '1'
    } catch {
      return false
    }
  })
  useEffect(() => {
    saveDesktopSidebarOpen(desktopSidebarOpen)
  }, [desktopSidebarOpen])
  useEffect(() => {
    saveDesktopSidebarWidth(desktopSidebarWidth)
  }, [desktopSidebarWidth])
  useEffect(() => {
    const onResize = () => {
      setDesktopSidebarWidth((current) => clampDesktopSidebarWidth(current, window.innerWidth))
    }
    window.addEventListener('resize', onResize)
    return () => {
      window.removeEventListener('resize', onResize)
    }
  }, [])

  const beginDesktopSidebarResize = useCallback((event: React.PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return
    event.preventDefault()
    const startX = event.clientX
    const startWidth = desktopSidebarWidth
    setDesktopSidebarResizing(true)
    const previousCursor = document.body.style.cursor
    const previousUserSelect = document.body.style.userSelect
    document.body.style.cursor = 'col-resize'
    document.body.style.userSelect = 'none'

    const onPointerMove = (moveEvent: PointerEvent) => {
      const deltaX = moveEvent.clientX - startX
      setDesktopSidebarWidth(clampDesktopSidebarWidth(startWidth + deltaX, window.innerWidth))
    }
    const stopResizing = () => {
      window.removeEventListener('pointermove', onPointerMove)
      window.removeEventListener('pointerup', stopResizing)
      window.removeEventListener('pointercancel', stopResizing)
      document.body.style.cursor = previousCursor
      document.body.style.userSelect = previousUserSelect
      setDesktopSidebarResizing(false)
    }

    window.addEventListener('pointermove', onPointerMove)
    window.addEventListener('pointerup', stopResizing)
    window.addEventListener('pointercancel', stopResizing)
  }, [desktopSidebarWidth])

  const docVisible = useDocumentVisible()
  const [focusTopicId, setFocusTopicId] = useState<string | null>(null)
  const [topicsSheetOpen, setTopicsSheetOpen] = useState(false)
  const pendingRunsForActivePersona = useMemo(
    () =>
      pendingRuns.filter(
        (r) => activePersonaId != null && activePersonaId > 0 && r.personaId === activePersonaId,
      ),
    [pendingRuns, activePersonaId],
  )
  const topics = useConversationTopics(
    activePersonaId,
    activeSessionId,
    pendingRunsForActivePersona.length > 0,
  )
  const topicStarts = useMemo(() => {
    const starts: Record<string, { id: string; title: string }> = {}
    for (const topic of topics.topics) {
      starts[topic.start_message_id] = { id: topic.id, title: topic.title }
    }
    return starts
  }, [topics.topics])
  const dossierTopic = useMemo(
    () => activeDossierTopic(topics.topics, dossierTopicId),
    [dossierTopicId, topics.topics],
  )
  const dossierTopicPageId = dossierTopic?.id ?? null
  if (dossierPageTopicId !== dossierTopicPageId) {
    setDossierPageTopicId(dossierTopicPageId)
    setDossierCursor({ kind: 'latest' })
    setDossierHighlightId(null)
  }
  const dossierQuery = useTopicDossier(
    activePersonaId,
    activeSessionId,
    dossierTopic?.id ?? null,
    dossierCursor,
    threadSurface === 'focus',
  )
  const handleTopicJump = useCallback((messageId: string) => {
    if (threadSurfaceRef.current === 'focus') {
      const topic = topics.topics.find((entry) => entry.start_message_id === messageId)
      if (topic) {
        setDossierTopicId(topic.id)
        setFocusTopicId(topic.id)
      }
      return
    }
    void revealMessageInThread(messageId)
  }, [revealMessageInThread, topics.topics])
  const handleDossierEarlier = useCallback(() => {
    const oldest = dossierQuery.data?.messages[0]?.id
    if (!oldest) return
    setDossierHighlightId(null)
    setDossierCursor({ kind: 'before', messageId: oldest })
  }, [dossierQuery.data?.messages])
  const handleDossierLater = useCallback(() => {
    const messages = dossierQuery.data?.messages ?? []
    const newest = messages[messages.length - 1]?.id
    if (!newest) return
    setDossierHighlightId(null)
    setDossierCursor({ kind: 'after', messageId: newest })
  }, [dossierQuery.data?.messages])
  const handleDossierAround = useCallback((messageId: string) => {
    setDossierHighlightId(messageId)
    setDossierCursor({ kind: 'around', messageId })
  }, [])
  const handleCreateTopicSession = useCallback(async (topicId?: string, title = '') => {
    const id = topicId?.trim() || dossierTopic?.id
    if (!id || chatId == null || activePersonaId == null) {
      throw new Error('Choose a persona before starting a session')
    }
    const created = await handleCreateSession(title.trim(), false, { sourceTopicId: id })
    if (!created) throw new Error('Could not create a session')
    setTopicsSheetOpen(false)
    chooseThreadSurface('transcript')
  }, [activePersonaId, chatId, chooseThreadSurface, dossierTopic?.id, handleCreateSession])
  useEffect(() => {
    if (dossierQuery.isError && dossierCursor.kind !== 'latest') {
      setDossierCursor({ kind: 'latest' })
      setDossierHighlightId(null)
    }
  }, [dossierCursor.kind, dossierQuery.isError])
  const handleRevealTopic = useCallback((topicId: string) => {
    setFocusTopicId(topicId)
  }, [])
  const handleClearTopicFocus = useCallback(() => {
    setFocusTopicId(null)
  }, [])
  const handleDeleteTopic = useCallback((topicId: string) => {
    const topic = topics.topics.find((item) => item.id === topicId)
    const title = topic?.title.trim() || 'this topic'
    requestConfirm({
      title: `Delete "${title}"?`,
      description:
        'This deletes the topic and every message in its range, including bookmarks and side chats anchored on those messages. This cannot be undone.',
      confirmLabel: 'Delete topic and messages',
      destructive: true,
      onConfirm: async () => {
        try {
          const deleted = await topics.remove(topicId)
          if (focusTopicId === topicId) setFocusTopicId(null)
          if (dossierTopicId === topicId) setDossierTopicId(null)
          await chat.loadHistory(chatId, activePersonaId, null, { replace: true })
          if (activePersonaId != null) await ops.loadPersonaBulletin(activePersonaId)
          toast.success(deleted === 1 ? 'Deleted 1 message' : `Deleted ${deleted} messages`)
        } catch (err) {
          toast.error(err instanceof Error ? err.message : 'Could not delete topic')
        }
      },
    })
  }, [
    activePersonaId,
    chat.loadHistory,
    chatId,
    dossierTopicId,
    focusTopicId,
    ops.loadPersonaBulletin,
    requestConfirm,
    topics.remove,
    topics.topics,
  ])
  const handleSplitTopic = useCallback(async (messageId: string) => {
    if (activePersonaId == null) return
    const params = new URLSearchParams()
    if (activeSessionId) params.set('session_id', activeSessionId)
    const query = params.toString()
    try {
      await api(`/api/personas/${activePersonaId}/topics/split${query ? `?${query}` : ''}`, {
        method: 'POST',
        body: JSON.stringify({ message_id: messageId }),
      })
      await topics.invalidate()
    } catch (err) {
      toast.error(err instanceof Error ? err.message : 'Could not split topic')
    }
  }, [activePersonaId, activeSessionId, topics.invalidate])
  const handleOpenTopics = useCallback(() => {
    setTopicsSheetOpen(true)
  }, [])
  const { queueLane, queueLanesAll, otherPersonasPending, backgroundActiveCount, backgroundJobs, queueSampledAt, unreadRuns, blockedItems, invalidateOps, clearUnreadRuns } =
    useOpsPoll({
      chatId,
      activePersonaId,
      docVisible,
      pendingRunsForActivePersona: pendingRunsForActivePersona.length,
      setPersonas,
    })

  const queueDialogItems = useMemo(() => {
    if (queueShowAllPersonas) {
      const merged: QueueItem[] = []
      for (const lane of queueLanesAll) {
        for (const it of lane.items ?? []) {
          merged.push(it)
        }
      }
      return merged
    }
    return queueLane?.items ?? []
  }, [queueShowAllPersonas, queueLanesAll, queueLane?.items])

  const selectedSessionReadOnly = false

  const schedulesFiltered = useMemo(() => {
    if (schedulesShowArchived) return schedules
    return schedules.filter((t) => t.status !== 'completed' && t.status !== 'cancelled')
  }, [schedules, schedulesShowArchived])

  const inboxUnread = useMemo(() => {
    return personas
      .filter((p) => personaHasNew[p.id])
      .map((p) => ({
        personaId: p.id,
        personaName: p.name,
        lastBotMessageAt: p.last_bot_message_at ?? null,
        sessionId: p.last_bot_message_session_id ?? null,
        sessionTitle: p.last_bot_message_session_title ?? null,
      }))
  }, [personas, personaHasNew])

  const inboxBadgeCount = inboxUnread.length + inboxTodos.length

  const backgroundJobsVisible = useMemo<BackgroundJobItem[]>(() => backgroundJobs.slice(0, 20), [backgroundJobs])

  function isActiveBackgroundJobStatus(status: string): boolean {
    return (
      status === 'pending'
      || status === 'running'
      || status === 'completed_raw'
      || status === 'main_agent_processing'
    )
  }

  function isTerminalBackgroundJobStatus(status: string): boolean {
    return status === 'done' || status === 'failed' || status === 'cancelled'
  }

  const prevBgJobStatusByIdRef = useRef<Map<string, string>>(new Map())

  async function loadPersonaMemory(pid: number): Promise<void> {
    setMemoryBusy(true)
    setMemoryError('')
    try {
      const data = await api<{ content?: string; mtime_ms?: number; path?: string }>(`/api/personas/${pid}/memory`)
      setMemoryContent(typeof data.content === 'string' ? data.content : '')
      setMemoryMtimeMs(typeof data.mtime_ms === 'number' ? data.mtime_ms : null)
      setMemoryPathHint(typeof data.path === 'string' ? data.path : '')
    } catch (e) {
      setMemoryError(e instanceof Error ? e.message : String(e))
    } finally {
      setMemoryBusy(false)
    }
  }

  async function savePersonaMemory(pid: number): Promise<void> {
    setMemoryBusy(true)
    setMemoryError('')
    try {
      const res = await api<{ mtime_ms?: number }>(`/api/personas/${pid}/memory`, {
        method: 'PUT',
        body: JSON.stringify({
          content: memoryContent,
          if_match_mtime_ms: memoryMtimeMs ?? undefined,
        }),
      })
      if (typeof res.mtime_ms === 'number') {
        setMemoryMtimeMs(res.mtime_ms)
      }
      setStatusText('Memory saved')
    } catch (e) {
      setMemoryError(e instanceof Error ? e.message : String(e))
    } finally {
      setMemoryBusy(false)
    }
  }

  async function loadAgentHistoryLatest(pid: number): Promise<void> {
    setAgentHistoryBusy(true)
    setAgentHistoryError('')
    try {
      const data = await api<{
        content?: string
        path?: string
        filename?: string
        mtime_ms?: number
      }>(`/api/personas/${pid}/agent_history/latest`)
      const raw = typeof data.content === 'string' ? data.content : ''
      setAgentHistoryPathHint(typeof data.path === 'string' ? data.path : '')
      setAgentHistoryFilename(typeof data.filename === 'string' ? data.filename : '')
      setAgentHistoryMtimeMs(typeof data.mtime_ms === 'number' ? data.mtime_ms : null)
      const { traceMarkdown, qualityEvalMarkdown, initialPromptJson } = splitAgentHistoryRaw(raw)
      const parsedTrace = parseAgentHistoryMarkdown(traceMarkdown)
      setAgentHistoryRaw(traceMarkdown)
      setAgentHistoryParsed({
        ...parsedTrace,
        initialPromptJson,
        pteDecisions: parsePteDecisions(parsedTrace),
        pdqeSteps: parsePdqeSteps(qualityEvalMarkdown),
      })
      setAgentHistoryIterationIdx(0)
      setAgentHistoryTab((prev) => (prev === 'prompt' && initialPromptJson == null ? 'trace' : prev))
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e)
      const isEmpty = /no agent history for this persona/i.test(msg)
      setAgentHistoryError(
        isEmpty ? 'No saved agent run history for this persona yet.' : msg,
      )
      setAgentHistoryRaw('')
      setAgentHistoryParsed(null)
      setAgentHistoryPathHint('')
      setAgentHistoryFilename('')
      setAgentHistoryMtimeMs(null)
    } finally {
      setAgentHistoryBusy(false)
    }
  }

  async function optimizeAgentHistoryLatest(
    pid: number,
    operatorNotes?: string,
  ): Promise<void> {
    setAgentHistoryOptimizeBusy(true)
    setAgentHistoryError('')
    try {
      const body: AgentHistoryOptimizeRequest | undefined =
        operatorNotes && operatorNotes.trim()
          ? { operator_notes: operatorNotes.trim() }
          : undefined
      const data = await api<AgentHistoryOptimizeResponse>(
        `/api/personas/${pid}/agent_history/latest/optimize`,
        {
          method: 'POST',
          ...(body
            ? {
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify(body),
              }
            : {}),
        },
      )
      const jobId = typeof data.job_id === 'string' ? data.job_id : ''
      const msg =
        typeof data.message === 'string' && data.message.trim()
          ? data.message.trim()
          : 'Learn & optimize queued.'
      setReplayNotice(jobId ? `${msg} (job ${jobId})` : msg)
      if (chatId != null) void invalidateOps(chatId)
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e)
      setAgentHistoryError(msg)
    } finally {
      setAgentHistoryOptimizeBusy(false)
    }
  }

  const adapter = useMemo<ChatModelAdapter>(
    () => ({
      run: async function* (options): AsyncGenerator<ChatModelRunResult, void> {
        let userText = ''
        let attachments: SendAttachmentRef[] = []
        let localUserMessageId: string | undefined
        try {
          const extracted = await extractLatestUserInput(options.messages, {
            chatId,
            signal: options.abortSignal,
            onUploadProgress: (msg) => setStatusText(msg),
          })
          userText = extracted.text
          attachments = extracted.attachments
          localUserMessageId = extracted.messageId
        } catch (e) {
          // Upload failed/stalled/aborted while extracting attachments. Reset the
          // composer state so the app does not stay stuck on "Uploading…".
          if (options.abortSignal.aborted) {
            setStatusText('Idle')
            return
          }
          const msg = e instanceof Error ? e.message : String(e)
          setError(`Attachment upload failed: ${msg}`)
          setStatusText('Error')
          yield { content: [{ type: 'text', text: `Error: ${msg}` }] }
          return
        }
        const pendingReply = pendingReplyRef.current
        const messageText = pendingReply ? formatReplyForSend(pendingReply, userText) : userText
        if (!messageText.trim() && attachments.length === 0) return

        if (historyMode === 'anchored') {
          await jumpToLatest()
        }

        setStatusText(attachments.length > 0 ? 'Sending message…' : 'Sending...')
        setReplayNotice('')
        setError('')

        try {
          if (selectedSessionReadOnly) {
            setStatusText('Read-only channel')
            throw new Error('This chat is read-only. Switch to a web session or create a new chat to send messages.')
          }

          const sendBody: {
            chat_id?: number
            persona_id?: number
            sender_name: string
            message: string
            attachments?: SendAttachmentRef[]
          } = {
            sender_name: 'web-user',
            message: messageText,
          }
          if (chatId != null) sendBody.chat_id = chatId
          if (activePersonaId != null && activePersonaId > 0) sendBody.persona_id = activePersonaId
          if (activeSessionIdRef.current) (sendBody as Record<string, unknown>).session_id = activeSessionIdRef.current
          if (attachments.length > 0) sendBody.attachments = attachments
          const guestMentions = parseGuestMentions(messageText, personasRef.current, activePersonaId)
          if (guestMentions.speakers.length > GUEST_MAX_PER_TURN) {
            const threadKey = `${chatId ?? 0}:${activePersonaId ?? 0}`
            setDraftByThreadKey((prev) => ({ ...prev, [threadKey]: messageText }))
            showComposerAlert(GUEST_TURN_CAP_NOTICE, messageText)
            setStatusText('Idle')
            return
          }
          if (guestMentions.speakers.length > 0) {
            (sendBody as Record<string, unknown>).guest_persona_ids = guestMentions.speakers.map(
              (speaker) => speaker.id,
            )
          }
          let sendResponse: { run_id?: string }
          try {
            sendResponse = await api<{ run_id?: string }>('/api/send_stream', {
              method: 'POST',
              body: JSON.stringify(sendBody),
              signal: options.abortSignal,
            })
          } catch (e) {
            if (options.abortSignal.aborted) {
              setStatusText('Idle')
              return
            }
            const msg = e instanceof Error ? e.message : String(e)
            const threadKey = `${chatId ?? 0}:${activePersonaId ?? 0}`
            setDraftByThreadKey((prev) => ({ ...prev, [threadKey]: messageText }))
            showComposerAlert(msg, messageText)
            setStatusText('Idle')
            return
          }

          const runId = sendResponse.run_id
          if (!runId) {
            throw new Error('missing run_id')
          }
          const threadKey = `${chatId ?? 0}:${activePersonaId ?? 0}`
          setDraftByThreadKey((prev) => {
            if (!prev[threadKey]) return prev
            return { ...prev, [threadKey]: '' }
          })
          setPendingReplyByThreadKey((prev) => {
            if (!prev[threadKey]) return prev
            const next = { ...prev }
            delete next[threadKey]
            return next
          })
          pendingReplyRef.current = null
          const pid = activePersonaId != null && activePersonaId > 0 ? activePersonaId : 0
          setPendingRuns((prev) =>
            prev.some((r) => r.runId === runId)
              ? prev
              : [...prev, { runId, personaId: pid }],
          )
          setStatusText('Queued')
          // Create assistant bubble immediately; SSE deltas will append real text.
          yield { content: [{ type: 'text', text: '' }] }

          const chatIdForRun = chatId
          const personaIdForRun = activePersonaId

          const parseJsonObject = (raw: string): Record<string, unknown> | null => {
            try {
              const parsed = JSON.parse(raw) as Record<string, unknown>
              if (parsed && typeof parsed === 'object') return parsed
              return null
            } catch {
              return null
            }
          }

          type SseEvent = { event: string; data: string; id?: string }

          async function* parseSseEvents(resp: Response): AsyncGenerator<SseEvent, void, unknown> {
            const body = resp.body
            if (!body) return

            const reader = body.getReader()
            const decoder = new TextDecoder('utf-8')

            let buffer = ''
            let eventName: string | undefined
            let eventId: string | undefined
            let dataLines: string[] = []

            while (true) {
              const { value, done } = await reader.read()
              if (done) break
              buffer += decoder.decode(value, { stream: true })

              while (true) {
                const newlineIdx = buffer.indexOf('\n')
                if (newlineIdx < 0) break

                const line = buffer.slice(0, newlineIdx).replace(/\r$/, '')
                buffer = buffer.slice(newlineIdx + 1)

                if (line === '') {
                  if (dataLines.length > 0) {
                    yield {
                      event: eventName ?? 'message',
                      data: dataLines.join('\n'),
                      id: eventId,
                    }
                  }
                  eventName = undefined
                  eventId = undefined
                  dataLines = []
                  continue
                }

                if (line.startsWith(':')) continue
                if (line.startsWith('event:')) {
                  eventName = line.slice('event:'.length).trim()
                  continue
                }
                if (line.startsWith('id:')) {
                  eventId = line.slice('id:'.length).trim()
                  continue
                }
                if (line.startsWith('data:')) {
                  dataLines.push(line.slice('data:'.length).trimStart())
                }
              }
            }

            if (dataLines.length > 0) {
              yield { event: eventName ?? 'message', data: dataLines.join('\n'), id: eventId }
            }
          }

          let completedOrError = false
          let seenAnyDelta = false

          let pendingDelta = ''
          let lastFlushMs = Date.now()
          const sseSubscribeStartMs = Date.now()
          let firstDeltaLatencyMs: number | null = null

          const sseUrl = `/api/stream?run_id=${encodeURIComponent(runId)}`
          const sseResp = await fetch(sseUrl, { headers: makeHeaders(), signal: options.abortSignal })
          if (!sseResp.ok) {
            throw new Error(`stream subscribe failed (HTTP ${sseResp.status})`)
          }

          let storedUserMessageId = ''

          try {
            for await (const evt of parseSseEvents(sseResp)) {
              if (options.abortSignal.aborted) break

              if (evt.event === 'message_stored') {
                const obj = parseJsonObject(evt.data)
                if (typeof obj?.message_id === 'string' && obj.message_id) {
                  storedUserMessageId = obj.message_id
                }
                void invalidateOps(chatIdForRun)
                continue
              }

              if (evt.event === 'message_rewritten') {
                const obj = parseJsonObject(evt.data)
                const msgId =
                  typeof obj?.message_id === 'string' && obj.message_id
                    ? obj.message_id
                    : storedUserMessageId
                const original = typeof obj?.original === 'string' ? obj.original : ''
                const rewritten = typeof obj?.rewritten === 'string' ? obj.rewritten : ''
                if (rewritten) {
                  const entry = { original: original || messageText, rewritten }
                  setRewrittenMessages((prev) => {
                    const next = { ...prev }
                    if (msgId) next[msgId] = entry
                    if (localUserMessageId) next[localUserMessageId] = entry
                    return next
                  })
                  if (msgId) {
                    chat.setHistorySeed((prev) =>
                      prev.map((m) =>
                        m.id === msgId
                          ? { ...m, content: [{ type: 'text' as const, text: rewritten }] }
                          : m,
                      ),
                    )
                  }
                  setStatusText('Message rewritten')
                }
                continue
              }

              if (evt.event === 'status') {
                const obj = parseJsonObject(evt.data)
                const message = typeof obj?.message === 'string' ? obj.message : null
                if (message) setStatusText(message)
                continue
              }

              if (evt.event === 'delta') {
                const obj = parseJsonObject(evt.data)
                const delta = typeof obj?.delta === 'string' ? obj.delta : ''
                if (!delta) continue

                seenAnyDelta = true
                if (firstDeltaLatencyMs == null) {
                  firstDeltaLatencyMs = Date.now() - sseSubscribeStartMs
                }
                pendingDelta += delta

                // Throttle yields to avoid token-by-token re-renders.
                const nowMs = Date.now()
                if (nowMs - lastFlushMs >= 80) {
                  yield { content: [{ type: 'text', text: pendingDelta }] }
                  pendingDelta = ''
                  lastFlushMs = nowMs
                }
                continue
              }

              if (evt.event === 'done') {
                const doneObj = parseJsonObject(evt.data)
                const responseText =
                  typeof doneObj?.response === 'string' ? doneObj.response : ''
                if (pendingDelta) {
                  yield { content: [{ type: 'text', text: pendingDelta }] }
                  pendingDelta = ''
                } else if (responseText && !seenAnyDelta) {
                  // Agent runs do not emit TextDelta today; the final text arrives on `done`.
                  yield { content: [{ type: 'text', text: responseText }] }
                  seenAnyDelta = true
                }
                completedOrError = true
                if (chatIdForRun != null) {
                  try {
                    await loadHistory(chatIdForRun, personaIdForRun ?? undefined)
                    if (personaIdForRun != null && personaIdForRun > 0) {
                      await loadPersonaBulletin(personaIdForRun)
                    }
                  } catch {
                    // History sync failed; streamed `response` text (if any) remains visible.
                  }
                }
                setPendingRuns((prev) => prev.filter((r) => r.runId !== runId))
                setStatusText('Done')
                const doneLatencyMs = Date.now() - sseSubscribeStartMs
                if (import.meta.env?.DEV && typeof console !== 'undefined' && console.debug) {
                  console.debug('[web][stream]', {
                    runId,
                    chatId: chatIdForRun,
                    personaId: personaIdForRun,
                    firstDeltaLatencyMs,
                    doneLatencyMs,
                  })
                }
                break
              }

              if (evt.event === 'error') {
                completedOrError = true
                const obj = parseJsonObject(evt.data)
                const errorText = typeof obj?.error === 'string' ? obj.error : 'unknown error'
                if (obj?.kind === 'guest') {
                  showComposerAlert(errorText, '')
                  setPendingRuns((prev) => prev.filter((r) => r.runId !== runId))
                  setStatusText('Error')
                  if (chatIdForRun != null) {
                    try {
                      await loadHistory(chatIdForRun, personaIdForRun ?? undefined)
                      if (personaIdForRun != null && personaIdForRun > 0) {
                        await loadPersonaBulletin(personaIdForRun)
                      }
                    } catch {
                      // History sync failed; the composer notice still names the failure.
                    }
                  }
                  break
                }
                if (pendingDelta) {
                  yield { content: [{ type: 'text', text: pendingDelta }] }
                  pendingDelta = ''
                }
                if (!seenAnyDelta) {
                  yield { content: [{ type: 'text', text: `Error: ${errorText}` }] }
                }
                setPendingRuns((prev) => prev.filter((r) => r.runId !== runId))
                setStatusText('Error')
                if (chatIdForRun != null) {
                  try {
                    await loadHistory(chatIdForRun, personaIdForRun ?? undefined)
                    if (personaIdForRun != null && personaIdForRun > 0) {
                      await loadPersonaBulletin(personaIdForRun)
                    }
                  } catch {
                    // History sync failed; streamed error text remains visible.
                  }
                }
                const doneLatencyMs = Date.now() - sseSubscribeStartMs
                if (import.meta.env?.DEV && typeof console !== 'undefined' && console.debug) {
                  console.debug('[web][stream][error]', {
                    runId,
                    chatId: chatIdForRun,
                    personaId: personaIdForRun,
                    firstDeltaLatencyMs,
                    doneLatencyMs,
                    errorText,
                  })
                }
                break
              }
            }
          } catch (e) {
            if (!options.abortSignal.aborted) {
              const msg = e instanceof Error ? e.message : String(e)
              if (!seenAnyDelta) {
                yield { content: [{ type: 'text', text: `Error: ${msg}` }] }
              }
              setStatusText('Error')
              completedOrError = true
            }
          } finally {
            if (!completedOrError && !options.abortSignal.aborted) {
              // Stream ended without `done`/`error`; reconcile from DB once.
              setStatusText('Done')
              if (chatIdForRun != null) {
                try {
                  await loadHistory(chatIdForRun, personaIdForRun ?? undefined)
                } catch {
                  // ignore
                }
              }
            }
            setPendingRuns((prev) => prev.filter((r) => r.runId !== runId))
          }
        } finally {
          void invalidateOps(chatId)
        }
      },
    }),
    [chatId, selectedSessionReadOnly, activePersonaId, formatReplyForSend, historyMode, jumpToLatest, loadHistory, loadPersonaBulletin, invalidateOps, setDraftByThreadKey, showComposerAlert],
  )

  const toggleAppearance = useCallback((): void => {
    setAppearance((prev) => (prev === 'dark' ? 'light' : 'dark'))
  }, [])

  useEffect(() => {
    saveAppearance(appearance)
    document.documentElement.classList.toggle('dark', appearance === 'dark')
  }, [appearance])

  useEffect(() => {
    if (statusText === 'Done') toast.success('Run complete')
    else if (statusText === 'Error') toast.error('Run failed')
    else if (statusText === 'Memory saved') toast.success('Memory saved')
    else if (statusText === 'Queued') toast.message('Queued')
    else if (statusText === 'Read-only channel') toast.warning('Read-only channel')
  }, [statusText])

  useEffect(() => {
    saveUiTheme(uiTheme)
    document.documentElement.setAttribute('data-ui-theme', uiTheme)
  }, [uiTheme])

  useEffect(() => {
    ; (async () => {
      try {
        setError('')
        setHistoryLoading(true)
        void loadSettings()
        const data = await api<{ chat_id?: number; persona_id?: number }>('/api/chat')
        const cid = typeof data.chat_id === 'number' ? data.chat_id : null
        const pid = typeof data.persona_id === 'number' ? data.persona_id : null
        setChatId(cid)
        if (pid != null) setActivePersonaId(pid)
        if (cid != null) {
          const chosen = await loadPersonas(cid)
          loadBindings(cid).catch(() => { })
          loadSchedules(cid).catch(() => { })
          loadInboxTodos(cid).catch(() => { })
          void invalidateOps(cid)
          const personaForSession = chosen?.id ?? pid ?? null
          const sessions = await loadSessions(cid, personaForSession)
          const restoredSessionId =
            personaForSession != null ? resolveStoredSessionId(sessions, personaForSession) : null
          if (restoredSessionId) setActiveSessionId(restoredSessionId)
          resetHistoryPagination()
          await loadHistory(cid, chosen?.id ?? pid, null, {
            force: true,
            limitOverride: HISTORY_PAGE_SIZE,
            sessionId: restoredSessionId,
          })
          if (chosen?.id != null) {
            await loadPersonaBulletin(chosen.id)
          } else if (pid != null) {
            await loadPersonaBulletin(pid)
          }
          const readId = chosen?.id ?? pid ?? null
          if (readId != null) markPersonaRead(readId, cid)
        }
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
      } finally {
        setHistoryLoading(false)
      }
    })()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  async function loadSchedules(cid: number | null = chatId): Promise<void> {
    if (cid == null) return
    try {
      const query = new URLSearchParams({ chat_id: String(cid) })
      const data = await api<{ tasks?: ScheduleTask[] }>(`/api/schedules?${query.toString()}`)
      setSchedules(Array.isArray(data.tasks) ? data.tasks : [])
    } catch {
      setSchedules([])
    }
  }

  async function loadInboxTodos(cid: number | null = chatId): Promise<void> {
    if (cid == null) {
      setInboxTodos([])
      return
    }
    setInboxLoading(true)
    try {
      const query = new URLSearchParams({ chat_id: String(cid), status: 'open' })
      const data = await api<{ todos?: PersonaTodo[] }>(`/api/todos?${query.toString()}`)
      setInboxTodos(Array.isArray(data.todos) ? data.todos : [])
    } catch {
      setInboxTodos([])
    } finally {
      setInboxLoading(false)
    }
  }

  async function completeInboxTodo(todoId: number): Promise<void> {
    setInboxBusyTodoId(todoId)
    try {
      await api(`/api/todos/${todoId}`, {
        method: 'PATCH',
        body: JSON.stringify({ completed: true }),
      })
      setInboxTodos((prev) => prev.filter((t) => t.id !== todoId))
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setInboxBusyTodoId(null)
    }
  }

  useEffect(() => {
    if (!schedulesDialogOpen) return
    void loadSchedules(chatId)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [schedulesDialogOpen])

  useEffect(() => {
    if (!inboxDialogOpen) return
    void loadInboxTodos(chatId)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [inboxDialogOpen])

  useEffect(() => {
    if (chatId == null) {
      setInboxTodos([])
      return
    }
    void loadInboxTodos(chatId)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [chatId])

  useEffect(() => {
    if (!memoryDialogOpen) return
    if (activePersonaId == null) return
    void loadPersonaMemory(activePersonaId)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [memoryDialogOpen, activePersonaId])

  useEffect(() => {
    if (!agentsMdOpen) return
    void loadWorkspaceAgentsMd()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [agentsMdOpen])

  useEffect(() => {
    if (!queueDialogOpen || chatId == null) return
    void invalidateOps(chatId)
    const id = setInterval(() => {
      void invalidateOps(chatId)
    }, 2500)
    return () => clearInterval(id)
  }, [queueDialogOpen, chatId, invalidateOps])

  useEffect(() => {
    if (!agentHistoryDialogOpen) return
    if (activePersonaId == null) return
    void loadAgentHistoryLatest(activePersonaId)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [agentHistoryDialogOpen, activePersonaId])

  useEffect(() => {
    if (!settingsDialogOpen) return
    void loadSettings()
    if (chatId != null) {
      void loadBindings(chatId)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [settingsDialogOpen, chatId])

  useEffect(() => {
    if (!agentHistoryDialogOpen) return
    const n = agentHistoryParsed?.iterations.length ?? 0
    if (n === 0 || agentHistoryTab !== 'trace') return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'ArrowLeft') {
        e.preventDefault()
        setAgentHistoryIterationIdx((i) => Math.max(0, i - 1))
      } else if (e.key === 'ArrowRight') {
        e.preventDefault()
        setAgentHistoryIterationIdx((i) => Math.min(n - 1, i + 1))
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [agentHistoryDialogOpen, agentHistoryParsed?.iterations.length, agentHistoryTab])

  async function loadBindings(cid: number | null = chatId): Promise<void> {
    if (cid == null) return
    try {
      const query = new URLSearchParams({ chat_id: String(cid) })
      const data = await api<{ bindings?: ChannelBinding[] }>(`/api/contacts/bindings?${query.toString()}`)
      setBindings(Array.isArray(data.bindings) ? data.bindings : [])
    } catch {
      setBindings([])
    }
  }

  async function loadSettings(): Promise<void> {
    setSettingsError('')
    try {
      const data = await api<{
        installation_status?: InstallationStatus
      }>('/api/settings')
      setInstallationStatus(data.installation_status ?? null)
    } catch (e) {
      setSettingsError(e instanceof Error ? e.message : String(e))
      setInstallationStatus(null)
    }
  }


  async function requestRestart(): Promise<void> {
    setSettingsError('')
    setRestartNotice(null)
    setRestartBusy(true)
    try {
      const data = await api<{ ok?: boolean; message?: string }>('/api/restart', { method: 'POST' })
      setRestartNotice(data.message ?? 'Restart initiated.')
    } catch (e) {
      setSettingsError(e instanceof Error ? e.message : String(e))
    } finally {
      setRestartBusy(false)
    }
  }



  async function updateChannelPersonaPolicy(
    botInstanceId: number,
    mode: 'all' | 'single',
    personaId?: number,
  ): Promise<void> {
    if (chatId == null) return
    setSettingsError('')
    try {
      if (mode === 'all') {
        await api('/api/channel_persona_policy', {
          method: 'DELETE',
          body: JSON.stringify({ chat_id: chatId, bot_instance_id: botInstanceId }),
        })
      } else {
        await api('/api/channel_persona_policy', {
          method: 'POST',
          body: JSON.stringify({
            chat_id: chatId,
            bot_instance_id: botInstanceId,
            mode: 'single',
            persona_id: personaId,
          }),
        })
      }
      await loadBindings(chatId)
    } catch (e) {
      setSettingsError(e instanceof Error ? e.message : String(e))
    }
  }

  async function loadQueueDiagnostics(cid: number | null = chatId): Promise<void> {
    await invalidateOps(cid)
  }

  async function loadBackgroundVisibility(cid: number | null = chatId): Promise<void> {
    await invalidateOps(cid)
  }

  async function bindToContact(contactChatId: number): Promise<void> {
    await api('/api/contacts/bind', {
      method: 'POST',
      body: JSON.stringify({ contact_chat_id: contactChatId }),
    })
    await loadBindings(chatId)
    resetHistoryPagination()
    if (activePersonaId != null && activePersonaId > 0) {
      await loadHistory(chatId, activePersonaId, null, {
        force: true,
        limitOverride: HISTORY_PAGE_SIZE,
      })
    }
  }

  async function unlinkContact(): Promise<void> {
    await api('/api/contacts/unlink', {
      method: 'POST',
      body: JSON.stringify({}),
    })
    await loadBindings(chatId)
  }

  async function createSchedule(
    prompt: string,
    scheduleType: string,
    scheduleValue: string,
    personaId?: number | null,
    outputDelivery: 'runs_only' | 'transcript' = 'runs_only',
  ): Promise<void> {
    await api('/api/schedules', {
      method: 'POST',
      body: JSON.stringify({
        chat_id: chatId ?? undefined,
        prompt,
        schedule_type: scheduleType,
        schedule_value: scheduleValue,
        persona_id: personaId && personaId > 0 ? personaId : undefined,
        output_delivery: outputDelivery,
      }),
    })
    await loadSchedules(chatId)
  }

  async function updateSchedule(
    taskId: number,
    patch: {
      status?: string
      persona_id?: number
      prompt?: string
      schedule_type?: string
      schedule_value?: string
      timezone?: string
      output_delivery?: 'runs_only' | 'transcript'
    },
  ): Promise<void> {
    await api(`/api/schedules/${taskId}`, {
      method: 'PATCH',
      body: JSON.stringify(patch),
    })
    setScheduleDetailTask((current) =>
      current?.id === taskId ? { ...current, ...patch } : current,
    )
    await loadSchedules(chatId)
  }

  async function cancelQueueRun(runId: string): Promise<void> {
    await api('/api/queue/cancel', {
      method: 'POST',
      body: JSON.stringify({ run_id: runId, chat_id: chatId ?? undefined }),
    })
    await loadQueueDiagnostics(chatId)
  }

  async function removeQueueRun(runId: string): Promise<void> {
    await api('/api/queue/remove', {
      method: 'POST',
      body: JSON.stringify({ run_id: runId, chat_id: chatId ?? undefined }),
    })
    await loadQueueDiagnostics(chatId)
  }

  async function handleQueueAction(runId: string, state: string): Promise<void> {
    const id = runId.trim()
    if (!id) return
    if (stoppingRunIds.includes(id)) return
    setStoppingRunIds((prev) => (prev.includes(id) ? prev : [...prev, id]))
    try {
      if (state === 'running') {
        await cancelQueueRun(id)
      } else {
        await removeQueueRun(id)
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setStoppingRunIds((prev) => prev.filter((x) => x !== id))
      await loadQueueDiagnostics(chatId)
    }
  }

  async function cancelBackgroundJob(jobId: string): Promise<void> {
    await api('/api/background_jobs/cancel', {
      method: 'POST',
      body: JSON.stringify({ job_id: jobId, chat_id: chatId ?? undefined }),
    })
    await loadBackgroundVisibility(chatId)
  }

  async function handleBackgroundJobStop(jobId: string): Promise<void> {
    const id = jobId.trim()
    if (!id) return
    if (stoppingBackgroundJobIds.includes(id)) return
    setStoppingBackgroundJobIds((prev) => (prev.includes(id) ? prev : [...prev, id]))
    try {
      await cancelBackgroundJob(id)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setStoppingBackgroundJobIds((prev) => prev.filter((x) => x !== id))
      await loadBackgroundVisibility(chatId)
    }
  }

  async function loadWorkspaceAgentsMd(): Promise<void> {
    setAgentsMdError('')
    setAgentsMdBusy(true)
    try {
      const data = await api<{ content?: string; mtime_ms?: number; path?: string }>('/api/workspace/agents_md')
      setAgentsMdContent(typeof data.content === 'string' ? data.content : '')
      setAgentsMdMtimeMs(typeof data.mtime_ms === 'number' ? data.mtime_ms : null)
      setAgentsMdPath(typeof data.path === 'string' ? data.path : '')
    } catch (e) {
      setAgentsMdError(e instanceof Error ? e.message : String(e))
    } finally {
      setAgentsMdBusy(false)
    }
  }

  async function saveWorkspaceAgentsMd(): Promise<void> {
    setAgentsMdError('')
    setAgentsMdBusy(true)
    try {
      const data = await api<{ mtime_ms?: number }>('/api/workspace/agents_md', {
        method: 'PUT',
        body: JSON.stringify({
          content: agentsMdContent,
          if_match_mtime_ms: agentsMdMtimeMs ?? undefined,
        }),
      })
      if (typeof data.mtime_ms === 'number') {
        setAgentsMdMtimeMs(data.mtime_ms)
      }
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e)
      if (msg.includes('409') || msg.toLowerCase().includes('conflict')) {
        setAgentsMdError('File changed on disk. Reload and retry.')
      } else {
        setAgentsMdError(msg)
      }
    } finally {
      setAgentsMdBusy(false)
    }
  }

  useEffect(() => {
    if (chatId == null) return
    let cancelled = false
    async function init() {
      const chosen = await loadPersonas(chatId)
      if (cancelled) return
      loadBindings(chatId).catch(() => { })
      loadSchedules(chatId).catch(() => { })
      loadInboxTodos(chatId).catch(() => { })
      void invalidateOps(chatId)
      if (chosen) {
        try {
          await api('/api/personas/switch', {
            method: 'POST',
            body: JSON.stringify({ chat_id: chatId ?? undefined, persona_name: chosen.name }),
          })
        } catch {
          // ignore; we still load history for the chosen persona
        }
      }
      resetHistoryPagination()
      loadHistory(chatId, chosen?.id, null, { force: true, limitOverride: HISTORY_PAGE_SIZE }).catch((e) =>
        setError(e instanceof Error ? e.message : String(e)),
      )
      if (chosen?.id != null) {
        loadPersonaBulletin(chosen.id).catch(() => { })
      }
    }
    init()
    return () => { cancelled = true }
  }, [chatId, invalidateOps, resetHistoryPagination])

  useEffect(() => {
    if (activePersonaId != null && activePersonaId > 0) {
      setNewSchedulePersonaId((prev) => (prev == null ? activePersonaId : prev))
    }
  }, [activePersonaId, setNewSchedulePersonaId])

  useEffect(() => {
    setPendingRuns([])
  }, [chatId])

  useEffect(() => {
    prevBgJobStatusByIdRef.current = new Map()
  }, [chatId])

  useEffect(() => {
    if (chatId == null) return
    const prev = prevBgJobStatusByIdRef.current
    let shouldReloadThread = false
    let reloadBulletin = false
    for (const job of backgroundJobs) {
      const last = prev.get(job.id)
      if (
        last !== undefined
        && isActiveBackgroundJobStatus(last)
        && isTerminalBackgroundJobStatus(job.status)
      ) {
        if (activePersonaId == null || job.persona_id === activePersonaId) {
          shouldReloadThread = true
        }
        if (activePersonaId != null && job.persona_id === activePersonaId) {
          reloadBulletin = true
        }
      }
    }
    const next = new Map<string, string>()
    for (const job of backgroundJobs) {
      next.set(job.id, job.status)
    }
    prevBgJobStatusByIdRef.current = next
    if (!shouldReloadThread) return
    setStatusText('Done')
    void loadHistory(chatId, activePersonaId ?? undefined)
    if (reloadBulletin && activePersonaId != null) {
      void loadPersonaBulletin(activePersonaId)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [chatId, activePersonaId, backgroundJobs])

  // Lightweight refresh for cross-channel / background replies (no SSE for those paths).
  useEffect(() => {
    if (chatId == null) return
    if (!docVisible) return
    if (pendingRunsForActivePersona.length > 0) return
    const shouldPollFast = backgroundActiveCount > 0 || (queueLane?.pending ?? 0) > 0
    const intervalMs = shouldPollFast ? 5000 : 30000
    const interval = setInterval(() => {
      void loadHistory(chatId, activePersonaId ?? undefined).catch(() => { })
    }, intervalMs)
    return () => clearInterval(interval)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    chatId,
    activePersonaId,
    docVisible,
    pendingRunsForActivePersona.length,
    backgroundActiveCount,
    queueLane?.pending,
  ])

  const runtimeKey = `${chatId ?? 0}-${activePersonaId ?? 0}`

  const handleShowShortcuts = useCallback(() => {
    setShortcutsOpen(true)
  }, [])

  const handleOpenSettings = useCallback(() => {
    setPersonaDialogOpen(false)
    setSettingsPage('overview')
    setSettingsDialogOpen(true)
  }, [])

  const openSettingsPage = useCallback((page: SettingsLedgerPage) => {
    setPersonaDialogOpen(false)
    setSettingsPage(page)
    setSettingsReveal((n) => n + 1)
    setSettingsDialogOpen(true)
  }, [])

  const handleOpenPersona = useCallback(() => {
    setSettingsDialogOpen(false)
    setPersonaError('')
    setPersonaPage('profile')
    setPersonaDialogOpen(true)
  }, [])

  const openPersonaPage = useCallback((page?: PersonaLedgerPage) => {
    setSettingsDialogOpen(false)
    setPersonaError('')
    setPersonaPage(page ?? 'profile')
    if (page) setPersonaReveal((n) => n + 1)
    setPersonaDialogOpen(true)
  }, [])

  const handleOpenInbox = useCallback(() => {
    setInboxDialogOpen(true)
  }, [])

  const handleOpenSchedules = useCallback(() => {
    setSchedulesDialogOpen(true)
  }, [])

  const handleOpenPrinciples = useCallback(() => {
    setAgentsMdOpen(true)
  }, [])

  const handleOpenArtifacts = useCallback(() => {
    setArtifactsDialogOpen(true)
  }, [])

  const handleOpenTerminal = useCallback(() => {
    setTerminalError('')
    setTerminalDialogOpen(true)
  }, [])

  const handleOpenMemory = useCallback(() => {
    setMemoryDialogOpen(true)
  }, [])

  const handleOpenAgentHistory = useCallback(() => {
    setAgentHistoryDialogOpen(true)
  }, [])

  const handleToggleCockpit = useCallback(() => {
    setCockpitExpanded((v) => !v)
  }, [])

  const handleOpenMobileNav = useCallback(() => {
    setMobileNavOpen(true)
  }, [])

  const handleToggleDesktopSidebar = useCallback(() => {
    setDesktopSidebarOpen((v) => !v)
  }, [])

  const handleOpenMobileOps = useCallback(() => {
    setMobileOpsOpen(true)
  }, [])

  const handleUiThemeChange = useCallback((theme: string) => {
    setUiTheme(theme as UiTheme)
  }, [])

  const handleCreatePersonaClick = useCallback(() => {
    setNewPersonaOpen(true)
  }, [])

  const handleRenamePersona = useCallback(
    (id: number, name: string) => onRenamePersona(id, name),
    [onRenamePersona],
  )

  const handleDeletePersonaClick = useCallback((id: number) => {
    void onDeletePersona(id)
  }, [onDeletePersona])

  const handlePersonaSelect = useCallback((name: string) => {
    void switchPersona(name)
  }, [switchPersona])

  const handleQueueClick = useCallback(() => {
    setQueueDialogOpen(true)
  }, [])

  const handleBulletinStatus = useCallback((msg: string) => {
    setStatusText(msg)
  }, [])

  const handleDismissError = useCallback(() => {
    setError('')
  }, [])

  const handleDismissOnboarding = useCallback(() => {
    try {
      sessionStorage.setItem('meido_onboarding_banner_dismissed', '1')
    } catch {
      /* ignore */
    }
    setOnboardingDismissed(true)
  }, [])

  const bookmarkedMessageIds = useMemo(
    () => new Set(personaBookmarks.map((b) => b.message_id)),
    [personaBookmarks],
  )

  useKeyboardShortcuts({
    activePendingReply,
    onDismissPendingReply: handleDismissPendingReply,
    settingsDialogOpen: settingsDialogOpen || personaDialogOpen,
    mobileOpsOpen,
    shortcutsOpen,
    onOpenShortcuts: handleShowShortcuts,
  })

  const handleCloseMobileNav = useCallback(() => {
    setMobileNavOpen(false)
  }, [])

  const handleSelectSessionStable = useCallback((sid: string | null) => {
    void handleSelectSession(sid)
  }, [handleSelectSession])

  const appHeaderNav = useMemo(
    () => ({
      mobileNavOpen,
      onOpenMobileNav: handleOpenMobileNav,
      desktopSidebarOpen,
      onToggleDesktopSidebar: handleToggleDesktopSidebar,
    }),
    [mobileNavOpen, handleOpenMobileNav, desktopSidebarOpen, handleToggleDesktopSidebar],
  )

  const appHeaderSession = useMemo(
    () => ({
      activePersonaId,
      activePersonaName,
      chatSessions,
      activeSessionId,
      historyLoading,
      onSelectSession: handleSelectSessionStable,
      onCreateSession: handleCreateSession,
      onDeleteSession: handleDeleteSession,
      sideChats,
      activeSideChatId: activeSideChat?.id ?? null,
      sideChatsLoading,
      onSelectSideChat: (id: string) => {
        void handleSelectSideChat(id)
      },
      onDeleteSideChat: deleteSideChat,
    }),
    [
      activePersonaId,
      activePersonaName,
      chatSessions,
      activeSessionId,
      historyLoading,
      handleSelectSessionStable,
      handleCreateSession,
      handleDeleteSession,
      sideChats,
      activeSideChat?.id,
      sideChatsLoading,
      handleSelectSideChat,
      deleteSideChat,
    ],
  )

  const terminalAvailable = installationStatus?.terminal?.web_terminal_available === true
  const agentHistoryDisabled = activePersonaId == null

  const appHeaderToolbar = useMemo(
    () => ({
      queueLane,
      backgroundActiveCount,
      installationStatus,
      statusText,
      cockpitExpanded,
      onToggleCockpit: handleToggleCockpit,
      onOpenPersona: handleOpenPersona,
      onOpenSettings: handleOpenSettings,
      onOpenInbox: handleOpenInbox,
      inboxBadgeCount,
      schedulesBadgeCount: unreadRuns,
      onOpenSchedules: handleOpenSchedules,
      onOpenPrinciples: handleOpenPrinciples,
      onOpenArtifacts: handleOpenArtifacts,
      onOpenTerminal: handleOpenTerminal,
      terminalAvailable,
      onOpenMemory: handleOpenMemory,
      onOpenAgentHistory: handleOpenAgentHistory,
      agentHistoryDisabled,
      onOpenTopics: handleOpenTopics,
    }),
    [
      queueLane,
      backgroundActiveCount,
      installationStatus,
      statusText,
      cockpitExpanded,
      handleToggleCockpit,
      handleOpenPersona,
      handleOpenSettings,
      handleOpenInbox,
      inboxBadgeCount,
      unreadRuns,
      handleOpenSchedules,
      handleOpenPrinciples,
      handleOpenArtifacts,
      handleOpenTerminal,
      terminalAvailable,
      handleOpenMemory,
      handleOpenAgentHistory,
      agentHistoryDisabled,
      handleOpenTopics,
    ],
  )

  return (
    <TooltipProvider delayDuration={250}>
      <Toaster appearance={appearance} position="bottom-right" richColors closeButton />
      <AuthDialog />
      <CommandPalette
        open={commandPaletteOpen}
        onOpenChange={setCommandPaletteOpen}
        personas={personas}
        chatSessions={chatSessions}
        sideChats={sideChats}
        actions={{
          onOpenPersona: handleOpenPersona,
          onOpenSettings: handleOpenSettings,
          onOpenInbox: handleOpenInbox,
          onOpenOps: handleOpenMobileOps,
          onOpenTerminal: terminalAvailable ? handleOpenTerminal : undefined,
          terminalAvailable,
          onToggleAppearance: toggleAppearance,
          appearance,
          onToggleSidebar: handleToggleDesktopSidebar,
          onToggleCockpit: handleToggleCockpit,
          onShowShortcuts: handleShowShortcuts,
          onCreateSession: () => {
            void handleCreateSession('New session', true)
          },
          onSelectPersona: handlePersonaSelect,
          onSelectSession: (id) => {
            void handleSelectSession(id)
          },
          onSelectSideChat: (id) => {
            void handleSelectSideChat(id)
          },
        }}
      />

      <div
        className={
          appearance === 'dark'
            ? 'mc-app-shell h-[100dvh] min-w-0 w-full overflow-hidden bg-[var(--mc-bg-main)] pb-[env(safe-area-inset-bottom,0px)] pt-[env(safe-area-inset-top,0px)]'
            : 'mc-app-shell h-[100dvh] min-w-0 w-full overflow-hidden bg-[var(--mc-bg-main)] pb-[env(safe-area-inset-bottom,0px)] pt-[env(safe-area-inset-top,0px)]'
        }
      >
        {mobileNavOpen ? (
          <div
            className="fixed inset-0 z-[100] flex md:hidden"
            role="dialog"
            aria-modal="true"
            aria-label="Persona and theme"
          >
            <button
              type="button"
              className="absolute inset-0 bg-black/50"
              aria-label="Close menu"
              onClick={() => setMobileNavOpen(false)}
            />
            <div
              id="mobile-session-sidebar-panel"
              className="relative z-[101] flex h-full min-h-0 w-[min(320px,100vw)] max-w-[90vw] flex-col border-r border-[color:var(--mc-border-soft)] bg-[color:var(--mc-bg-sidebar)] pt-[env(safe-area-inset-top,0px)] pl-[env(safe-area-inset-left,0px)]"
            >
              <SessionSidebar
                appearance={appearance}
                onToggleAppearance={toggleAppearance}
                uiTheme={uiTheme}
                onUiThemeChange={handleUiThemeChange}
                uiThemeOptions={UI_THEME_OPTIONS}
                personas={personas}
                personaHasNew={personaHasNew}
                selectedPersonaId={activePersonaId}
                onPersonaSelect={handlePersonaSelect}
                onCreatePersona={handleCreatePersonaClick}
                onCloseRequest={handleCloseMobileNav}
                mode={sidebarMode}
                onModeChange={setMode}
                blockedItems={blockedItems}
                onOpenPersonaConfig={handleOpenPersona}
                onOpenSettings={handleOpenSettings}
              />
            </div>
          </div>
        ) : null}

        <div
          className={desktopSidebarOpen ? 'mc-layout-grid mc-layout-grid--sidebar-open' : 'mc-layout-grid'}
          style={desktopSidebarOpen ? ({ '--mc-sidebar-width': `${desktopSidebarWidth}px` } as React.CSSProperties) : undefined}
        >
          {desktopSidebarOpen ? (
            <div id="desktop-session-sidebar" className="relative hidden min-h-0 md:flex md:flex-col">
              <SessionSidebar
                appearance={appearance}
                onToggleAppearance={toggleAppearance}
                uiTheme={uiTheme}
                onUiThemeChange={handleUiThemeChange}
                uiThemeOptions={UI_THEME_OPTIONS}
                personas={personas}
                personaHasNew={personaHasNew}
                selectedPersonaId={activePersonaId}
                onPersonaSelect={handlePersonaSelect}
                onCreatePersona={handleCreatePersonaClick}
                mode={sidebarMode}
                onModeChange={setMode}
                blockedItems={blockedItems}
              />
              <div
                role="separator"
                tabIndex={0}
                aria-label="Resize personas sidebar"
                aria-orientation="vertical"
                aria-valuemin={DESKTOP_SIDEBAR_MIN_WIDTH}
                aria-valuemax={DESKTOP_SIDEBAR_MAX_WIDTH}
                aria-valuenow={desktopSidebarWidth}
                className={desktopSidebarResizing ? 'mc-sidebar-resize-handle mc-sidebar-resize-handle--active' : 'mc-sidebar-resize-handle'}
                onPointerDown={beginDesktopSidebarResize}
                onKeyDown={(event) => {
                  if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return
                  event.preventDefault()
                  const delta = event.key === 'ArrowRight' ? 16 : -16
                  setDesktopSidebarWidth((current) => clampDesktopSidebarWidth(current + delta, window.innerWidth))
                }}
              />
            </div>
          ) : null}

          <main
            className={
              appearance === 'dark'
                ? 'flex h-full min-h-0 min-w-0 flex-col overflow-hidden bg-[var(--mc-bg-panel)]'
                : 'flex h-full min-h-0 min-w-0 flex-col overflow-hidden bg-[color:var(--mc-surface-elevated)]/95'
            }
          >
            <AppHeader
              appearance={appearance}
              mobileContextOpen={topicsSheetOpen}
              onOpenMobileContext={handleOpenTopics}
              nav={appHeaderNav}
              session={appHeaderSession}
              toolbar={appHeaderToolbar}
              onOpenMobileOps={handleOpenMobileOps}
              moduleTitle={sidebarMode === 'personas' ? null : SIDEBAR_MODE_TITLES[sidebarMode]}
            />
            <div
              className={
                appearance === 'dark'
                  ? 'mc-work-column relative flex min-h-0 min-w-0 flex-1 flex-col bg-[linear-gradient(to_bottom,var(--mc-bg-panel),var(--mc-bg-main)_28%)]'
                  : 'mc-work-column relative flex min-h-0 min-w-0 flex-1 flex-col bg-[var(--mc-bg-main)]'
              }
            >
              <div
                className={
                  sidebarMode === 'personas'
                    ? 'relative z-20 mx-auto w-full max-w-5xl shrink-0 px-2 md:px-3'
                    : 'hidden'
                }
              >
                <div className="hidden md:block">
                  <BulletinStrip bulletinFocus={bulletinFocus} hideCockpitLauncher />
                </div>
                <CockpitBar
                  appearance={appearance}
                  statusText={statusText}
                  queueLane={queueLane}
                  otherPersonasPending={otherPersonasPending}
                  backgroundActiveCount={backgroundActiveCount}
                  installationStatus={installationStatus}
                  onQueueClick={handleQueueClick}
                  bulletinFocus={bulletinFocus}
                  bookmarks={personaBookmarks}
                  activePersonaId={activePersonaId}
                  onJumpToBookmark={revealMessageInThread}
                  onRemoveBookmark={removePersonaBookmark}
                  operatorMemoServer={bulletinOperatorMemo}
                  reloadBulletin={reloadPersonaBulletin}
                  onBulletinStatus={handleBulletinStatus}
                  onExpandedChange={setCockpitExpanded}
                  expanded={cockpitExpanded}
                />
              </div>
              <div className="mx-auto w-full max-w-5xl px-2 pt-2 md:px-3">
                <StatusRegion message={statusText} />
                {installationStatus != null &&
                !onboardingDismissed &&
                (!installationStatus.llm_ready || !installationStatus.channel_ready) ? (
                  <div role="status" className="mc-notice-owed mb-2">
                    <div className="flex flex-col gap-2">
                      <span>
                        LLM or a channel is missing. Open Settings to finish setup.
                      </span>
                      <div className="flex gap-2 items-center flex-wrap">
                        <Button size="sm" variant="default" onClick={handleOpenSettings}>
                          Open Settings
                        </Button>
                        <Button
                          size="sm"
                          variant="secondary"
                          onClick={handleDismissOnboarding}
                        >
                          Dismiss
                        </Button>
                      </div>
                    </div>
                  </div>
                ) : null}
                {replayNotice ? (
                  <div role="status" className="mc-notice-owed">{replayNotice}</div>
                ) : null}
                {error ? (
                  <ErrorBanner
                    message={error}
                    className={replayNotice ? 'mt-2' : ''}
                    onDismiss={handleDismissError}
                  />
                ) : null}
              </div>

              <div className={sidebarMode === 'personas' ? 'flex min-h-0 min-w-0 flex-1 flex-col md:flex-row' : 'hidden'}>
                {activePersonaId != null ? (
                  <TopicOutlineRail
                    className="mc-topic-rail-slot hidden md:flex"
                    topics={topics.topics}
                    segmenting={topics.segmenting}
                    unsegmentedCount={topics.unsegmentedCount}
                    loading={topics.loading}
                    error={topics.error}
                    focusTopicId={threadSurface === 'focus' ? (dossierTopic?.id ?? focusTopicId) : focusTopicId}
                    onClearFocus={handleClearTopicFocus}
                    onJump={handleTopicJump}
                    onSelectSideChat={(id) => {
                      void handleSelectSideChat(id)
                    }}
                    onRename={(topicId, title) => {
                      void topics.rename(topicId, title).catch((err: unknown) => {
                        toast.error(err instanceof Error ? err.message : 'Could not rename topic')
                      })
                    }}
                    onTogglePin={(topicId, pinned) => {
                      void topics.pin(topicId, pinned).catch((err: unknown) => {
                        toast.error(err instanceof Error ? err.message : 'Could not pin topic')
                      })
                    }}
                    onMerge={(topicId) => {
                      void topics.merge(topicId).catch((err: unknown) => {
                        toast.error(err instanceof Error ? err.message : 'Could not merge topics')
                      })
                    }}
                    onDelete={handleDeleteTopic}
                    onCreateSession={handleCreateTopicSession}
                    onRefresh={() => {
                      void topics.refresh().catch((err: unknown) => {
                        toast.error(err instanceof Error ? err.message : 'Could not refresh topics')
                      })
                    }}
                    enableSpy={threadSurface === 'transcript'}
                  />
                ) : null}
                <div className="flex min-h-0 min-w-0 flex-1 flex-col px-0 pb-1 md:px-1">
                <div className="hidden items-center gap-1 px-2 pt-1 md:flex" aria-label="Conversation surface">
                  <Button
                    type="button"
                    size="sm"
                    variant={threadSurface === 'focus' ? 'secondary' : 'ghost'}
                    className="cursor-pointer"
                    aria-pressed={threadSurface === 'focus'}
                    onClick={() => chooseThreadSurface('focus')}
                  >
                    Focus
                  </Button>
                  <Button
                    type="button"
                    size="sm"
                    variant={threadSurface === 'transcript' ? 'secondary' : 'ghost'}
                    className="cursor-pointer"
                    aria-pressed={threadSurface === 'transcript'}
                    onClick={() => chooseThreadSurface('transcript')}
                  >
                    Transcript
                  </Button>
                  <Button
                    type="button"
                    size="sm"
                    variant={hideScheduled ? 'secondary' : 'ghost'}
                    className="cursor-pointer"
                    aria-pressed={hideScheduled}
                    onClick={() => setHideScheduled((current) => !current)}
                  >
                    {hideScheduled ? 'Scheduled hidden' : 'Hide scheduled'}
                  </Button>
                </div>
                <ResizablePanelGroup
                  id="meido-thread-subthread"
                  orientation="horizontal"
                  className="mc-thread-with-subthread min-h-0 min-w-0 flex-1"
                >
                  <ResizablePanel
                    id="thread-main"
                    defaultSize="62"
                    minSize="35"
                    className="mc-thread-main min-h-0 min-w-0"
                  >
                    <ThreadPane
                      key={runtimeKey}
                      adapter={adapter}
                      initialMessages={historySeed}
                      runtimeKey={runtimeKey}
                      isStreaming={pendingRunsForActivePersona.length > 0}
                      historyLoading={historyLoading}
                      historyMode={historyMode}
                      historyHasOlder={historyHasOlder}
                      historyHasNewer={historyHasNewer}
                      historyLoadingMore={historyLoadingMore}
                      historyLoadingNewer={historyLoadingNewer}
                      newMessagesPending={newMessagesPending}
                      onLoadOlder={loadOlder}
                      onLoadNewer={loadNewer}
                      onJumpToLatest={jumpToLatest}
                      draftText={activeDraftText}
                      onDraftTextChange={onComposerDraftChange}
                      composerNotice={composerNotice}
                      bookmarkedMessageIds={bookmarkedMessageIds}
                      onToggleBookmark={toggleMessageBookmark}
                      onReplyToMessage={handleReplyToMessage}
                      onDeleteMessage={handleDeleteMessage}
                      onSaveMessageEdit={handleSaveMessageEdit}
                      onOpenSubthread={handleOpenSubthread}
                      editingMessageId={editingMessageId}
                      onEditingMessageIdChange={setEditingMessageId}
                      activeSubthreadMessageId={activeSideChat?.anchorMessageId ?? null}
                      pendingReply={activePendingReply}
                      onDismissPendingReply={handleDismissPendingReply}
                      targetScrollMessageId={targetScrollMessageId}
                      suppressAutoScroll={jumpSuppressAutoScroll}
                      onTargetScrollHandled={handleTargetScrollHandled}
                      onShowShortcuts={handleShowShortcuts}
                      topicStarts={topicStarts}
                      onRevealTopic={handleRevealTopic}
                      onSplitTopic={handleSplitTopic}
                      mentionPersonas={personas}
                      hostPersonaId={activePersonaId}
                      rewrittenMessages={rewrittenMessages}
                      focusSlot={
                        threadSurface === 'focus' ? (
                          <TopicFocusView
                            topicTitle={dossierTopic?.title ?? ''}
                            assistantLabel={activePersonaName || 'Assistant'}
                            loading={dossierQuery.isLoading}
                            error={
                              dossierQuery.error instanceof Error
                                ? dossierQuery.error.message
                                : dossierQuery.error
                                  ? 'Could not load this topic'
                                  : ''
                            }
                            segmenting={topics.segmenting || Boolean(dossierQuery.data?.segmenting)}
                            dossier={dossierQuery.data ?? null}
                            pageKey={`${dossierTopic?.id ?? ''}:${dossierCursorKey(dossierCursor)}`}
                            highlightMessageId={dossierHighlightId}
                            paging={dossierQuery.isFetching}
                            onSelectSideChat={(id) => {
                              void handleSelectSideChat(id)
                            }}
                            onRetry={() => {
                              setDossierCursor({ kind: 'latest' })
                              void dossierQuery.refetch()
                            }}
                            onEarlier={handleDossierEarlier}
                            onLater={handleDossierLater}
                            onOpenAround={handleDossierAround}
                            onHighlight={setDossierHighlightId}
                          />
                        ) : null
                      }
                      composerSlot={
                        threadSurface === 'focus' ? (
                          <TopicSessionAction
                            topicTitle={dossierTopic?.title ?? ''}
                            disabled={!dossierTopic}
                            onCreate={handleCreateTopicSession}
                          />
                        ) : null
                      }
                      uploadHint={
                        statusText.startsWith('Uploading') || statusText.startsWith('Sending message')
                          ? statusText
                          : undefined
                      }
                    />
                  </ResizablePanel>
                  {sideChatHost(threadSurface) === 'split' && activeSideChat ? (
                    <>
                      <ResizableHandle withHandle className="hidden md:flex" />
                      <ResizablePanel
                        id="thread-side"
                        defaultSize="38"
                        minSize="22"
                        className="h-full min-h-0 min-w-0"
                      >
                        <SubthreadSidePane
                          key={activeSideChat.id}
                          chatId={chatId}
                          personaId={activePersonaId}
                          sessionId={activeSideChat.sessionId ?? activeSessionId}
                          sideChatId={activeSideChat.id}
                          anchorMessageId={activeSideChat.anchorMessageId}
                          anchorMessage={activeSideChat.anchorMessage}
                          historySuffix={bulletinHistorySuffix}
                          turns={activeTurns}
                          onTurnsChange={setActiveTurns}
                          draft={activeDraft}
                          onDraftChange={setActiveDraft}
                          onDraftLocalChange={setActiveDraftLocal}
                          onMessageActivity={() => { void invalidateOps(chatId) }}
                          onSendComplete={refreshAfterSend}
                          onAddToMainChat={handleAddSideChatTurnToMain}
                          onDelete={() => deleteSideChat(activeSideChat.id)}
                          onClose={closeSideChat}
                        />
                      </ResizablePanel>
                    </>
                  ) : null}
                </ResizablePanelGroup>
                {sideChatHost(threadSurface) === 'overlay' && activeSideChat ? (
                  <SideChatOverlay onClose={closeSideChat}>
                    <SubthreadSidePane
                      key={activeSideChat.id}
                      chatId={chatId}
                      personaId={activePersonaId}
                      sessionId={activeSideChat.sessionId ?? activeSessionId}
                      sideChatId={activeSideChat.id}
                      anchorMessageId={activeSideChat.anchorMessageId}
                      anchorMessage={activeSideChat.anchorMessage}
                      historySuffix={bulletinHistorySuffix}
                      turns={activeTurns}
                      onTurnsChange={setActiveTurns}
                      draft={activeDraft}
                      onDraftChange={setActiveDraft}
                      onDraftLocalChange={setActiveDraftLocal}
                      onMessageActivity={() => { void invalidateOps(chatId) }}
                      onSendComplete={refreshAfterSend}
                      onAddToMainChat={handleAddSideChatTurnToMain}
                      onDelete={() => deleteSideChat(activeSideChat.id)}
                      autoFocusComposer={!narrowViewport}
                      onClose={closeSideChat}
                    />
                  </SideChatOverlay>
                ) : null}
                </div>
              </div>
              {sidebarMode !== 'personas' ? (
                <Suspense fallback={<div className="p-4 text-sm">Loading…</div>}>
                  {sidebarMode === 'groups' ? (
                    <GroupsWorkspace chatId={chatId} personas={personas} queueLanes={queueLanesAll} />
                  ) : null}
                  {sidebarMode === 'coordinator' ? (
                    <CoordinatorWorkspace
                      chatId={chatId}
                      personas={personas}
                      focusTodoId={boardFocusTodoId}
                      onFocusTodoHandled={() => setBoardFocusTodoId(null)}
                      onOpenPersona={(personaId, sessionId) => {
                        const persona = personas.find((item) => item.id === personaId)
                        if (persona) {
                          void switchPersona(persona.name, { sessionId })
                        }
                        setMode('personas')
                      }}
                    />
                  ) : null}
                </Suspense>
              ) : null}
            </div>
          </main>
        </div>

      </div>
      <MobileSheet
        open={topicsSheetOpen}
        onOpenChange={setTopicsSheetOpen}
        title="Session and topics"
        description="Choose a session, change the thread view, or jump to a topic."
      >
        {activePersonaId != null ? (
          <div className="flex flex-col gap-4">
            <div className="flex flex-col gap-2">
              <span className="text-xs font-medium text-[color:var(--mc-text-muted)]">Session</span>
              <SessionPicker
                compact
                sessions={chatSessions}
                activeSessionId={activeSessionId}
                onSelectSession={(sessionId) => {
                  setTopicsSheetOpen(false)
                  void handleSelectSession(sessionId)
                }}
                onCreateSession={handleCreateSession}
                onDeleteSession={handleDeleteSession}
                loading={historyLoading}
              />
              <SideChatsPicker
                sideChats={sideChats}
                sessions={chatSessions}
                activeSideChatId={activeSideChat?.id ?? null}
                loading={sideChatsLoading}
                onSelect={(id) => {
                  setTopicsSheetOpen(false)
                  void handleSelectSideChat(id)
                }}
                onDelete={deleteSideChat}
              />
            </div>
            <div className="flex flex-wrap gap-1" aria-label="Conversation surface">
              <Button
                type="button"
                size="sm"
                variant={threadSurface === 'focus' ? 'secondary' : 'ghost'}
                aria-pressed={threadSurface === 'focus'}
                onClick={() => chooseThreadSurface('focus')}
              >
                Focus
              </Button>
              <Button
                type="button"
                size="sm"
                variant={threadSurface === 'transcript' ? 'secondary' : 'ghost'}
                aria-pressed={threadSurface === 'transcript'}
                onClick={() => chooseThreadSurface('transcript')}
              >
                Transcript
              </Button>
              <Button
                type="button"
                size="sm"
                variant={hideScheduled ? 'secondary' : 'ghost'}
                aria-pressed={hideScheduled}
                onClick={() => setHideScheduled((current) => !current)}
              >
                {hideScheduled ? 'Scheduled hidden' : 'Hide scheduled'}
              </Button>
            </div>
          <TopicOutlineRail
            variant="sheet"
            topics={topics.topics}
            segmenting={topics.segmenting}
            unsegmentedCount={topics.unsegmentedCount}
            loading={topics.loading}
            error={topics.error}
            focusTopicId={threadSurface === 'focus' ? (dossierTopic?.id ?? focusTopicId) : focusTopicId}
            onClearFocus={handleClearTopicFocus}
            onJump={(messageId) => {
              setTopicsSheetOpen(false)
              handleTopicJump(messageId)
            }}
            onSelectSideChat={(id) => {
              setTopicsSheetOpen(false)
              void handleSelectSideChat(id)
            }}
            onRename={(topicId, title) => {
              void topics.rename(topicId, title).catch((err: unknown) => {
                toast.error(err instanceof Error ? err.message : 'Could not rename topic')
              })
            }}
            onTogglePin={(topicId, pinned) => {
              void topics.pin(topicId, pinned).catch((err: unknown) => {
                toast.error(err instanceof Error ? err.message : 'Could not pin topic')
              })
            }}
            onMerge={(topicId) => {
              void topics.merge(topicId).catch((err: unknown) => {
                toast.error(err instanceof Error ? err.message : 'Could not merge topics')
              })
            }}
            onDelete={handleDeleteTopic}
            onCreateSession={handleCreateTopicSession}
            onRefresh={() => {
              void topics.refresh().catch((err: unknown) => {
                toast.error(err instanceof Error ? err.message : 'Could not refresh topics')
              })
            }}
          />
          </div>
        ) : (
          <p className="text-sm text-[color:var(--mc-text-muted)]">Choose a persona to open sessions and topics.</p>
        )}
      </MobileSheet>
      <AppDialogs
        appearance={appearance}
        api={api}
        chatId={chatId}
        activePersonaId={activePersonaId}
        personas={personas}
        settings={{
          open: settingsDialogOpen,
          onOpenChange: setSettingsDialogOpen,
          error: settingsError,
          setError: setSettingsError,
          restartNotice,
          installationStatus,
          restartBusy,
          requestRestart,
          bindings,
          updateChannelPersonaPolicy,
          reloadInstallationStatus: loadSettings,
          page: settingsPage,
          onPageChange: (page) => {
            setSettingsPage(page)
            setSettingsReveal((n) => n + 1)
          },
          revealPage: settingsReveal,
        }}
        onConfigurePersona={openPersonaPage}
        persona={{
          open: personaDialogOpen,
          onOpenChange: (open) => {
            setPersonaDialogOpen(open)
            if (!open) setPersonaError('')
          },
          error: personaError,
          setError: setPersonaError,
          onPersonaBulletinChanged: reloadPersonaBulletin,
          onCreatePersona: handleCreatePersonaClick,
          onRenamePersona: handleRenamePersona,
          onDeletePersona: handleDeletePersonaClick,
          page: personaPage,
          onPageChange: (page) => {
            setPersonaPage(page)
            setPersonaReveal((n) => n + 1)
          },
          revealPage: personaReveal,
          onOpenHousehold: handleOpenSettings,
          onOpenSettingsPage: openSettingsPage,
          engineMark:
            installationStatus?.agent_engine === 'cursor' && installationStatus.cursor_engine_ready === false
              ? 'Missing'
              : undefined,
        }}
        queue={{
          open: queueDialogOpen,
          onOpenChange: setQueueDialogOpen,
          showAllPersonas: queueShowAllPersonas,
          setShowAllPersonas: setQueueShowAllPersonas,
          items: queueDialogItems,
          sampledAtMs: queueSampledAt,
          stoppingRunIds,
          handleQueueAction,
          backgroundJobs: backgroundJobsVisible,
          stoppingBackgroundJobIds,
          isActiveBackgroundJobStatus,
          handleBackgroundJobStop,
          onOpenSchedules: () => setSchedulesDialogOpen(true),
        }}
        schedules={{
          open: schedulesDialogOpen,
          onOpenChange: setSchedulesDialogOpen,
          showArchived: schedulesShowArchived,
          setShowArchived: setSchedulesShowArchived,
          schedules,
          filtered: schedulesFiltered,
          newPrompt: newSchedulePrompt,
          setNewPrompt: setNewSchedulePrompt,
          newType: newScheduleType,
          setNewType: setNewScheduleType,
          newValue: newScheduleValue,
          setNewValue: setNewScheduleValue,
          newDelivery: newScheduleDelivery,
          setNewDelivery: setNewScheduleDelivery,
          newPersonaId: newSchedulePersonaId,
          setNewPersonaId: setNewSchedulePersonaId,
          createSchedule,
          updateSchedule,
          onUnreadCleared: clearUnreadRuns,
          openDetail: (t) => {
            setScheduleDetailTask(t)
            setScheduleDetailPrompt(t.prompt)
            setScheduleDetailScheduleType(t.schedule_type === 'once' ? 'once' : 'cron')
            setScheduleDetailScheduleValue(t.schedule_value)
          },
        }}
        inbox={{
          open: inboxDialogOpen,
          onOpenChange: setInboxDialogOpen,
          unread: inboxUnread,
          todos: inboxTodos,
          loading: inboxLoading,
          busyTodoId: inboxBusyTodoId,
          onRefresh: () => {
            void loadPersonas(chatId)
            void loadInboxTodos(chatId)
          },
          onOpenTarget: (target) => {
            const p = personas.find((x) => x.id === target.personaId)
            if (p) void switchPersona(p.name, { sessionId: target.sessionId })
          },
          onOpenTodo: (todoId) => {
            setBoardFocusTodoId(todoId)
            setMode('coordinator')
          },
          onCompleteTodo: (todoId) => {
            void completeInboxTodo(todoId)
          },
        }}
        scheduleDetail={{
          task: scheduleDetailTask,
          onOpenChange: (o) => {
            if (!o) {
              setScheduleDetailTask(null)
              setScheduleDetailBusy(false)
              setScheduleDetailScheduleValue('')
            }
          },
          prompt: scheduleDetailPrompt,
          setPrompt: setScheduleDetailPrompt,
          scheduleType: scheduleDetailScheduleType,
          setScheduleType: setScheduleDetailScheduleType,
          scheduleValue: scheduleDetailScheduleValue,
          setScheduleValue: setScheduleDetailScheduleValue,
          busy: scheduleDetailBusy,
          setBusy: setScheduleDetailBusy,
          updateSchedule,
          close: () => setScheduleDetailTask(null),
        }}
        agentsMd={{
          open: agentsMdOpen,
          onOpenChange: (o) => {
            setAgentsMdOpen(o)
            if (!o) {
              setAgentsMdError('')
              setAgentsMdBusy(false)
            }
          },
          path: agentsMdPath,
          error: agentsMdError,
          setError: setAgentsMdError,
          setBusy: setAgentsMdBusy,
          content: agentsMdContent,
          setContent: setAgentsMdContent,
          mtimeMs: agentsMdMtimeMs,
          busy: agentsMdBusy,
          load: loadWorkspaceAgentsMd,
          save: saveWorkspaceAgentsMd,
        }}
        artifacts={{
          open: artifactsDialogOpen,
          onOpenChange: setArtifactsDialogOpen,
          onOpenMessage: (id) => { void revealMessageInThread(id) },
        }}
        memory={{
          open: memoryDialogOpen,
          onOpenChange: (open) => {
            setMemoryDialogOpen(open)
            if (!open) {
              setMemoryError('')
              setMemoryBusy(false)
            }
          },
          setError: setMemoryError,
          setBusy: setMemoryBusy,
          pathHint: memoryPathHint,
          error: memoryError,
          content: memoryContent,
          setContent: setMemoryContent,
          mtimeMs: memoryMtimeMs,
          busy: memoryBusy,
          load: loadPersonaMemory,
          save: savePersonaMemory,
        }}
        agentHistory={{
          open: agentHistoryDialogOpen,
          onOpenChange: (open) => {
            setAgentHistoryDialogOpen(open)
            if (open) {
              setAgentHistoryTab('trace')
            }
            if (!open) {
              setAgentHistoryError('')
              setAgentHistoryBusy(false)
            }
          },
          setTab: setAgentHistoryTab,
          setError: setAgentHistoryError,
          setBusy: setAgentHistoryBusy,
          pathHint: agentHistoryPathHint,
          filename: agentHistoryFilename,
          mtimeMs: agentHistoryMtimeMs,
          busy: agentHistoryBusy,
          error: agentHistoryError,
          parsed: agentHistoryParsed,
          tab: agentHistoryTab,
          iterationIdx: agentHistoryIterationIdx,
          setIterationIdx: setAgentHistoryIterationIdx,
          raw: agentHistoryRaw,
          optimizeBusy: agentHistoryOptimizeBusy,
          optimizeNotes: agentHistoryOptimizeNotes,
          setOptimizeNotes: setAgentHistoryOptimizeNotes,
          load: loadAgentHistoryLatest,
          optimize: optimizeAgentHistoryLatest,
        }}
        terminal={{
          open: terminalDialogOpen,
          onOpenChange: (open) => {
            setTerminalDialogOpen(open)
            if (!open) setTerminalError('')
          },
          error: terminalError,
          setError: setTerminalError,
        }}
      />
      {confirmDialog}
      <NewPersonaDialog
        open={newPersonaOpen}
        onOpenChange={setNewPersonaOpen}
        existingNames={personas.map((p) => p.name)}
        onCreate={onCreatePersona}
      />
      <MobileOpsSheet
        open={mobileOpsOpen}
        onOpenChange={setMobileOpsOpen}
        onOpenQueue={() => setQueueDialogOpen(true)}
        onOpenInbox={() => setInboxDialogOpen(true)}
        inboxBadgeCount={inboxBadgeCount}
        schedulesBadgeCount={unreadRuns}
        onOpenSchedules={() => setSchedulesDialogOpen(true)}
        onOpenPrinciples={() => setAgentsMdOpen(true)}
        onOpenArtifacts={() => setArtifactsDialogOpen(true)}
        onOpenTerminal={() => {
          setTerminalError('')
          setTerminalDialogOpen(true)
        }}
        terminalAvailable={installationStatus?.terminal?.web_terminal_available === true}
        onOpenMemory={() => setMemoryDialogOpen(true)}
        onOpenAgentHistory={() => setAgentHistoryDialogOpen(true)}
        agentHistoryDisabled={activePersonaId == null}
        onOpenPersona={activePersonaId == null ? undefined : handleOpenPersona}
        onOpenSettings={handleOpenSettings}
      />
      <ShortcutsDialog open={shortcutsOpen} onOpenChange={setShortcutsOpen} />
      </TooltipProvider>
    
  )
}
