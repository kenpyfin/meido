/** Read mc-* design tokens from computed styles for xterm.js theme. */
export function xtermThemeFromCss(root: HTMLElement = document.documentElement): {
  background: string
  foreground: string
  cursor: string
  selectionBackground: string
} {
  const style = getComputedStyle(root)
  const pick = (name: string, fallback: string) => {
    const v = style.getPropertyValue(name).trim()
    return v || fallback
  }
  return {
    background: pick('--mc-bg-main', '#12141a'),
    foreground: pick('--mc-text-primary', '#e8eaef'),
    cursor: pick('--mc-accent', '#7c9cff'),
    selectionBackground: pick('--mc-border-strong', '#3a4460'),
  }
}
