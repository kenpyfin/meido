import { describe, expect, it } from 'vitest'
import { setupWizardShouldOpen } from './AppDialogs'

describe('setupWizardShouldOpen', () => {
  it('opens when the workspace is not configured (fresh install)', () => {
    expect(setupWizardShouldOpen({ workspace_configured: false })).toBe(true)
  })

  it('stays closed when the workspace is already configured', () => {
    expect(setupWizardShouldOpen({ workspace_configured: true })).toBe(false)
  })

  it('stays closed when status is missing', () => {
    expect(setupWizardShouldOpen(null)).toBe(false)
    expect(setupWizardShouldOpen(undefined)).toBe(false)
  })
})
