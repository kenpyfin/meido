param(
  [string]$Repo = $(if ($env:MEIDO_REPO) { $env:MEIDO_REPO } elseif ($env:FINALLY_A_VALUE_BOT_REPO) { $env:FINALLY_A_VALUE_BOT_REPO } else { 'kenpyfin/meido' }),
  [string]$InstallDir = $(if ($env:MEIDO_INSTALL_DIR) { $env:MEIDO_INSTALL_DIR } elseif ($env:FINALLY_A_VALUE_BOT_INSTALL_DIR) { $env:FINALLY_A_VALUE_BOT_INSTALL_DIR } else { Join-Path $env:USERPROFILE '.local\bin' })
)

$ErrorActionPreference = 'Stop'
$BinName = 'meido.exe'
$ApiUrl = "https://api.github.com/repos/$Repo/releases/latest"

function Write-Info([string]$msg) {
  Write-Host $msg
}

function Resolve-Arch {
  switch ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture) {
    'X64' { return 'x86_64' }
    'Arm64' { return 'aarch64' }
    default { throw "Unsupported architecture: $([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture)" }
  }
}

function Select-AssetUrl([object]$release, [string]$arch) {
  $patterns = @(
    "meido-v?[0-9]+\.[0-9]+\.[0-9]+-$arch-pc-windows-msvc\.zip$",
    "meido-v?[0-9]+\.[0-9]+\.[0-9]+-.*$arch.*windows.*\.zip$"
  )

  foreach ($p in $patterns) {
    $match = $release.assets | Where-Object { $_.browser_download_url -match $p } | Select-Object -First 1
    if ($null -ne $match) {
      return $match.browser_download_url
    }
  }

  return $null
}

function Path-Contains([string]$pathValue, [string]$dir) {
  if ([string]::IsNullOrWhiteSpace($pathValue)) { return $false }
  $needle = $dir.Trim().TrimEnd('\\').ToLowerInvariant()
  foreach ($part in $pathValue.Split(';')) {
    if ([string]::IsNullOrWhiteSpace($part)) { continue }
    if ($part.Trim().TrimEnd('\\').ToLowerInvariant() -eq $needle) {
      return $true
    }
  }
  return $false
}

function Ensure-UserPathContains([string]$dir) {
  $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
  if (Path-Contains $userPath $dir) {
    return $false
  }

  $newPath = if ([string]::IsNullOrWhiteSpace($userPath)) {
    $dir
  } else {
    "$userPath;$dir"
  }

  [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')

  # Also update current process PATH so this shell can find it immediately.
  if (-not (Path-Contains $env:Path $dir)) {
    $env:Path = "$env:Path;$dir"
  }

  return $true
}

$script:EnvKeyAllowlist = '^(ANTHROPIC_API_KEY|OPENAI_API_KEY|GEMINI_API_KEY|XAI_API_KEY|OPENROUTER_API_KEY|DEEPSEEK_API_KEY|CURSOR_API_KEY|TAVILY_API_KEY|BROWSER_MANAGED|STEEL_API_PORT|STEEL_CDP_PORT|STEEL_API_URL|STEEL_DOCKER_IMAGE|STEEL_DOCKER_CONTAINER_NAME|WORKSPACE_DIR|TIMEZONE|VAULT_ORIGIN_VAULT_PATH|VAULT_VECTOR_DB_PATH|VAULT_VECTOR_DB_COLLECTION|VAULT_EMBEDDING_SERVER_URL|VAULT_ORIGIN_VAULT_REPO|VAULT_PRINCIPLES_PATH|LLM_PROVIDER|LLM_MODEL|SYSTEM_LLM_PROVIDER|SYSTEM_LLM_MODEL|TOPICS_MODEL_SOURCE|TOPICS_MODEL|WEB_HOST|WEB_PORT|WEB_AUTH_TOKEN)$'

function Get-EnvValue([string]$Key, [string]$File = '.env') {
  if (-not (Test-Path -LiteralPath $File)) { return '' }
  $pattern = '^' + [regex]::Escape($Key) + '=(.*)$'
  $found = ''
  foreach ($line in Get-Content -LiteralPath $File) {
    if ($line -match $pattern) {
      $found = $Matches[1].Trim().Trim('"').Trim("'")
    }
  }
  return $found
}

function Read-Prefilled([string]$Key, [string]$Fallback) {
  $current = Get-EnvValue $Key
  $shown = if (-not [string]::IsNullOrWhiteSpace($current)) { $current } else { $Fallback }
  $label = if (-not [string]::IsNullOrWhiteSpace($shown)) { "$Key [$shown]" } else { "$Key [empty]" }
  $value = Read-Host $label
  if ([string]::IsNullOrWhiteSpace($value)) { return $shown }
  return $value.Trim()
}

function Save-Prefilled([string]$Key, [string]$Fallback, [string]$Description = '') {
  if (-not [string]::IsNullOrWhiteSpace($Description)) {
    Write-Info "  $Description"
  }
  $value = Read-Prefilled $Key $Fallback
  if ($value -match "[\r\n`"]") {
    Write-Info "Skipped ${Key}: value must be a single line without quotes."
    return
  }
  if ([string]::IsNullOrWhiteSpace($value)) {
    Write-Info "Left $Key unset."
    return
  }
  Set-EnvKey $Key $value
  Write-Info "Saved ${Key}=$value"
}

function Convert-ToWorkspaceRelative([string]$Workspace, [string]$Value) {
  $wsFull = [System.IO.Path]::GetFullPath($Workspace).TrimEnd('\', '/')
  $full = [System.IO.Path]::GetFullPath($Value).TrimEnd('\', '/')
  $prefix = $wsFull + [System.IO.Path]::DirectorySeparatorChar
  if ($full.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    return $full.Substring($wsFull.Length).TrimStart('\', '/').Replace('\', '/')
  }
  if ([System.IO.Path]::IsPathRooted($Value) -or $Value -match '^[A-Za-z]:[\\/]') {
    return $Value
  }
  return ($Value -replace '^\./', '' -replace '^\.\\', '')
}

function Save-InsideWorkspace([string]$Key, [string]$RelativeFallback, [string]$Description = '') {
  $ws = Get-EnvValue 'WORKSPACE_DIR'
  if ([string]::IsNullOrWhiteSpace($ws)) { $ws = './workspace' }
  $shown = ((Join-WorkspaceChild $ws $RelativeFallback) -replace '\\', '/')
  if (-not [string]::IsNullOrWhiteSpace($Description)) {
    Write-Info "  $Description Default follows the workspace you entered (${ws}): $shown."
  }
  $value = Read-Host "$Key [$shown]"
  if ([string]::IsNullOrWhiteSpace($value)) { $value = $shown }
  $value = $value.Trim()
  if ($value -match "[\r\n`"]") {
    Write-Info "Skipped ${Key}: value must be a single line without quotes."
    return
  }
  $stored = Convert-ToWorkspaceRelative $ws $value
  if ([string]::IsNullOrWhiteSpace($stored)) {
    Write-Info "Left $Key unset."
    return
  }
  Set-EnvKey $Key $stored
  if ([System.IO.Path]::IsPathRooted($stored) -or $stored -match '^[A-Za-z]:[\\/]') {
    Write-Info "Saved ${Key}=$stored"
  } else {
    Write-Info "Saved ${Key}=$stored (inside $ws)"
  }
}

function Invoke-WorkspaceAndVault {
  Write-Info ''
  Write-Info 'Step — workspace'
  Write-Info 'Press Enter to keep the value in brackets.'
  Save-Prefilled 'WORKSPACE_DIR' './workspace' 'Root folder for chats, skills, runtime data, and tools (workspace/runtime, workspace/skills, workspace/shared).'
  Save-Prefilled 'TIMEZONE' 'UTC' 'IANA timezone for the clock the agent sees and for scheduled jobs (example: America/Los_Angeles).'

  $wsForVault = Get-EnvValue 'WORKSPACE_DIR'
  if ([string]::IsNullOrWhiteSpace($wsForVault)) { $wsForVault = './workspace' }
  Write-Info ''
  Write-Info 'Step — ORIGIN vault'
  Write-Info "Defaults follow the workspace you entered ($wsForVault). Press Enter to keep the bracketed path."
  Write-Info 'The embedding server URL has no default; Enter leaves it unset.'
  Save-InsideWorkspace 'VAULT_ORIGIN_VAULT_PATH' 'shared/ORIGIN' 'Obsidian vault of markdown notes.'
  Save-InsideWorkspace 'VAULT_VECTOR_DB_PATH' 'shared/vault_db' 'Local Chroma directory for the vault embeddings.'
  Save-Prefilled 'VAULT_VECTOR_DB_COLLECTION' 'vault' 'Chroma collection name inside that database.'
  Save-Prefilled 'VAULT_PRINCIPLES_PATH' 'AGENTS.md' 'Principles file the agent reads. Default is AGENTS.md at the workspace root.'
  Save-Prefilled 'VAULT_EMBEDDING_SERVER_URL' '' 'HTTP URL of the embedding server (for example llama.cpp). Leave empty until you have one.'
  Save-Prefilled 'VAULT_ORIGIN_VAULT_REPO' '' 'Git URL used to clone or pull the vault. Leave empty to use only the local folder.'
  Add-WorkspaceVaultGitignore
  Install-DefaultAgentsMd
}

function Install-DefaultAgentsMd {
  $ws = Get-WorkspaceDir
  if ([string]::IsNullOrWhiteSpace($ws)) { $ws = './workspace' }
  $rel = Get-EnvValue 'VAULT_PRINCIPLES_PATH'
  if ([string]::IsNullOrWhiteSpace($rel)) { $rel = 'AGENTS.md' }
  $rel = $rel.Trim()
  if ($rel.StartsWith('/') -or $rel.StartsWith('\') -or $rel.Contains('..') -or $rel.Contains(':')) {
    Write-Info "Not writing default principles for $rel."
    return
  }
  $dest = Join-Path ($ws.TrimEnd('\', '/')) ($rel -replace '/', [IO.Path]::DirectorySeparatorChar)
  if (Test-Path -LiteralPath $dest) {
    Write-Info "Principles file already present: $dest"
    return
  }
  $parent = Split-Path -Parent $dest
  if (-not [string]::IsNullOrWhiteSpace($parent)) {
    New-Item -ItemType Directory -Force -Path $parent | Out-Null
  }
  $local = Join-Path (Get-Location) 'defaults\AGENTS.md'
  if (Test-Path -LiteralPath $local) {
    Copy-Item -LiteralPath $local -Destination $dest
    Write-Info "Wrote default principles to $dest"
    return
  }
  $url = "https://raw.githubusercontent.com/$Repo/main/defaults/AGENTS.md"
  try {
    Invoke-WebRequest -Uri $url -OutFile $dest -UseBasicParsing
    if ((Get-Item -LiteralPath $dest).Length -gt 0) {
      Write-Info "Wrote default principles to $dest"
      return
    }
  } catch {
  }
  if (Test-Path -LiteralPath $dest) {
    Remove-Item -LiteralPath $dest -Force -ErrorAction SilentlyContinue
  }
  Write-Info 'Default principles will be written on first start if this Meido build includes them.'
}

function Test-GitignoreLinePresent([string]$File, [string]$Pattern) {
  if (-not (Test-Path -LiteralPath $File)) { return $false }
  $bare = $Pattern.TrimEnd('/')
  $want = @($Pattern, $bare, "/$bare", "/$Pattern")
  foreach ($line in Get-Content -LiteralPath $File) {
    if ($want -contains $line.Trim()) { return $true }
  }
  return $false
}

function Add-GitignoreDir([string]$RawPath) {
  if ([string]::IsNullOrWhiteSpace($RawPath)) { return }
  if ($RawPath -match '[\r\n#\!\*\?\[]') {
    Write-Info 'Skipped .gitignore for an unsafe path.'
    return
  }
  $root = [System.IO.Path]::GetFullPath((Get-Location).Path).TrimEnd('\', '/')
  $target = [System.IO.Path]::GetFullPath($RawPath).TrimEnd('\', '/')
  $rootPrefix = $root + [System.IO.Path]::DirectorySeparatorChar
  if (-not $target.StartsWith($rootPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    Write-Info "Not adding $RawPath to .gitignore (outside this repo)."
    return
  }
  $rel = $target.Substring($root.Length).TrimStart('\', '/').Replace('\', '/')
  if ([string]::IsNullOrWhiteSpace($rel) -or $rel -eq '.git' -or $rel.StartsWith('.git/')) {
    Write-Info "Refusing to gitignore $RawPath."
    return
  }
  $pattern = "$rel/"
  $file = Join-Path (Get-Location) '.gitignore'
  if (Test-GitignoreLinePresent $file $pattern) { return }
  $marker = '# Meido workspace and vault (installer)'
  $lines = @()
  if (Test-Path -LiteralPath $file) {
    $lines = @(Get-Content -LiteralPath $file)
  }
  if ($lines -notcontains $marker) {
    if ($lines.Count -gt 0 -and -not [string]::IsNullOrWhiteSpace($lines[-1])) {
      $lines += ''
    }
    $lines += $marker
  }
  $lines += $pattern
  $utf8 = New-Object System.Text.UTF8Encoding $false
  [System.IO.File]::WriteAllLines($file, $lines, $utf8)
  Write-Info "Added $pattern to .gitignore"
}

function Join-WorkspaceChild([string]$Workspace, [string]$Child) {
  if ($Child.StartsWith('/') -or $Child -match '^[A-Za-z]:[\\/]') {
    return $Child
  }
  $relative = $Child
  if ($relative.StartsWith('./') -or $relative.StartsWith('.\')) {
    $relative = $relative.Substring(2)
  }
  return Join-Path ($Workspace.TrimEnd('\', '/')) $relative
}

function Add-WorkspaceVaultGitignore {
  if (-not (Test-Path -LiteralPath '.git') -and -not (Test-Path -LiteralPath '.gitignore')) {
    return
  }
  $ws = Get-EnvValue 'WORKSPACE_DIR'
  if ([string]::IsNullOrWhiteSpace($ws)) { $ws = './workspace' }
  $origin = Get-EnvValue 'VAULT_ORIGIN_VAULT_PATH'
  if ([string]::IsNullOrWhiteSpace($origin)) { $origin = 'shared/ORIGIN' }
  $db = Get-EnvValue 'VAULT_VECTOR_DB_PATH'
  if ([string]::IsNullOrWhiteSpace($db)) { $db = 'shared/vault_db' }
  Add-GitignoreDir $ws
  Add-GitignoreDir (Join-WorkspaceChild $ws $origin)
  Add-GitignoreDir (Join-WorkspaceChild $ws $db)
}

function Confirm-Step([string]$Prompt, [bool]$DefaultYes = $false) {
  $hint = if ($DefaultYes) { '[Y/n]' } else { '[y/N]' }
  $ans = Read-Host "$Prompt $hint"
  if ([string]::IsNullOrWhiteSpace($ans)) { return $DefaultYes }
  return $ans.Trim().ToLowerInvariant() -in @('y', 'yes')
}

function Read-SecretPlain([string]$Prompt) {
  $secure = Read-Host -Prompt $Prompt -AsSecureString
  $bstr = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($secure)
  try {
    return [Runtime.InteropServices.Marshal]::PtrToStringBSTR($bstr)
  } finally {
    [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($bstr)
  }
}

function Test-EnvKeySet([string]$Key, [string]$File = '.env') {
  if (-not (Test-Path -LiteralPath $File)) { return $false }
  $pattern = '^' + [regex]::Escape($Key) + '=\S'
  foreach ($line in Get-Content -LiteralPath $File) {
    if ($line -match $pattern) { return $true }
  }
  return $false
}

function Set-EnvKey([string]$Key, [string]$Value, [string]$File = '.env') {
  if ($Key -notmatch $script:EnvKeyAllowlist) {
    throw 'Refusing to write unexpected env key.'
  }
  if ($Value -match "[\r\n]") {
    throw "Refusing to write a multi-line value for $Key."
  }
  if ([string]::IsNullOrWhiteSpace($Value)) {
    Write-Info "Skipped $Key (empty)."
    return
  }
  $lines = @()
  if (Test-Path -LiteralPath $File) {
    $lines = @(Get-Content -LiteralPath $File)
  }
  $found = $false
  $out = foreach ($line in $lines) {
    if ($line -match ('^' + [regex]::Escape($Key) + '=')) {
      $found = $true
      "$Key=$Value"
    } else {
      $line
    }
  }
  if (-not $found) {
    $out = @($out) + "$Key=$Value"
  }
  $path = Join-Path (Get-Location) $File
  $utf8 = New-Object System.Text.UTF8Encoding $false
  [System.IO.File]::WriteAllLines($path, $out, $utf8)
}

function Set-EnvKeyIfEmpty([string]$Key, [string]$Value) {
  if (Test-EnvKeySet $Key) { return }
  Set-EnvKey $Key $Value
}

function Save-SecretKey([string]$Key, [string]$Label) {
  if (Test-EnvKeySet $Key) {
    if (-not (Confirm-Step "$Key is already set. Replace it?" $false)) {
      Write-Info "Kept existing $Key."
      return
    }
  }
  $value = Read-SecretPlain $Label
  if ([string]::IsNullOrWhiteSpace($value)) {
    Write-Info "Skipped $Key."
    return
  }
  Set-EnvKey $Key $value.Trim()
  Write-Info "Saved $Key in .env (value not shown)."
}

function Get-WorkspaceDir {
  $raw = './workspace'
  if (Test-Path -LiteralPath '.env') {
    foreach ($line in Get-Content -LiteralPath '.env') {
      if ($line -match '^WORKSPACE_DIR=(.+)$') {
        $raw = $Matches[1].Trim().Trim('"').Trim("'")
      }
    }
  }
  if ([string]::IsNullOrWhiteSpace($raw)) { $raw = './workspace' }
  return $raw
}

function Save-LlmSetting([string]$Key, [string]$Value) {
  Set-EnvKey $Key $Value
  Write-Info "Saved ${Key}=$Value"
  $db = Join-Path (Get-WorkspaceDir) 'runtime\meido.db'
  $legacyDb = Join-Path (Get-WorkspaceDir) 'runtime\finally_a_value_bot.db'
  if (-not (Test-Path -LiteralPath $db) -and (Test-Path -LiteralPath $legacyDb)) {
    # Pre-rename install; the binary renames this file on its next start.
    $db = $legacyDb
  }
  if (-not (Test-Path -LiteralPath $db)) { return }
  $py = Get-Command python -ErrorAction SilentlyContinue
  if (-not $py) { $py = Get-Command python3 -ErrorAction SilentlyContinue }
  if (-not $py) {
    Write-Info "python is missing, so $Key is in .env only until the next start."
    return
  }
  $now = [DateTime]::UtcNow.ToString('yyyy-MM-ddTHH:mm:ssZ')
  & $py.Source - $db $Key $Value $now @'
import sqlite3, sys
db, key, value, now = sys.argv[1:]
con = sqlite3.connect(db)
con.execute(
    "INSERT INTO app_settings (key, value, updated_at) VALUES (?, ?, ?) "
    "ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
    (key, value, now),
)
con.commit()
'@
}

function Get-ProviderModels([string]$Provider, [string]$ApiKey) {
  $headers = @{}
  switch ($Provider) {
    'anthropic' {
      $uri = 'https://api.anthropic.com/v1/models?limit=100'
      $headers['x-api-key'] = $ApiKey
      $headers['anthropic-version'] = '2023-06-01'
    }
    'google' { $uri = 'https://generativelanguage.googleapis.com/v1beta/openai/models' }
    'openai' { $uri = 'https://api.openai.com/v1/models' }
    'xai' { $uri = 'https://api.x.ai/v1/models' }
    'openrouter' { $uri = 'https://openrouter.ai/api/v1/models' }
    'deepseek' { $uri = 'https://api.deepseek.com/v1/models' }
    default { return @() }
  }
  if ($Provider -ne 'anthropic') {
    $headers['Authorization'] = "Bearer $ApiKey"
  }
  try {
    $raw = Invoke-RestMethod -Method Get -Uri $uri -Headers $headers
  } catch {
    Write-Info "Could not list models from the $Provider API. Type a model id, or press Enter for the default."
    return @()
  }
  $rows = @()
  if ($raw.data) { $rows = @($raw.data) }
  elseif ($raw.models) { $rows = @($raw.models) }
  $ids = New-Object System.Collections.Generic.List[string]
  foreach ($item in $rows) {
    $id = [string]($item.id)
    if ([string]::IsNullOrWhiteSpace($id)) { $id = [string]($item.name) }
    if ($id.StartsWith('models/')) { $id = $id.Substring(7) }
    if (-not [string]::IsNullOrWhiteSpace($id) -and -not $ids.Contains($id)) {
      $ids.Add($id)
    }
  }
  return $ids
}

function Read-ModelChoice([string]$Purpose, [string]$Fallback, [string[]]$Models) {
  $shown = @()
  $limit = [Math]::Min(40, $Models.Count)
  for ($i = 0; $i -lt $limit; $i++) {
    Write-Info ("  {0}) {1}" -f ($i + 1), $Models[$i])
    $shown += $Models[$i]
  }
  if ($Models.Count -gt 40) {
    Write-Info "  Showing 40 of $($Models.Count). Type a model id if yours is not listed."
  }
  Write-Info "  $Purpose"
  $choice = Read-Host "Model [$Fallback]"
  if ([string]::IsNullOrWhiteSpace($choice)) { return $Fallback }
  $number = 0
  if ([int]::TryParse($choice, [ref]$number) -and $number -ge 1 -and $number -le $shown.Count) {
    return $shown[$number - 1]
  }
  return $choice.Trim()
}

function Test-LocalWebHost([string]$HostName) {
  return $HostName -in @('127.0.0.1', 'localhost', '::1', '[::1]')
}

function Split-WebAddress([string]$Raw) {
  $text = $Raw.Trim()
  $text = $text -replace '^https?://', ''
  $text = ($text -split '[/#?]')[0]
  $hostName = $text
  $port = ''
  if ($text.StartsWith('[')) {
    $end = $text.IndexOf(']')
    if ($end -gt 0) {
      $hostName = $text.Substring(1, $end - 1)
      if ($text.Length -gt ($end + 1) -and $text[$end + 1] -eq ':') {
        $port = $text.Substring($end + 2)
      }
    }
  } elseif (($text.ToCharArray() | Where-Object { $_ -eq ':' }).Count -eq 1) {
    $hostName = $text.Split(':')[0]
    $port = $text.Split(':')[-1]
  }
  return [pscustomobject]@{ Host = $hostName; Port = $port }
}

function Test-WebName([string]$HostName) {
  return $HostName -match '^[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?(\.[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?)*$'
}

function Get-LanIPv4 {
  try {
    $cfg = Get-NetIPConfiguration | Where-Object {
      $null -ne $_.IPv4DefaultGateway -and $_.NetAdapter.Status -eq 'Up'
    } | Select-Object -First 1
    if ($cfg -and $cfg.IPv4Address) {
      $addr = @($cfg.IPv4Address)[0].IPAddress
      if (-not [string]::IsNullOrWhiteSpace($addr)) { return $addr }
    }
  } catch {
  }
  return '<this-machine-LAN-IP>'
}

function Write-DnsSetup([string]$Name) {
  $ip = Get-LanIPv4
  Write-Info "How to make ${Name} resolve to this machine"
  Write-Info "Meido does not register DNS. Point ${Name} at ${ip}, then open http://${Name}:10961 (or your WEB_PORT)."
  Write-Info '  mDNS, when the name ends in .local and matches this computer''s hostname:'
  Write-Info '    Linux: sudo apt install avahi-daemon && sudo systemctl enable --now avahi-daemon'
  Write-Info '           hostname    # the advertised name is <that>.local'
  Write-Info "           Extra name: avahi-publish -a -R ${Name} ${ip}"
  Write-Info '    macOS: sudo scutil --set LocalHostName <label>    # answers as <label>.local'
  Write-Info '    Windows: rename the PC to the label before .local (Settings → System → About)'
  Write-Info '  Any other device, or a name that is not this PC''s hostname — one hosts-file line:'
  Write-Info '    Linux/macOS /etc/hosts'
  Write-Info '    Windows C:\Windows\System32\drivers\etc\hosts'
  Write-Info "    ${ip}  ${Name}"
  Write-Info '  Router or Pi-hole: an A record from the name to that IP. Prefer a suffix such as .lan; many routers will not answer names that end in .local.'
}

function Get-DnsPromptDefault([string]$Current) {
  if ([string]::IsNullOrWhiteSpace($Current) -or $Current -match '^[0-9]' -or $Current.Contains(':') -or $Current -in @('localhost', '0.0.0.0')) {
    return 'pp.local'
  }
  return $Current
}

function Write-WebUiUrl {
  $hostName = Get-EnvValue 'WEB_HOST'
  $port = Get-EnvValue 'WEB_PORT'
  if ([string]::IsNullOrWhiteSpace($hostName)) { $hostName = '127.0.0.1' }
  if ([string]::IsNullOrWhiteSpace($port)) { $port = '10961' }
  $open = if ($hostName -in @('0.0.0.0', '::', '[::]')) { '127.0.0.1' } else { $hostName }
  Write-Info 'Start Meido before opening the web UI: meido start'
  if ($port -eq '10961') {
    Write-Info "After it starts, open http://${open}:${port}. Default port is 10961. If 10961 is taken, Meido uses the next free port, up to 50 ports higher."
  } else {
    Write-Info "After it starts, open http://${open}:${port} (WEB_PORT)."
  }
  if ($hostName -in @('0.0.0.0', '::', '[::]')) {
    Write-Info "It will listen on ${hostName}:${port}. Other devices use this machine's IP and the same port."
  }
}

function Invoke-WebAccess {
  Write-Info ''
  Write-Info 'Step — Web UI address'
  Write-Info 'Choose who can open the web UI.'
  Write-Info '  1) This machine only — http://127.0.0.1:10961'
  Write-Info '  2) Every interface — 0.0.0.0, so any IP on this machine can reach it. Requires an access token.'
  Write-Info '  3) A DNS name or URL (example: pp.local or http://pp.local:10961). Listens on every interface under that name. Requires an access token.'
  $current = Get-EnvValue 'WEB_HOST'
  if ([string]::IsNullOrWhiteSpace($current)) { $current = '127.0.0.1' }
  $defaultChoice = if ($current -eq '0.0.0.0') { '2' } elseif (Test-LocalWebHost $current) { '1' } else { '3' }
  $choice = Read-Host "Choose [$defaultChoice]"
  if ([string]::IsNullOrWhiteSpace($choice)) { $choice = $defaultChoice }
  $hostName = '127.0.0.1'
  switch ($choice) {
    '2' { $hostName = '0.0.0.0' }
    '3' {
      $dnsDefault = Get-DnsPromptDefault $current
      $typed = Read-Host "DNS name or URL [$dnsDefault]"
      if ([string]::IsNullOrWhiteSpace($typed)) { $typed = $dnsDefault }
      $parsed = Split-WebAddress $typed
      if (Test-WebName $parsed.Host) {
        $hostName = $parsed.Host
        $portNumber = 0
        if (-not [string]::IsNullOrWhiteSpace($parsed.Port) -and [int]::TryParse($parsed.Port, [ref]$portNumber) -and $portNumber -ge 1 -and $portNumber -le 65535) {
          Set-EnvKey 'WEB_PORT' "$portNumber"
          Write-Info "Saved WEB_PORT=$portNumber"
        }
      } else {
        Write-Info 'Enter a DNS name such as pp.local. Kept 127.0.0.1.'
      }
    }
  }
  Set-EnvKey 'WEB_HOST' $hostName
  Write-Info "Saved WEB_HOST=$hostName"
  if (Test-LocalWebHost $hostName) {
    Write-WebUiUrl
    return
  }
  if ($hostName -notmatch '^[0-9.]+$') {
    Write-DnsSetup $hostName
  }
  Write-Info 'This name is reachable beyond this machine, so WEB_AUTH_TOKEN is required. The value is hidden and is not printed.'
  Save-SecretKey 'WEB_AUTH_TOKEN' 'Paste WEB_AUTH_TOKEN (input hidden)'
  if (-not (Test-EnvKeySet 'WEB_AUTH_TOKEN')) {
    Write-Info 'No access token, so the web UI stays on 127.0.0.1.'
    Set-EnvKey 'WEB_HOST' '127.0.0.1'
    Write-Info 'Saved WEB_HOST=127.0.0.1'
  }
  Write-WebUiUrl
}

function Invoke-BuiltinGuide {
  if (-not (Test-Path -LiteralPath '.env') -and (Test-Path -LiteralPath '.env.example')) {
    Copy-Item -LiteralPath '.env.example' -Destination '.env'
    Write-Info 'Copied .env.example to .env.'
  } elseif (-not (Test-Path -LiteralPath '.env')) {
    Write-Info 'No .env.example in this directory. Create .env before configuring built-ins.'
    return
  }

  Write-Info ''
  Write-Info 'Guided setup for built-in features. Secrets are written to .env and are not printed.'
  Invoke-WorkspaceAndVault
  Invoke-WebAccess

  Write-Info ''
  Write-Info 'Step — System LLM'
  Write-Info 'This model splits chats into topics, updates the bulletin, and runs other learning.'
  Write-Info 'It is saved as SYSTEM_LLM_PROVIDER and SYSTEM_LLM_MODEL. The channel chat model is chosen later in Settings.'
  Write-Info '  1) Anthropic   ANTHROPIC_API_KEY'
  Write-Info '  2) OpenAI      OPENAI_API_KEY'
  Write-Info '  3) Gemini      GEMINI_API_KEY'
  Write-Info '  4) xAI         XAI_API_KEY'
  Write-Info '  5) OpenRouter  OPENROUTER_API_KEY'
  Write-Info '  6) DeepSeek    DEEPSEEK_API_KEY'
  Write-Info '  7) Skip'
  $choice = Read-Host 'Choose a provider [1-7]'
  $llmPick = switch ($choice) {
    '1' { [pscustomobject]@{ Key = 'ANTHROPIC_API_KEY'; Provider = 'anthropic'; Fallback = 'claude-opus-4-7' } }
    '2' { [pscustomobject]@{ Key = 'OPENAI_API_KEY'; Provider = 'openai'; Fallback = 'gpt-5.4' } }
    '3' { [pscustomobject]@{ Key = 'GEMINI_API_KEY'; Provider = 'google'; Fallback = 'gemini-3.1-pro-preview' } }
    '4' { [pscustomobject]@{ Key = 'XAI_API_KEY'; Provider = 'xai'; Fallback = 'grok-4.3' } }
    '5' { [pscustomobject]@{ Key = 'OPENROUTER_API_KEY'; Provider = 'openrouter'; Fallback = 'openrouter/auto' } }
    '6' { [pscustomobject]@{ Key = 'DEEPSEEK_API_KEY'; Provider = 'deepseek'; Fallback = 'deepseek-chat' } }
    default { $null }
  }
  if ($null -eq $llmPick) {
    Write-Info 'Skipped system LLM.'
  } else {
    Save-SecretKey $llmPick.Key "Paste $($llmPick.Key) (input hidden)"
    if (Test-EnvKeySet $llmPick.Key) {
      $models = @(Get-ProviderModels $llmPick.Provider (Get-EnvValue $llmPick.Key))
      $fallback = Get-EnvValue 'SYSTEM_LLM_MODEL'
      if ([string]::IsNullOrWhiteSpace($fallback)) { $fallback = Get-EnvValue 'TOPICS_MODEL' }
      if ([string]::IsNullOrWhiteSpace($fallback)) { $fallback = $llmPick.Fallback }
      $systemLlm = Read-ModelChoice 'System LLM. Topic split, bulletin, and learning use this model.' $fallback $models
      if (-not [string]::IsNullOrWhiteSpace($systemLlm)) {
        Save-LlmSetting 'SYSTEM_LLM_PROVIDER' $llmPick.Provider
        Save-LlmSetting 'SYSTEM_LLM_MODEL' $systemLlm
      } else {
        Write-Info 'No system LLM saved.'
      }
    } else {
      Write-Info "System LLM was not saved because $($llmPick.Key) is unset."
    }
  }

  Write-Info ''
  Write-Info 'Step — Cursor agent engine API key'
  Write-Info 'Cursor Dashboard → Integrations. The sidecar also needs Node.js and npm.'
  if (-not ((Get-Command node -ErrorAction SilentlyContinue) -and (Get-Command npm -ErrorAction SilentlyContinue))) {
    Write-Info 'node/npm not found. Install Node.js before using Agent engine = Cursor.'
  }
  if (Confirm-Step 'Save CURSOR_API_KEY into .env now?' $true) {
    Save-SecretKey 'CURSOR_API_KEY' 'Paste CURSOR_API_KEY (input hidden)'
  } else {
    Write-Info 'Skipped CURSOR_API_KEY. Set it before switching the agent engine to Cursor.'
  }

  Write-Info ''
  Write-Info 'Step — MemPalace recall'
  Write-Info 'Installs the mempalace package into WORKSPACE_DIR/runtime/mempalace-venv.'
  $python = Get-Command python -ErrorAction SilentlyContinue
  if (-not $python) { $python = Get-Command py -ErrorAction SilentlyContinue }
  if (-not $python) {
    Write-Info 'python is missing. Install it, then rerun this installer or start the bot.'
  } elseif (Confirm-Step 'Install MemPalace into the workspace runtime now?' $true) {
    $ws = Get-WorkspaceDir
    $runtime = Join-Path $ws 'runtime'
    New-Item -ItemType Directory -Force -Path (Join-Path $runtime 'mempalace') | Out-Null
    $venv = Join-Path $runtime 'mempalace-venv'
    $venvPy = Join-Path $venv 'Scripts\python.exe'
    if (-not (Test-Path -LiteralPath $venvPy)) {
      Write-Info "Creating venv at $venv"
      & $python.Source -m venv $venv
    }
    if (Test-Path -LiteralPath $venvPy) {
      Write-Info 'Installing mempalace (this can take a few minutes)...'
      & $venvPy -m pip install mempalace
      if ($LASTEXITCODE -eq 0) {
        Write-Info "MemPalace is installed in $venv."
      } else {
        Write-Info 'MemPalace pip install failed. The bot will retry on startup.'
      }
    }
  } else {
    Write-Info 'Skipped MemPalace install.'
  }

  Write-Info ''
  Write-Info 'Step — Steel browser'
  Write-Info 'Pulls ghcr.io/steel-dev/steel-browser and sets BROWSER_MANAGED=true so the bot starts it.'
  if (-not (Get-Command docker -ErrorAction SilentlyContinue)) {
    Write-Info 'docker is missing. Install Docker, then rerun this installer to pull Steel.'
  } elseif (Confirm-Step 'Install Steel (docker pull + local skill venv) and enable BROWSER_MANAGED?' $true) {
    $image = 'ghcr.io/steel-dev/steel-browser:latest'
    Write-Info "Pulling $image..."
    & docker pull $image
    if ($LASTEXITCODE -ne 0) {
      Write-Info 'docker pull failed. Check that the Docker daemon is running.'
    }
    Set-EnvKey 'BROWSER_MANAGED' 'true'
    Set-EnvKeyIfEmpty 'STEEL_API_PORT' '13920'
    Set-EnvKeyIfEmpty 'STEEL_CDP_PORT' '13923'
    Set-EnvKeyIfEmpty 'STEEL_API_URL' 'http://127.0.0.1:13920'
    Set-EnvKeyIfEmpty 'STEEL_DOCKER_IMAGE' $image
    Set-EnvKeyIfEmpty 'STEEL_DOCKER_CONTAINER_NAME' 'meido-steel'
    Write-Info 'Steel bootstrap written to .env. The bot starts the container on boot.'
    $req = 'builtin_skills/steel-browser/requirements.txt'
    if ((Test-Path -LiteralPath $req) -and $python) {
      $ws = Get-WorkspaceDir
      $venv = Join-Path $ws 'shared\.venv-steel'
      $venvPy = Join-Path $venv 'Scripts\python.exe'
      Write-Info 'Creating the Steel Python venv (steel-sdk + playwright client)...'
      if (-not (Test-Path -LiteralPath $venvPy)) {
        & $python.Source -m venv $venv
      }
      if (Test-Path -LiteralPath $venvPy) {
        & $venvPy -m pip install -r $req
      }
    } else {
      Write-Info 'Steel skill venv: from a Meido checkout, install builtin_skills/steel-browser/requirements.txt into workspace/shared/.venv-steel.'
    }
  } else {
    Write-Info 'Skipped Steel.'
  }

  Write-Info ''
  Write-Info 'Step — other built-ins'
  if (Confirm-Step 'Save TAVILY_API_KEY for the tavily-full web search skill?' $false) {
    Save-SecretKey 'TAVILY_API_KEY' 'Paste TAVILY_API_KEY (input hidden)'
  } else {
    Write-Info 'Web search still works via SearXNG (SEARXNG_URL) or the DuckDuckGo fallback.'
  }
  Write-Info 'Messaging channels (Telegram, Discord, WhatsApp, WeCom) are configured in Web UI → Settings → Integrations.'
  Write-Info ''
  Write-Info 'Built-in setup finished.'
  Write-Info 'meido start serves the web UI and returns immediately. It stays up until meido stop or a reboot.'
  Write-Info 'Stop that process: meido stop'
  Write-Info 'A login service (meido gateway install) exists on macOS and Linux only. It starts Meido at login and restarts it if the process exits. Windows uses meido start.'
  Write-WebUiUrl
}

$arch = Resolve-Arch
Write-Info "Installing Meido for windows/$arch..."

$release = Invoke-RestMethod -Uri $ApiUrl -Headers @{ 'User-Agent' = 'meido-install-script' }
$assetUrl = Select-AssetUrl -release $release -arch $arch
if (-not $assetUrl) {
  throw "No prebuilt binary found for windows/$arch in the latest GitHub release."
}

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
$tmpDir = New-Item -ItemType Directory -Force -Path (Join-Path ([System.IO.Path]::GetTempPath()) ("meido-install-" + [guid]::NewGuid().ToString()))
try {
  $archivePath = Join-Path $tmpDir.FullName 'meido.zip'
  Write-Info "Downloading: $assetUrl"
  Invoke-WebRequest -Uri $assetUrl -OutFile $archivePath

  Expand-Archive -Path $archivePath -DestinationPath $tmpDir.FullName -Force
  $bin = Get-ChildItem -Path $tmpDir.FullName -Filter $BinName -Recurse | Select-Object -First 1
  if (-not $bin) {
    throw "Could not find $BinName in archive"
  }

  $targetPath = Join-Path $InstallDir $BinName
  Copy-Item -Path $bin.FullName -Destination $targetPath -Force

  $pathUpdated = Ensure-UserPathContains $InstallDir

  Write-Info "Installed Meido to: $targetPath"
  if ($pathUpdated) {
    Write-Info "Added '$InstallDir' to your user PATH."
    Write-Info "Open a new terminal if command lookup does not refresh immediately."
  } else {
    Write-Info "PATH already contains '$InstallDir'."
  }

  if (Get-Command meido -ErrorAction SilentlyContinue) {
    Write-Info "Run: meido help"
  } else {
    Write-Info "If 'meido' is not found, open a new terminal and run: meido help"
  }

  Write-Info "Optional runtime dependencies for built-in features:"
  if (Get-Command python -ErrorAction SilentlyContinue) {
    Write-Info "  python — present (MemPalace recall venv + Cursor Python rollback)"
  } else {
    Write-Info "  python — missing (MemPalace MCP auto-install and Cursor Python rollback need it)"
  }
  if ((Get-Command node -ErrorAction SilentlyContinue) -and (Get-Command npm -ErrorAction SilentlyContinue)) {
    Write-Info "  node/npm — present (Cursor SDK engine sidecar)"
  } else {
    Write-Info "  node/npm — missing (Cursor agent engine needs Node.js; Classic engine still works)"
  }
  if (-not (Get-Command docker -ErrorAction SilentlyContinue)) {
    Write-Info "  docker — missing (Steel browser skill needs Docker when BROWSER_MANAGED=true)"
  } else {
    Write-Info "  docker — present (Steel browser when BROWSER_MANAGED=true)"
  }

  Invoke-BuiltinGuide
} finally {
  Remove-Item -Recurse -Force $tmpDir.FullName -ErrorAction SilentlyContinue
}
