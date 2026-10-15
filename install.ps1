param(
  [string]$Repo = $(if ($env:FINALLY_A_VALUE_BOT_REPO) { $env:FINALLY_A_VALUE_BOT_REPO } else { 'finally-a-value-bot/finally-a-value-bot' }),
  [string]$InstallDir = $(if ($env:FINALLY_A_VALUE_BOT_INSTALL_DIR) { $env:FINALLY_A_VALUE_BOT_INSTALL_DIR } else { Join-Path $env:USERPROFILE '.local\bin' })
)

$ErrorActionPreference = 'Stop'
$BinName = 'finally-a-value-bot.exe'
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
    "finally-a-value-bot-v?[0-9]+\.[0-9]+\.[0-9]+-$arch-pc-windows-msvc\.zip$",
    "finally-a-value-bot-v?[0-9]+\.[0-9]+\.[0-9]+-.*$arch.*windows.*\.zip$"
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
Write-Info "Installing finally-a-value-bot for windows/$arch..."

$release = Invoke-RestMethod -Uri $ApiUrl -Headers @{ 'User-Agent' = 'finally-a-value-bot-install-script' }
$assetUrl = Select-AssetUrl -release $release -arch $arch
if (-not $assetUrl) {
  throw "No prebuilt binary found for windows/$arch in the latest GitHub release."
}

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
$tmpDir = New-Item -ItemType Directory -Force -Path (Join-Path ([System.IO.Path]::GetTempPath()) ("finally-a-value-bot-install-" + [guid]::NewGuid().ToString()))
try {
  $archivePath = Join-Path $tmpDir.FullName 'finally-a-value-bot.zip'
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

  Write-Info "Installed finally-a-value-bot to: $targetPath"
  if ($pathUpdated) {
    Write-Info "Added '$InstallDir' to your user PATH."
    Write-Info "Open a new terminal if command lookup does not refresh immediately."
  } else {
    Write-Info "PATH already contains '$InstallDir'."
  }

  if (Get-Command finally-a-value-bot -ErrorAction SilentlyContinue) {
    Write-Info "Run: finally-a-value-bot help"
  } else {
    Write-Info "If 'finally-a-value-bot' is not found, open a new terminal and run: finally-a-value-bot help"
  }

  if (-not (Get-Command docker -ErrorAction SilentlyContinue)) {
    Write-Info "Optional: install Docker for Steel browser automation (steel-browser skill)."
  }

  Write-Info ""
  Write-Info "Next steps:"
  Write-Info "  1) In a project folder, add .env (copy .env.example from the repo if needed)"
  Write-Info "  2) Set bootstrap vars in .env (workspace + web host/port/auth when needed)"
  Write-Info "  3) Start: finally-a-value-bot start"
  Write-Info "  4) Open Web UI: http://127.0.0.1:10961 and finish channel/LLM/persona settings"
  Write-Info "See README.md — finally-a-value-bot doctor for preflight checks."
} finally {
  Remove-Item -Recurse -Force $tmpDir.FullName -ErrorAction SilentlyContinue
}
