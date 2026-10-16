import { beforeEach, describe, expect, it } from 'vitest'
import {
  LEGACY_STORAGE_PREFIX,
  STORAGE_PREFIX,
  migrateLegacyStorageKeys,
} from './legacy-storage'

describe('migrateLegacyStorageKeys', () => {
  beforeEach(() => {
    window.localStorage.clear()
    window.sessionStorage.clear()
  })

  it('copies legacy keys to the new prefix and removes the old ones', () => {
    window.localStorage.setItem(`${LEGACY_STORAGE_PREFIX}ui_theme`, 'linear')
    window.sessionStorage.setItem(`${LEGACY_STORAGE_PREFIX}onboarding_banner_dismissed`, '1')

    migrateLegacyStorageKeys()

    expect(window.localStorage.getItem(`${STORAGE_PREFIX}ui_theme`)).toBe('linear')
    expect(window.localStorage.getItem(`${LEGACY_STORAGE_PREFIX}ui_theme`)).toBeNull()
    expect(window.sessionStorage.getItem(`${STORAGE_PREFIX}onboarding_banner_dismissed`)).toBe('1')
    expect(window.sessionStorage.getItem(`${LEGACY_STORAGE_PREFIX}onboarding_banner_dismissed`)).toBeNull()
  })

  it('never overwrites a value already stored under the new prefix', () => {
    window.localStorage.setItem(`${STORAGE_PREFIX}appearance`, 'dark')
    window.localStorage.setItem(`${LEGACY_STORAGE_PREFIX}appearance`, 'light')

    migrateLegacyStorageKeys()

    expect(window.localStorage.getItem(`${STORAGE_PREFIX}appearance`)).toBe('dark')
    expect(window.localStorage.getItem(`${LEGACY_STORAGE_PREFIX}appearance`)).toBeNull()
  })

  it('leaves unrelated keys alone and is a no-op when nothing is legacy', () => {
    window.localStorage.setItem('other_app_key', 'x')
    window.localStorage.setItem(`${STORAGE_PREFIX}thread_ui`, 'compact')

    migrateLegacyStorageKeys()

    expect(window.localStorage.getItem('other_app_key')).toBe('x')
    expect(window.localStorage.getItem(`${STORAGE_PREFIX}thread_ui`)).toBe('compact')
    expect(window.localStorage.length).toBe(2)
  })
})
