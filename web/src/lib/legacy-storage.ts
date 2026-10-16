/**
 * One-time migration of browser storage keys from the pre-rename prefix.
 *
 * Every persisted UI key is `meido_*`. Builds before the product rename wrote
 * `finally-a-value-bot_*`. On load, each legacy key whose new counterpart is
 * absent is copied once and then removed, so a returning browser keeps its
 * theme, sidebar state, and persona selection without the old name lingering.
 */

export const STORAGE_PREFIX = 'meido_'
export const LEGACY_STORAGE_PREFIX = 'finally-a-value-bot_'

function migrate(storage: Storage): void {
  const legacyKeys: string[] = []
  for (let i = 0; i < storage.length; i += 1) {
    const key = storage.key(i)
    if (key && key.startsWith(LEGACY_STORAGE_PREFIX)) legacyKeys.push(key)
  }
  for (const legacyKey of legacyKeys) {
    const nextKey = STORAGE_PREFIX + legacyKey.slice(LEGACY_STORAGE_PREFIX.length)
    const value = storage.getItem(legacyKey)
    if (value !== null && storage.getItem(nextKey) === null) {
      storage.setItem(nextKey, value)
    }
    storage.removeItem(legacyKey)
  }
}

/** Safe to call on every page load; a no-op once no legacy keys remain. */
export function migrateLegacyStorageKeys(): void {
  for (const pick of [() => window.localStorage, () => window.sessionStorage]) {
    try {
      migrate(pick())
    } catch {
      // Storage can be unavailable (privacy mode, disabled cookies). Nothing to migrate.
    }
  }
}
