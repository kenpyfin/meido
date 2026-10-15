import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { Badge } from '@/components/ui/badge'
import { Separator } from '@/components/ui/separator'
import { ScrollArea } from '@/components/ui/scroll-area'
import React, { useEffect, useMemo, useRef, useState } from 'react'
import type { SidebarMode } from '../app/sidebar-mode'
import type { Persona } from '../types'

const PRIMARY_THEME_KEYS = new Set(['green', 'slate', 'blue'])

function IconPalette({ className }: { className?: string }) {
  return (
    <svg className={className} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" aria-hidden>
      <path strokeLinecap="round" strokeLinejoin="round" d="M12 2a10 10 0 1 0 0 20 4 4 0 0 0 0-8 2.5 2.5 0 0 1-2.4-2.4 4 4 0 0 0-1.6-3.2A10 10 0 0 0 12 2Z" />
      <circle cx="7.5" cy="10.5" r="1" fill="currentColor" stroke="none" />
      <circle cx="10.5" cy="7.5" r="1" fill="currentColor" stroke="none" />
      <circle cx="14.5" cy="8.5" r="1" fill="currentColor" stroke="none" />
    </svg>
  )
}

function IconSun({ className }: { className?: string }) {
  return (
    <svg className={className} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" aria-hidden>
      <circle cx="12" cy="12" r="4" />
      <path strokeLinecap="round" d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M4.93 19.07l1.41-1.41M17.66 6.34l1.41-1.41" />
    </svg>
  )
}

function IconMoon({ className }: { className?: string }) {
  return (
    <svg className={className} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" aria-hidden>
      <path strokeLinecap="round" strokeLinejoin="round" d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" />
    </svg>
  )
}

const iconBtnClass =
  'inline-flex h-8 w-8 items-center justify-center rounded-md border border-[color:var(--mc-border-soft)] bg-[color:var(--mc-bg-panel)] text-[color:var(--mc-text-muted)] hover:text-[color:var(--mc-text-primary)] hover:brightness-110'

type SessionSidebarProps = {
  appearance: 'dark' | 'light'
  onToggleAppearance: () => void
  uiTheme: string
  onUiThemeChange: (theme: string) => void
  uiThemeOptions: Array<{ key: string; label: string; color: string }>
  personas: Persona[]
  personaHasNew?: Record<number, boolean>
  selectedPersonaId: number | null
  onPersonaSelect: (personaName: string) => void
  onCreatePersona: () => void
  onCloseRequest?: () => void
  mode?: SidebarMode
  onModeChange?: (mode: SidebarMode) => void
  blockedItems?: number
}

const SIDEBAR_MODES: Array<{ id: SidebarMode; label: string }> = [
  { id: 'personas', label: 'Personas' },
  { id: 'groups', label: 'Groups' },
  { id: 'coordinator', label: 'Coordinator' },
]

function ThemeSwatchButton({
  theme,
  selected,
  onSelect,
}: {
  theme: { key: string; label: string; color: string }
  selected: boolean
  onSelect: () => void
}) {
  return (
    <button
      type="button"
      onClick={(e) => {
        e.stopPropagation()
        onSelect()
      }}
      className={
        selected
          ? 'flex items-center gap-2 rounded-md border border-[color:var(--mc-accent)] bg-[color:var(--mc-bg-panel)] px-2 py-1 text-left text-xs text-[color:var(--mc-text-primary)]'
          : 'flex items-center gap-2 rounded-md border border-transparent px-2 py-1 text-left text-xs text-[color:var(--mc-text-muted)] hover:border-[color:var(--mc-border-soft)] hover:bg-[color:var(--mc-bg-panel)]'
      }
      style={
        selected
          ? { borderColor: 'var(--mc-accent)', backgroundColor: 'color-mix(in srgb, var(--mc-accent) 12%, var(--mc-surface-elevated))' }
          : undefined
      }
    >
      <span
        className="h-3 w-3 rounded-sm border border-[color:var(--mc-border-soft)]"
        style={{ backgroundColor: theme.color }}
        aria-hidden="true"
      />
      {theme.label}
    </button>
  )
}

export const SessionSidebar = React.memo(function SessionSidebar({
  appearance,
  onToggleAppearance,
  uiTheme,
  onUiThemeChange,
  uiThemeOptions,
  personas,
  personaHasNew,
  selectedPersonaId,
  onPersonaSelect,
  onCreatePersona,
  onCloseRequest,
  mode = 'personas',
  onModeChange,
  blockedItems = 0,
}: SessionSidebarProps) {
  const isDark = appearance === 'dark'
  const [themeMenuOpen, setThemeMenuOpen] = useState(false)
  const [moreThemesOpen, setMoreThemesOpen] = useState(false)
  const themeMenuRef = useRef<HTMLDivElement | null>(null)
  const themeButtonRef = useRef<HTMLButtonElement | null>(null)

  const { primaryThemes, moreThemes } = useMemo(() => {
    const primary = uiThemeOptions.filter((t) => PRIMARY_THEME_KEYS.has(t.key))
    const more = uiThemeOptions.filter((t) => !PRIMARY_THEME_KEYS.has(t.key))
    return { primaryThemes: primary, moreThemes: more }
  }, [uiThemeOptions])

  useEffect(() => {
    if (moreThemes.some((t) => t.key === uiTheme)) {
      setMoreThemesOpen(true)
    }
  }, [uiTheme, moreThemes])

  useEffect(() => {
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target as Node | null
      if (!target) return
      if (themeButtonRef.current?.contains(target)) return
      if (themeMenuRef.current?.contains(target)) return
      setThemeMenuOpen(false)
    }
    const closeOnScroll = () => setThemeMenuOpen(false)
    window.addEventListener('pointerdown', onPointerDown)
    window.addEventListener('scroll', closeOnScroll, true)
    return () => {
      window.removeEventListener('pointerdown', onPointerDown)
      window.removeEventListener('scroll', closeOnScroll, true)
    }
  }, [])

  return (
    <aside
      className="flex h-full min-h-0 flex-col border-r border-[color:var(--mc-border-soft)] bg-[color:var(--mc-bg-sidebar)] p-4"
    >
      <div className="flex justify-between items-center mb-4">
        <div className="min-w-0">
          <span className="text-xl font-semibold tracking-tight">
            FinallyAValueBot
          </span>
          <span className="mt-0.5 block">
            {mode === 'personas' ? 'Personas & sessions' : mode}
          </span>
        </div>
        <div className="relative flex items-center gap-2">
          {onCloseRequest ? (
            <button
              type="button"
              onClick={() => onCloseRequest()}
              aria-label="Close menu"
              title="Close"
              className={`${iconBtnClass} h-10 w-10 shrink-0 md:hidden`}
            >
              <svg className="size-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden>
                <path d="M18 6L6 18M6 6l12 12" />
              </svg>
            </button>
          ) : null}
          <button
            ref={themeButtonRef}
            type="button"
            onClick={(e) => {
              e.stopPropagation()
              setThemeMenuOpen((v) => !v)
            }}
            aria-label="Change UI theme color"
            title="Theme color"
            className={iconBtnClass}
          >
            <IconPalette className="size-4" />
          </button>
          <button
            type="button"
            onClick={(e) => {
              e.stopPropagation()
              onToggleAppearance()
            }}
            aria-label={isDark ? 'Switch to light mode' : 'Switch to dark mode'}
            title={isDark ? 'Light mode' : 'Dark mode'}
            className={iconBtnClass}
          >
            {isDark ? <IconSun className="size-4" /> : <IconMoon className="size-4" />}
          </button>
          {themeMenuOpen ? (
            <div
              ref={themeMenuRef}
              className="absolute right-0 top-10 z-50 w-56 rounded-lg border border-[color:var(--mc-border-soft)] bg-[color:var(--mc-bg-sidebar)] p-2"
            >
              <span>Theme</span>
              <div className="mt-2 grid grid-cols-2 gap-1">
                {primaryThemes.map((theme) => (
                  <ThemeSwatchButton
                    key={theme.key}
                    theme={theme}
                    selected={uiTheme === theme.key}
                    onSelect={() => {
                      onUiThemeChange(theme.key)
                      setThemeMenuOpen(false)
                    }}
                  />
                ))}
              </div>
              {moreThemes.length > 0 ? (
                <div className="mt-2">
                  <button
                    type="button"
                    className="w-full rounded-md px-1 py-1 text-left text-[11px] text-[color:var(--mc-text-muted)] hover:text-[color:var(--mc-text-primary)]"
                    onClick={(e) => {
                      e.stopPropagation()
                      setMoreThemesOpen((v) => !v)
                    }}
                  >
                    {moreThemesOpen ? 'Fewer colors' : 'More colors'}
                  </button>
                  {moreThemesOpen ? (
                    <div className="mt-1 grid grid-cols-2 gap-1">
                      {moreThemes.map((theme) => (
                        <ThemeSwatchButton
                          key={theme.key}
                          theme={theme}
                          selected={uiTheme === theme.key}
                          onSelect={() => {
                            onUiThemeChange(theme.key)
                            setThemeMenuOpen(false)
                          }}
                        />
                      ))}
                    </div>
                  ) : null}
                </div>
              ) : null}
            </div>
          ) : null}
        </div>
      </div>

      <div className="mb-3 grid grid-cols-3 gap-1" role="tablist" aria-label="Sidebar mode">
        {SIDEBAR_MODES.map((item) => {
          const badge = item.id === 'coordinator' ? blockedItems : 0
          const selected = mode === item.id
          return (
            <button
              key={item.id}
              type="button"
              role="tab"
              aria-selected={selected}
              className={
                selected
                  ? 'rounded-md border border-[color:var(--mc-accent)] bg-[color:var(--mc-bg-panel)] px-2 py-1 text-xs font-medium'
                  : 'rounded-md border border-transparent px-2 py-1 text-xs text-[color:var(--mc-text-muted)] hover:bg-[color:var(--mc-bg-panel)]'
              }
              onClick={() => {
                onModeChange?.(item.id)
                if (item.id !== 'personas') onCloseRequest?.()
              }}
            >
              {item.label}
              {badge > 0 ? ` ${badge}` : ''}
            </button>
          )
        })}
      </div>

      {mode === 'personas' ? (
      <>
      <div className="flex justify-between items-center mb-2">
        <span className="font-medium">
          Persona
        </span>
        <Button size="sm" variant="secondary" onClick={onCreatePersona} title="New persona">
          + New
        </Button>
      </div>

      <Separator className="my-2" />

      <ScrollArea className="mc-persona-list min-h-0 flex-1">
        <div className="flex min-w-0 flex-col pr-1">
          {personas.length === 0 ? (
            <span>Loading…</span>
          ) : (
            personas.map((p, index) => (
              <div
                key={p.id}
                className={
                  index < personas.length - 1
                    ? 'border-b border-[color:var(--mc-border-soft)]'
                    : undefined
                }
              >
                <div
                  className={
                    selectedPersonaId === p.id
                      ? 'group mc-persona-row flex w-full items-center justify-between gap-1 border-l-2 border-[color:var(--mc-accent)] bg-[color:var(--mc-bg-panel)] px-3 py-2'
                      : 'group mc-persona-row flex w-full items-center justify-between gap-1 border-l-2 border-transparent px-3 py-2 text-[color:var(--mc-text-muted)] hover:bg-[color:var(--mc-bg-panel)]/60'
                  }
                >
                  <button
                    type="button"
                    className="flex min-w-0 flex-1 items-center gap-2 text-left text-sm font-medium text-[color:var(--mc-text-primary)]"
                    title={p.name}
                    onClick={() => {
                      onPersonaSelect(p.name)
                      onCloseRequest?.()
                    }}
                  >
                    <span className="min-w-0 truncate">{p.name}</span>
                    {personaHasNew?.[p.id] ? (
                      <span
                        className="h-2 w-2 shrink-0 rounded-full bg-[color:var(--mc-accent)]"
                        aria-label="New messages"
                        title="New messages"
                      />
                    ) : null}
                  </button>
                  {p.is_active ? (
                    <Badge
                      size="sm"
                      variant="secondary"
                      className="shrink-0"
                      title="Channel messages (Telegram, Discord, etc.) route to this persona"
                    >
                      active
                    </Badge>
                  ) : null}
                </div>
              </div>
            ))
          )}
        </div>
      </ScrollArea>
      </>
      ) : (
        <p className="min-h-0 flex-1 text-sm text-[color:var(--mc-text-muted)]">
          {mode === 'groups'
            ? 'Groups open in the main pane. Personas stay available when you switch back.'
            : 'The Coordinator board and daily brief open in the main pane.'}
        </p>
      )}

      <div className="mt-4 border-t border-[color:var(--mc-border-soft)] pt-3">
        <div className="mt-3 flex flex-col items-center gap-1">
          <a
            href="https://finally-a-value-bot.ai"
            target="_blank"
            rel="noreferrer"
            className="text-xs text-[color:var(--mc-text-muted)] hover:text-[color:var(--mc-text-primary)]"
          >
            finally-a-value-bot.ai
          </a>
        </div>
      </div>
    </aside>
  )
})
