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

  if (-not (Test-Path -LiteralPath '.env') -and (Test-Path -LiteralPath '.env.example')) {
    Copy-Item -LiteralPath '.env.example' -Destination '.env'
    Write-Info 'Copied .env.example to .env.'
  }
  Write-Info ''
  Write-Info 'Start Meido, then finish setup in the web UI.'
  Write-Info '  meido start'
  Write-Info '  open http://127.0.0.1:10961'
  Write-Info 'The setup wizard opens when workspace, vault, or the system LLM is not set in .env.'
  Write-Info 'Edit .env directly any time. Settings shows the same values and writes them back.'
} finally {
  Remove-Item -Recurse -Force $tmpDir.FullName -ErrorAction SilentlyContinue
}
