export type SidebarMode = 'personas' | 'groups' | 'coordinator'

export const SIDEBAR_MODE_TITLES: Record<SidebarMode, string> = {
  personas: 'Personas',
  groups: 'Groups',
  coordinator: 'Coordinator',
}

const STORAGE_KEY = 'favb-sidebar-mode'

export function parseSidebarMode(value: string | null | undefined): SidebarMode | null {
  if (value === 'groups' || value === 'coordinator' || value === 'personas') {
    return value
  }
  return null
}

export function readSidebarMode(): SidebarMode {
  if (typeof window === 'undefined') return 'personas'
  const fromHash = parseSidebarMode(window.location.hash.replace(/^#/, ''))
  if (fromHash) return fromHash
  return parseSidebarMode(window.localStorage.getItem(STORAGE_KEY)) ?? 'personas'
}

export function writeSidebarMode(mode: SidebarMode): void {
  window.localStorage.setItem(STORAGE_KEY, mode)
  const url = new URL(window.location.href)
  url.hash = mode === 'personas' ? '' : mode
  window.history.replaceState(null, '', `${url.pathname}${url.search}${url.hash}`)
}
