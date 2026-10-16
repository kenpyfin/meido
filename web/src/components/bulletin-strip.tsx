import React, { useCallback, useEffect, useState } from 'react'
import { IconCockpit } from './icons'
import { MobileSheet } from './mobile-sheet'
import type { PersonaBulletinFocus } from '../types'

const COLLAPSED_PREVIEW_CHARS = 140

export type BulletinStripProps = {
  bulletinFocus: PersonaBulletinFocus | null
  /** Busy count for the cockpit launcher badge (queue + background). */
  cockpitBusyCount?: number
  cockpitExpanded?: boolean
  onToggleCockpit?: () => void
  /** When true, hide the strip-side cockpit launcher (mobile header already has one). */
  hideCockpitLauncher?: boolean
}

function bulletinFullText(focus: PersonaBulletinFocus | null): string {
  if (!focus) return 'No bulletin focus yet.'
  return `${focus.title ? `${focus.title}\n` : ''}${focus.content}`
}

export const BulletinStrip = React.memo(function BulletinStrip({
  bulletinFocus,
  cockpitBusyCount = 0,
  cockpitExpanded = false,
  onToggleCockpit,
  hideCockpitLauncher = false,
}: BulletinStripProps) {
  const [desktopExpanded, setDesktopExpanded] = useState(false)
  const [sheetOpen, setSheetOpen] = useState(false)
  const [isMobile, setIsMobile] = useState(false)

  useEffect(() => {
    const mq = window.matchMedia('(max-width: 767px)')
    const sync = () => setIsMobile(mq.matches)
    sync()
    mq.addEventListener('change', sync)
    return () => mq.removeEventListener('change', sync)
  }, [])

  const fullText = bulletinFullText(bulletinFocus)
  const title = bulletinFocus?.title?.trim() || (bulletinFocus ? 'Bulletin' : 'No bulletin focus yet.')
  const body = bulletinFocus?.content?.trim() ?? ''
  const collapsedPreview =
    body.length > COLLAPSED_PREVIEW_CHARS
      ? `${body.slice(0, COLLAPSED_PREVIEW_CHARS).trim()}…`
      : body || (bulletinFocus ? '' : 'Focus updates after each run.')
  const needsTruncate = fullText.length > COLLAPSED_PREVIEW_CHARS || body.length > COLLAPSED_PREVIEW_CHARS

  const openMobileSheet = useCallback(() => {
    setSheetOpen(true)
  }, [])

  const onStripActivate = useCallback(() => {
    if (isMobile) {
      openMobileSheet()
      return
    }
    if (needsTruncate || desktopExpanded) setDesktopExpanded((v) => !v)
  }, [desktopExpanded, isMobile, needsTruncate, openMobileSheet])

  return (
    <>
      <div className="mc-bulletin-strip" data-expanded={desktopExpanded && !isMobile ? 'true' : 'false'}>
        <button
          type="button"
          className="mc-bulletin-strip-main"
          onClick={onStripActivate}
          title={
            isMobile
              ? 'Open bulletin'
              : needsTruncate
                ? desktopExpanded
                  ? 'Collapse bulletin'
                  : 'Expand bulletin'
                : undefined
          }
          aria-expanded={isMobile ? sheetOpen : desktopExpanded}
        >
          <div className="mc-bulletin-strip-head">
            <span className="mc-bulletin-strip-label text-xs font-medium">
              Bulletin
            </span>
            <span className="mc-bulletin-strip-title">{title}</span>
            {!isMobile && needsTruncate ? (
              <span className="mc-bulletin-strip-chevron" aria-hidden>
                {desktopExpanded ? '▴' : '▾'}
              </span>
            ) : null}
          </div>
          {desktopExpanded && !isMobile ? (
            <div className="mc-bulletin-strip-body whitespace-pre-wrap">{body || fullText}</div>
          ) : collapsedPreview ? (
            <div className="mc-bulletin-strip-preview">{collapsedPreview}</div>
          ) : null}
        </button>
        {!hideCockpitLauncher && onToggleCockpit ? (
          <div className="mc-bulletin-strip-actions">
            <button
              type="button"
              className="mc-cockpit-launch mc-bulletin-strip-cockpit cursor-pointer max-md:!hidden"
              data-active={cockpitExpanded ? 'true' : 'false'}
              data-busy={cockpitBusyCount > 0 ? 'true' : 'false'}
              aria-pressed={cockpitExpanded}
              aria-label={
                cockpitBusyCount > 0 ? `Cockpit, ${cockpitBusyCount} active` : 'Cockpit'
              }
              title="Cockpit"
              onClick={onToggleCockpit}
            >
              <IconCockpit className="size-4 shrink-0" />
              <span className="mc-cockpit-launch-label">Cockpit</span>
              {cockpitBusyCount > 0 ? (
                <span className="mc-badge-counter" aria-hidden>
                  {cockpitBusyCount > 99 ? '99+' : cockpitBusyCount}
                </span>
              ) : null}
            </button>
          </div>
        ) : null}
      </div>

      {isMobile ? (
        <MobileSheet
          open={sheetOpen}
          onOpenChange={setSheetOpen}
          title="Bulletin"
          description="Current persona focus for this session."
        >
          <div className="mc-bulletin-sheet-body whitespace-pre-wrap text-sm leading-relaxed text-[color:var(--mc-text-primary)]">
            {fullText}
          </div>
        </MobileSheet>
      ) : null}
    </>
  )
})
