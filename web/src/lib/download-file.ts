const CODE_EXTENSIONS: Record<string, string> = {
  bash: 'sh',
  c: 'c',
  'c++': 'cpp',
  cpp: 'cpp',
  cs: 'cs',
  csharp: 'cs',
  css: 'css',
  csv: 'csv',
  diff: 'diff',
  docker: 'dockerfile',
  dockerfile: 'dockerfile',
  go: 'go',
  html: 'html',
  ini: 'ini',
  java: 'java',
  javascript: 'js',
  js: 'js',
  json: 'json',
  jsonc: 'jsonc',
  jsx: 'jsx',
  kotlin: 'kt',
  kt: 'kt',
  latex: 'tex',
  lua: 'lua',
  makefile: 'mak',
  markdown: 'md',
  md: 'md',
  php: 'php',
  powershell: 'ps1',
  ps1: 'ps1',
  py: 'py',
  python: 'py',
  rb: 'rb',
  ruby: 'rb',
  rs: 'rs',
  rust: 'rs',
  scss: 'scss',
  sh: 'sh',
  shell: 'sh',
  sql: 'sql',
  swift: 'swift',
  tex: 'tex',
  toml: 'toml',
  ts: 'ts',
  tsx: 'tsx',
  typescript: 'ts',
  xml: 'xml',
  yaml: 'yaml',
  yml: 'yml',
  zsh: 'zsh',
}

/** File extension for a fenced code block language tag; unknown languages fall back to `txt`. */
export function codeFileExtension(language: string | undefined): string {
  return CODE_EXTENSIONS[(language ?? '').toLowerCase()] ?? 'txt'
}

/**
 * Save text as a file via a temporary blob link. The object URL is revoked after a delay:
 * revoking it synchronously after `click()` aborts the download in some browsers.
 */
export function downloadTextFile(filename: string, text: string, mime = 'text/plain'): void {
  const url = URL.createObjectURL(new Blob([text], { type: mime }))
  const link = document.createElement('a')
  link.href = url
  link.download = filename
  link.style.display = 'none'
  document.body.appendChild(link)
  link.click()
  document.body.removeChild(link)
  window.setTimeout(() => URL.revokeObjectURL(url), 1000)
}
