import { describe, expect, it } from 'vitest'
import { setupWizardShouldOpen } from './AppDialogs'

describe('setupWizardShouldOpen', () => {
  it('opens when WORKSPACE_DIR is missing, even after the wizard was dismissed', () => {
    expect(setupWizardShouldOpen({ workspace_configured: false, needs_initialization: true }, true)).toBe(true)
  })

  it('stays closed after dismiss when the workspace is already set', () => {
    expect(setupWizardShouldOpen({ workspace_configured: true, needs_initialization: true }, true)).toBe(false)
  })

  it('opens for other missing setup when the wizard has not been dismissed', () => {
    expect(setupWizardShouldOpen({ workspace_configured: true, needs_initialization: true }, false)).toBe(true)
  })
})
